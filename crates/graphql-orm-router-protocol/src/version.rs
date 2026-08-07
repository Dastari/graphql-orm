use serde::{Deserialize, Serialize};

use crate::{ProtocolError, ProtocolErrorKind};

/// The protocol version implemented by this crate.
pub const SUPPORTED_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };

/// A protocol version with compatible-minor semantics inside one major version.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolVersion {
    /// The incompatible compatibility boundary.
    pub major: u16,
    /// An additive-compatible revision within `major`.
    pub minor: u16,
}

impl ProtocolVersion {
    /// Checks whether this producer version is readable by `supported`.
    ///
    /// A later minor is accepted because unknown additive fields are ignored.
    /// Required semantics are checked separately by [`crate::SubgraphDescriptor`].
    pub fn ensure_compatible_with(self, supported: Self) -> Result<(), ProtocolError> {
        if self.major == supported.major {
            Ok(())
        } else {
            Err(ProtocolError::new(
                ProtocolErrorKind::IncompatibleMajorVersion,
                format!(
                    "descriptor major {} is incompatible with supported major {}",
                    self.major, supported.major
                ),
            ))
        }
    }
}
