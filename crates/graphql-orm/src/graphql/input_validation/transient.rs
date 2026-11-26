use async_graphql::{
    Name, Pos, ServerError, Value,
    parser::types::{ExecutableDocument, Selection, SelectionSet},
    registry::{MetaType, Registry},
};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;

use super::operation_root;
use crate::graphql::orm::TRANSIENT_INPUT_INVALID;

/// async-graphql 7.2.1 reports argument value errors at the argument name
/// and default value errors at the variable definition. Match those source
/// locations against the schema and document, independent of error wording.
pub(super) fn redact_errors(
    registry: &Registry,
    document: &ExecutableDocument,
    errors: &mut [ServerError],
) {
    let error_locations: HashSet<_> = errors
        .iter()
        .filter_map(|error| match error.locations.as_slice() {
            [position] => Some(*position),
            _ => None,
        })
        .collect();
    if error_locations.is_empty() {
        return;
    }

    // Strict validation checks every operation and fragment, including those
    // outside the requested operation. A default belongs to a particular
    // operation, while a fragment can use that operation's variables.
    let mut transient_locations = HashSet::new();
    let fragments: HashMap<_, _> = document
        .fragments
        .iter()
        .map(|(name, fragment)| {
            let fragment = &fragment.node;
            let uses = TransientUses::collect(
                registry,
                registry
                    .types
                    .get(fragment.type_condition.node.on.node.as_str()),
                &fragment.selection_set.node,
                &mut transient_locations,
            );
            (name, uses)
        })
        .collect();
    for (_, operation) in document.operations.iter() {
        let operation = &operation.node;
        let root = operation_root(registry, operation).and_then(|name| registry.types.get(name));
        let mut uses = TransientUses::collect(
            registry,
            root,
            &operation.selection_set.node,
            &mut transient_locations,
        );
        let defaults: Vec<_> = operation
            .variable_definitions
            .iter()
            .filter(|definition| {
                definition.node.default_value.is_some() && error_locations.contains(&definition.pos)
            })
            .collect();
        if defaults.is_empty() {
            continue;
        }
        let mut seen = HashSet::new();
        while let Some(name) = uses.spreads.pop() {
            if seen.insert(name)
                && let Some(fragment) = fragments.get(name)
            {
                uses.variables.extend(fragment.variables.iter().cloned());
                uses.spreads.extend(&fragment.spreads);
            }
        }
        transient_locations.extend(
            defaults
                .into_iter()
                .filter(|definition| uses.variables.contains(&definition.node.name.node))
                .map(|definition| definition.pos),
        );
    }

    for error in errors {
        if matches!(error.locations.as_slice(), [position] if transient_locations.contains(position))
        {
            error.message = TRANSIENT_INPUT_INVALID.to_owned();
        }
    }
}

#[derive(Default)]
struct TransientUses<'a> {
    variables: HashSet<Name>,
    spreads: Vec<&'a Name>,
}

impl<'a> TransientUses<'a> {
    fn collect(
        registry: &Registry,
        root: Option<&MetaType>,
        selection_set: &'a SelectionSet,
        locations: &mut HashSet<Pos>,
    ) -> Self {
        let mut uses = Self::default();
        let mut pending = vec![(root, selection_set)];
        while let Some((parent, set)) = pending.pop() {
            for selection in &set.items {
                match &selection.node {
                    Selection::Field(field) => {
                        let field = &field.node;
                        let definition = parent.and_then(|ty| ty.field_by_name(&field.name.node));
                        if let Some(definition) = definition {
                            for (name, value) in &field.arguments {
                                let marked =
                                    definition.args.get(name.node.as_str()).is_some_and(|arg| {
                                        arg.directive_invocations
                                            .iter()
                                            .any(|d| d.name == "transientInput")
                                    });
                                if marked {
                                    locations.insert(name.pos);
                                    let _ = value.node.clone().into_const_with(|name| {
                                        uses.variables.insert(name);
                                        Ok::<_, Infallible>(Value::Null)
                                    });
                                }
                            }
                        }
                        pending.push((
                            definition.and_then(|field| registry.concrete_type_by_name(&field.ty)),
                            &field.selection_set.node,
                        ));
                    }
                    Selection::InlineFragment(fragment) => {
                        let fragment = &fragment.node;
                        let parent = match &fragment.type_condition {
                            Some(condition) => registry.types.get(condition.node.on.node.as_str()),
                            None => parent,
                        };
                        pending.push((parent, &fragment.selection_set.node));
                    }
                    Selection::FragmentSpread(spread) => {
                        uses.spreads.push(&spread.node.fragment_name.node);
                    }
                }
            }
        }
        uses
    }
}
