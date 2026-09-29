//! Trusted, content-free application refusal evidence.

/// Public reason selected by the trusted execution owner before any effect.
///
/// These reasons may be disclosed only when current host policy permits the
/// distinction. They are not inferred from resolver text or HTTP status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolPreExecutionRejectionReason {
    /// Current authentication or assurance must be renewed.
    AuthenticationRequired,
    /// Current authority does not permit this exact operation.
    AuthorizationDenied,
    /// The affected resource requires a separate consent decision.
    ConsentRequired,
    /// The authorized resource is currently unavailable.
    ResourceUnavailable,
    /// The resource does not support the requested capability.
    CapabilityUnsupported,
    /// The exact published operation binding has changed.
    CapabilityStale,
    /// Arguments violate the public operation contract.
    InvalidArguments,
    /// The request exceeds a reviewed resource limit.
    ResourceLimitExceeded,
    /// A dependency is temporarily unavailable before execution.
    TemporarilyUnavailable,
}

impl ToolPreExecutionRejectionReason {
    /// Closed model/browser code that explicitly establishes no execution.
    pub const fn failure_code(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "not_started_authentication_required",
            Self::AuthorizationDenied => "not_started_authorization_denied",
            Self::ConsentRequired => "not_started_consent_required",
            Self::ResourceUnavailable => "not_started_resource_unavailable",
            Self::CapabilityUnsupported => "not_started_capability_unsupported",
            Self::CapabilityStale => "not_started_capability_stale",
            Self::InvalidArguments => "not_started_invalid_arguments",
            Self::ResourceLimitExceeded => "not_started_resource_limit_exceeded",
            Self::TemporarilyUnavailable => "not_started_temporarily_unavailable",
        }
    }

    /// Parses only the closed public codes, without accepting arbitrary detail.
    pub fn from_failure_code(code: &str) -> Option<Self> {
        [
            Self::AuthenticationRequired,
            Self::AuthorizationDenied,
            Self::ConsentRequired,
            Self::ResourceUnavailable,
            Self::CapabilityUnsupported,
            Self::CapabilityStale,
            Self::InvalidArguments,
            Self::ResourceLimitExceeded,
            Self::TemporarilyUnavailable,
        ]
        .into_iter()
        .find(|reason| reason.failure_code() == code)
    }
}

/// Trusted host attestation that one exact request crossed no effect boundary.
///
/// The binding hash prevents accidental cross-request or cross-attempt adoption;
/// it does not authenticate a remote server. The trusted host must verify the
/// execution owner's authenticated evidence before constructing this value.
/// Never construct it from a generic GraphQL extension, error text, timeout,
/// missing execution row, or an absence of output. This type deliberately has
/// no serialization/deserialization implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPreExecutionRejection {
    binding_hash: String,
    reason: ToolPreExecutionRejectionReason,
}

impl ToolPreExecutionRejection {
    /// Attests no effect for a crate-authored exact invocation binding.
    ///
    /// Prefer the AI registered/remote binding helpers. Calling this constructor
    /// asserts the trusted host verified no admission, dispatch, persisted work,
    /// or application effect for this exact attempt, including idempotency replay.
    /// The hash alone does not establish those facts or grant execution authority.
    ///
    /// # Errors
    /// Returns a safe error if the binding is not a lowercase SHA-256 digest.
    pub fn attest_trusted(
        binding_hash: String,
        reason: ToolPreExecutionRejectionReason,
    ) -> Result<Self, crate::ToolExecutionError> {
        if binding_hash.len() != 64
            || !binding_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(crate::ToolExecutionError::StaleContract);
        }
        Ok(Self {
            binding_hash,
            reason,
        })
    }

    /// Exact invocation digest; never a credential or execution authority.
    pub fn binding_hash(&self) -> &str {
        &self.binding_hash
    }

    /// Reviewed public refusal category.
    pub const fn reason(&self) -> ToolPreExecutionRejectionReason {
        self.reason
    }
}
