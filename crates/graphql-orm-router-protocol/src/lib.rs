//! Versioned, project-neutral declarations exchanged by GraphQL subgraphs and
//! federation routers.
//!
//! Endpoint values in this crate are advertisements, not validated deployment
//! targets. The router that consumes them owns URL validation, SSRF policy,
//! credentials, and deployment-specific overrides.

#![deny(missing_docs)]

mod error;
mod fingerprint;
mod model;
mod version;

pub use error::{ProtocolError, ProtocolErrorKind};
pub use fingerprint::Fingerprint;
pub use model::{
    AdvertisedEndpoint, ArgumentDescriptor, AuthorizationRequirement, CapabilitySet,
    DescriptorFingerprints, GraphqlEndpoints, OperationDescriptor, RootOperationType,
    SchemaAdvertisement, ScopeSet, ScopeTemplate, SubgraphDescriptor, SubgraphDescriptorBuilder,
    SubgraphId, SubgraphIdentity, SubgraphName, UnrepresentablePolicy, UnrepresentablePolicyCode,
};
pub use version::{ProtocolVersion, SUPPORTED_PROTOCOL_VERSION};
