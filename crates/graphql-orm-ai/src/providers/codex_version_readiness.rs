//! Credential-free executable compatibility evidence, separate from model probes.
use crate::ProviderError;
use std::collections::BTreeSet;

/// Nonempty exact release allow-list, bounded to 64 input entries.
/// No ranges, wildcards, prereleases, build metadata or leading zeroes are accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiCodexAppServerSupportedVersions(BTreeSet<String>);
impl AiCodexAppServerSupportedVersions {
    /// Validates and canonicalizes an exact allow-list.
    ///
    /// # Errors
    /// Returns a content-free configuration error for empty, malformed or oversized input.
    pub fn exact<I, S>(versions: I) -> Result<Self, ProviderError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut result = BTreeSet::new();
        for (index, version) in versions.into_iter().enumerate() {
            let version = version.as_ref();
            if index >= 64 || !release(version) {
                return Err(invalid());
            }
            result.insert(version.to_owned());
        }
        if result.is_empty() {
            return Err(invalid());
        }
        Ok(Self(result))
    }
    pub(super) fn versions(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
    pub(super) fn contains(&self, version: &str) -> bool {
        self.0.contains(version)
    }
}
fn invalid() -> ProviderError {
    ProviderError::InvalidConfiguration("invalid Codex supported versions".to_owned())
}
fn release(version: &str) -> bool {
    if version.len() > 32 {
        return false;
    }
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && (part.len() == 1 || !part.starts_with('0'))
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u32>().is_ok()
        })
}

/// Closed content-free compatibility result. This grants no protocol or tool authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiCodexAppServerVersionReadiness {
    /// No declaration; model admission retains legacy behavior.
    NotDeclared,
    /// Observed image and version match the registration and exact allow-list.
    Supported,
    /// Observed release is outside the exact allow-list.
    Unsupported,
    /// Factory has not supplied fresh executable evidence.
    Missing,
    /// Output was not the bounded strict Codex CLI version format.
    Unparseable,
    /// Probe failed, exceeded its deadline or exited unsuccessfully.
    ProbeFailed,
    /// Observed digest or version differs from the reviewed registration.
    IdentityMismatch,
}
impl AiCodexAppServerVersionReadiness {
    /// Whether compatibility policy permits admission; undeclared preserves legacy behavior.
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::NotDeclared | Self::Supported)
    }
    /// Stable content-free code safe for metrics, logs and client errors.
    pub const fn reason_code(self) -> &'static str {
        match self {
            Self::NotDeclared => "codex_version_not_declared",
            Self::Supported => "codex_version_supported",
            Self::Unsupported => "codex_version_unsupported",
            Self::Missing => "codex_version_missing",
            Self::Unparseable => "codex_version_unparseable",
            Self::ProbeFailed => "codex_version_probe_failed",
            Self::IdentityMismatch => "codex_version_identity_mismatch",
        }
    }
}

/// Trusted factory attestation of `--version` from its verified executable image.
///
/// Construct only after directly running the fixed absolute executable with cleared
/// environment, disposable empty HOME/CODEX_HOME and working directory, no credentials,
/// bounded stdout (at most 128 bytes), discarded stderr, a bounded deadline, and complete
/// process-tree cleanup. The factory must prevent image replacement between verification,
/// probing and execution (for example by using an immutable image/handle). A path or
/// deployment-authored version alone proves nothing. Each launch requires fresh evidence.
/// Debug deliberately excludes even validated executable identity and version.
#[derive(Clone)]
pub struct AiCodexAppServerVersionObservation {
    status: AiCodexAppServerVersionReadiness,
    version: Option<String>,
    digest: Option<String>,
}
impl std::fmt::Debug for AiCodexAppServerVersionObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiCodexAppServerVersionObservation")
            .field("status", &self.status)
            .finish()
    }
}
impl AiCodexAppServerVersionObservation {
    /// Unavailable observation, including compatibility defaults for existing factories.
    pub const fn missing() -> Self {
        Self {
            status: AiCodexAppServerVersionReadiness::Missing,
            version: None,
            digest: None,
        }
    }
    /// Failed or timed-out probe; never include raw output in an error.
    pub const fn probe_failed() -> Self {
        Self {
            status: AiCodexAppServerVersionReadiness::ProbeFailed,
            version: None,
            digest: None,
        }
    }
    /// Parses only successful bounded UTF-8 `codex-cli major.minor.patch` output,
    /// with one optional LF or CRLF. Invalid evidence is retained only as a closed code.
    /// The digest must come from the factory's verified image, never copied from configuration.
    pub fn from_version_output(stdout: &[u8], success: bool, executable_sha256: &str) -> Self {
        if !success {
            return Self::probe_failed();
        }
        let parsed = (stdout.len() <= 128)
            .then(|| std::str::from_utf8(stdout).ok())
            .flatten()
            .and_then(|s| {
                s.strip_suffix("\r\n")
                    .or_else(|| s.strip_suffix('\n'))
                    .or(Some(s))
            })
            .and_then(|s| s.strip_prefix("codex-cli "))
            .filter(|s| release(s));
        let Some(version) = parsed else {
            return Self {
                status: AiCodexAppServerVersionReadiness::Unparseable,
                ..Self::missing()
            };
        };
        if !crate::valid_sha256(executable_sha256) {
            return Self {
                status: AiCodexAppServerVersionReadiness::IdentityMismatch,
                ..Self::missing()
            };
        }
        Self {
            status: AiCodexAppServerVersionReadiness::Supported,
            version: Some(version.to_owned()),
            digest: Some(executable_sha256.to_owned()),
        }
    }
    pub(super) fn check(
        &self,
        policy: Option<&AiCodexAppServerSupportedVersions>,
        digest: &str,
        version: &str,
    ) -> AiCodexAppServerVersionReadiness {
        use AiCodexAppServerVersionReadiness as R;
        let Some(policy) = policy else {
            return R::NotDeclared;
        };
        if self.status != R::Supported {
            return self.status;
        }
        let observed = self.version.as_deref().unwrap_or_default();
        if !policy.contains(observed) {
            return R::Unsupported;
        }
        if self.digest.as_deref() != Some(digest) || observed != version {
            return R::IdentityMismatch;
        }
        R::Supported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_bounds_and_strict_output() {
        for v in [
            "",
            "1.2",
            "01.2.3",
            "1.2.3-alpha",
            "1.2.3+build",
            "*",
            "^1.2.3",
            "4294967296.1.1",
            " 1.2.3",
        ] {
            assert!(AiCodexAppServerSupportedVersions::exact([v]).is_err());
        }
        assert!(AiCodexAppServerSupportedVersions::exact::<[&str; 0], &str>([]).is_err());
        assert!(AiCodexAppServerSupportedVersions::exact(["1.2.3"; 64]).is_ok());
        assert!(AiCodexAppServerSupportedVersions::exact(["1.2.3"; 65]).is_err());
        let policy = AiCodexAppServerSupportedVersions::exact(["1.2.3"]).unwrap();
        let digest = "a".repeat(64);
        for suffix in ["", "\n", "\r\n"] {
            let o = AiCodexAppServerVersionObservation::from_version_output(
                format!("codex-cli 1.2.3{suffix}").as_bytes(),
                true,
                &digest,
            );
            assert_eq!(
                o.check(Some(&policy), &digest, "1.2.3"),
                AiCodexAppServerVersionReadiness::Supported
            );
        }
        for bytes in [
            b"codex-cli 1.2.3\n\n".as_slice(),
            b"userAgent/1.2.3",
            &[255],
            &[b'a'; 129],
        ] {
            assert_eq!(
                AiCodexAppServerVersionObservation::from_version_output(bytes, true, &digest)
                    .status,
                AiCodexAppServerVersionReadiness::Unparseable
            );
        }
        assert_eq!(
            AiCodexAppServerVersionObservation::from_version_output(
                b"codex-cli 1.2.3",
                false,
                &digest
            )
            .status,
            AiCodexAppServerVersionReadiness::ProbeFailed
        );
    }
}

#[cfg(all(test, unix))]
mod live_tests {
    use super::*;
    use std::{
        io::Read,
        os::unix::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    // No credentials, configuration, model request, or real user home.
    #[test]
    #[ignore = "requires GRAPHQL_ORM_AI_CODEX_VERSION_BIN absolute real executable"]
    fn real_binary_version_only() {
        let executable = std::env::var("GRAPHQL_ORM_AI_CODEX_VERSION_BIN").unwrap();
        assert!(std::path::Path::new(&executable).is_absolute());
        use sha2::Digest as _;
        let digest = hex::encode(sha2::Sha256::digest(std::fs::read(&executable).unwrap()));
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new(&executable)
            .arg("--version")
            .env_clear()
            .env("HOME", home.path())
            .env("CODEX_HOME", home.path())
            .current_dir(home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.by_ref().take(129).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // On timeout the leader is unreaped, so its group ID cannot be reused.
        // Successful real Codex --version exits without persistent children.
        if status.is_none() {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .stderr(Stdio::null())
                .status();
        }
        let _ = child.wait();
        let bytes = reader.join().unwrap();
        let observation = AiCodexAppServerVersionObservation::from_version_output(
            &bytes,
            status.is_some_and(|s| s.success()),
            &digest,
        );
        assert_eq!(
            observation.status,
            AiCodexAppServerVersionReadiness::Supported
        );
    }
}
