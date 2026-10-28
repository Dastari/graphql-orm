//! Independent host proving base APIs, required scalars and additive features.
use async_graphql::{Error, ErrorExtensions, InputType, Json, OutputType};

/// Base parser, error extensions and JSON remain available without defaults.
pub fn base_contract() {
    let _ = async_graphql::parser::parse_query("{ __typename }").unwrap();
    let _ = Error::new("host error").extend_with(|_, extensions| {
        extensions.set("code", "HOST");
    });
    scalar::<Json<String>>();
}

fn scalar<T: InputType + OutputType>() {}

/// ORM's UUID, Decimal and loader modules remain available through its reexport.
#[cfg(feature = "orm")]
pub fn orm_contract() {
    use graphql_orm::async_graphql::{UploadValue, dataloader::DataLoader};
    scalar::<graphql_orm::uuid::Uuid>();
    scalar::<graphql_orm::rust_decimal::Decimal>();
    let _ = std::any::type_name::<DataLoader<()>>();
    #[cfg(not(feature = "additive"))]
    let _ = UploadValue {
        filename: "bytes.bin".into(),
        content_type: None,
        content: Vec::<u8>::new().into(),
    };
    #[cfg(feature = "additive")]
    let _ = std::any::type_name::<UploadValue>();
}

/// Dynamic registry stays opt-in.
#[cfg(feature = "runtime")]
pub fn runtime_contract() {
    let _ = graphql_orm::async_graphql::dynamic::Schema::build("Query", None, None);
}

#[test]
fn consumer_contract() {
    base_contract();
    #[cfg(feature = "orm")]
    orm_contract();
    #[cfg(feature = "runtime")]
    runtime_contract();
}

/// Public router constructors accept the host's exact canonical Auth types.
#[cfg(feature = "router-auth")]
pub fn router_auth_contract(
    validator: std::sync::Arc<agql_auth::AccessTokenValidator>,
    matcher: std::sync::Arc<dyn agql_auth::ScopeMatch>,
) {
    let _ = graphql_orm_router::AgqlAuthenticationProvider::new(validator);
    let _ = graphql_orm_router::AgqlScopeMatcher::new(matcher);
}
