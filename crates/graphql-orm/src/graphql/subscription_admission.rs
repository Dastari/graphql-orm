//! Structural admission shared by generated and runtime-IR subscriptions.
use async_graphql::{
    Context, ServerError, Variables,
    extensions::{
        Extension, ExtensionContext, ExtensionFactory, NextParseQuery, NextPrepareRequest,
        NextValidation,
    },
    parser::types::{ExecutableDocument, OperationDefinition, OperationType, Selection},
};
use std::sync::{Arc, Mutex};

/// Count occurrences without merging aliases or evaluating directives.
pub(crate) fn admit(
    document: &ExecutableDocument,
    operation: &OperationDefinition,
    protected: impl Fn(&str) -> bool,
    max_depth: usize,
    max_selections: usize,
) -> Result<(), &'static str> {
    if operation.ty != OperationType::Subscription {
        return Ok(());
    }
    let mut stack = vec![(&operation.selection_set.node, 0usize)];
    let (mut expanded, mut roots, mut selected) = (0usize, 0usize, false);
    while let Some((selections, depth)) = stack.pop() {
        if depth > max_depth {
            return Err("cost_exceeded");
        }
        for selection in &selections.items {
            expanded = expanded.checked_add(1).ok_or("cost_exceeded")?;
            if expanded > max_selections {
                return Err("cost_exceeded");
            }
            match &selection.node {
                Selection::Field(field) => {
                    roots += 1;
                    selected |= protected(field.node.name.node.as_str());
                }
                Selection::InlineFragment(fragment) => {
                    stack.push((&fragment.node.selection_set.node, depth + 1))
                }
                Selection::FragmentSpread(spread) => {
                    let fragment = document
                        .fragments
                        .get(&spread.node.fragment_name.node)
                        .ok_or("invalid_graphql_input")?;
                    stack.push((&fragment.node.selection_set.node, depth + 1));
                }
            }
        }
    }
    if selected && roots > 1 {
        Err("subscription_root_limit_exceeded")
    } else {
        Ok(())
    }
}

fn error(code: &str) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    async_graphql::Error::new(code).extend_with(|_, extensions| extensions.set("code", code))
}

/// Require the extension's private request mark before any generated resolver work.
/// Missing or misconfigured admission fails with SUBSCRIPTION_ADMISSION_REQUIRED.
#[doc(hidden)]
pub fn admit_generated_operation(ctx: &Context<'_>) -> async_graphql::Result<()> {
    if !ctx
        .data_opt::<AdmissionMark>()
        .is_some_and(|mark| mark.0.contains(&ctx.item.node.name.node.as_str()))
    {
        use async_graphql::ErrorExtensions;
        return Err(async_graphql::Error::new(
            "Generated Changed subscriptions require GeneratedSubscriptionAdmission",
        )
        .extend_with(|_, extensions| extensions.set("code", "SUBSCRIPTION_ADMISSION_REQUIRED")));
    }
    let document = ExecutableDocument {
        operations: async_graphql::parser::types::DocumentOperations::Single(
            ctx.query_env.operation.clone(),
        ),
        fragments: ctx.query_env.fragments.clone(),
    };
    admit(
        &document,
        &ctx.query_env.operation.node,
        |_| true,
        64,
        100_000,
    )
    .map_err(error)
}

/// Refuse multi-root operations selecting one of the named generated Changed roots.
///
/// Generated `schema_builder` helpers install this automatically. Manually built
/// schemas must install it too, including every generated Changed field's actual
/// GraphQL name (after resolver-case configuration). This ensures skipped roots
/// and custom sibling sources are checked before any resolver starts. The
/// extension installs a private request mark; resolvers without the mark, or
/// whose field name is absent from the configured list, fail closed with
/// `SUBSCRIPTION_ADMISSION_REQUIRED` before authority checks or receiver creation.
///
/// ```
/// use graphql_orm::graphql::subscriptions::GeneratedSubscriptionAdmission;
/// let admission = GeneratedSubscriptionAdmission(vec!["recordChanged"]);
/// // Pass admission to async_graphql::Schema::build(...).extension(admission).
/// ```
#[derive(Clone)]
pub struct GeneratedSubscriptionAdmission(pub Vec<&'static str>);

impl ExtensionFactory for GeneratedSubscriptionAdmission {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(Admission {
            roots: self.0.clone(),
            parsed: Mutex::new(None),
            selected: Mutex::new(None),
        })
    }
}
struct AdmissionMark(Vec<&'static str>);

struct Admission {
    selected: Mutex<Option<String>>,
    roots: Vec<&'static str>,
    parsed: Mutex<Option<ExecutableDocument>>,
}
#[async_graphql::async_trait::async_trait]
impl Extension for Admission {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        request: async_graphql::Request,
        next: NextPrepareRequest<'_>,
    ) -> async_graphql::ServerResult<async_graphql::Request> {
        let request = next
            .run(ctx, request.data(AdmissionMark(self.roots.clone())))
            .await?;
        *self.selected.lock().unwrap() = request.operation_name.clone();
        Ok(request)
    }

    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> async_graphql::ServerResult<ExecutableDocument> {
        let document = next.run(ctx, query, variables).await?;
        *self.parsed.lock().unwrap() = Some(document.clone());
        Ok(document)
    }
    async fn validation(
        &self,
        ctx: &ExtensionContext<'_>,
        next: NextValidation<'_>,
    ) -> Result<async_graphql::ValidationResult, Vec<ServerError>> {
        if let Some(document) = self.parsed.lock().unwrap().as_ref() {
            let selected = self.selected.lock().unwrap();
            for (_, operation) in document.operations.iter().filter(|(name, _)| {
                selected
                    .as_deref()
                    .map_or(document.operations.iter().len() == 1, |expected| {
                        name.is_some_and(|name| name == expected)
                    })
            }) {
                admit(
                    document,
                    &operation.node,
                    |name| self.roots.contains(&name),
                    64,
                    100_000,
                )
                .map_err(|code| {
                    vec![error(code).into_server_error(async_graphql::Pos::default())]
                })?;
            }
        }
        next.run(ctx).await
    }
}
