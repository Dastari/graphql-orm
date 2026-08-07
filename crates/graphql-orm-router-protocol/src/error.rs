use std::{error::Error, fmt};

/// Stable machine-readable categories returned while decoding or validating a descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolErrorKind {
    /// The JSON payload or one of its primitive values is malformed.
    MalformedPayload,
    /// The descriptor's protocol major version is unsupported.
    IncompatibleMajorVersion,
    /// The descriptor requires a semantic the reader does not implement.
    UnknownRequiredSemantics,
    /// A descriptor invariant other than a more specific category failed.
    InvalidDescriptor,
    /// A scope template is syntactically invalid.
    InvalidScopeTemplate,
    /// A scope template references an argument absent from its operation.
    UnknownTemplateArgument,
    /// Two operation descriptors identify the same root field.
    DuplicateOperation,
    /// A declared derived fingerprint differs from its canonical value.
    FingerprintMismatch,
}

impl ProtocolErrorKind {
    /// Returns the stable wire/logging code for this category.
    pub const fn code(self) -> &'static str {
        match self {
            Self::MalformedPayload => "MALFORMED_PAYLOAD",
            Self::IncompatibleMajorVersion => "INCOMPATIBLE_MAJOR_VERSION",
            Self::UnknownRequiredSemantics => "UNKNOWN_REQUIRED_SEMANTICS",
            Self::InvalidDescriptor => "INVALID_DESCRIPTOR",
            Self::InvalidScopeTemplate => "INVALID_SCOPE_TEMPLATE",
            Self::UnknownTemplateArgument => "UNKNOWN_TEMPLATE_ARGUMENT",
            Self::DuplicateOperation => "DUPLICATE_OPERATION",
            Self::FingerprintMismatch => "FINGERPRINT_MISMATCH",
        }
    }
}

/// A protocol decoding or compatibility error with a stable category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolError {
    kind: ProtocolErrorKind,
    detail: String,
}

impl ProtocolError {
    /// Creates an error in `kind` with human-readable detail.
    pub(crate) fn new(kind: ProtocolErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// Returns the stable category callers should branch or report on.
    pub const fn kind(&self) -> ProtocolErrorKind {
        self.kind
    }

    /// Returns contextual detail that is not a wire compatibility contract.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.kind.code(), self.detail)
    }
}

impl Error for ProtocolError {}
