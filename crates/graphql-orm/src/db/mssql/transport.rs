//! SQL Server transport security policy.
//!
//! The pinned driver treats an ADO connection string without `Encrypt` as
//! login-only TLS: it encrypts the login packet and then sends SQL, parameters
//! and results in clear unless the server forces encryption. It also ignores
//! keys it does not recognise, so a misspelt security key silently weakens the
//! connection. The ORM validates the security-relevant keys itself, before the
//! driver sees them, and requires full-session TLS with a validated server
//! certificate unless the caller opts into a weaker mode in code.

use tiberius::{Config, EncryptionLevel};

/// Transport security a SQL Server connection must meet.
///
/// The default, [`MssqlTransportPolicy::verified`], requires TLS for the whole
/// session and validates the server certificate chain and hostname against
/// the platform trust store, or against the private root named by
/// `TrustServerCertificateCA`. Each weaker mode is a separate, explicit opt-in
/// that a connection string alone cannot enable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MssqlTransportPolicy {
    unverified_certificate: bool,
    login_only_encryption: bool,
}

impl MssqlTransportPolicy {
    /// Full-session TLS with a validated server certificate. This is the default.
    pub const fn verified() -> Self {
        Self {
            unverified_certificate: false,
            login_only_encryption: false,
        }
    }

    /// **Insecure.** Accept `TrustServerCertificate=true`.
    ///
    /// The session stays encrypted, but the server certificate's chain and
    /// hostname are not checked. That defeats passive capture only: anyone on
    /// the network path can impersonate the server and read the login
    /// credentials and all traffic. Reserve it for disposable local servers.
    /// Prefer `TrustServerCertificateCA=<path>`, which keeps validation on.
    pub const fn insecure_allow_unverified_certificate(mut self) -> Self {
        self.unverified_certificate = true;
        self
    }

    /// **Insecure.** Accept `Encrypt=false`, and leave the encryption level of
    /// a caller-built [`Config`] as the caller set it.
    ///
    /// With login-only encryption TLS protects only the login packet. Every
    /// later SQL statement, parameter and result crosses the network in clear
    /// unless the server forces encryption.
    pub const fn insecure_allow_login_only_encryption(mut self) -> Self {
        self.login_only_encryption = true;
        self
    }

    /// Whether `TrustServerCertificate=true` is accepted.
    pub const fn allows_unverified_certificate(&self) -> bool {
        self.unverified_certificate
    }

    /// Whether login-only encryption is accepted.
    pub const fn allows_login_only_encryption(&self) -> bool {
        self.login_only_encryption
    }

    /// Apply this policy to a caller-built driver configuration.
    ///
    /// Unless login-only encryption is allowed, the configuration's
    /// encryption level is raised to `Required`. The driver exposes no way to
    /// read back certificate trust from a [`Config`], so a caller that calls
    /// [`Config::trust_cert`] itself has chosen an unverified certificate in
    /// code; build pools from connection strings to have it validated.
    pub(crate) fn apply(self, config: &mut Config) {
        if !self.login_only_encryption {
            config.encryption(EncryptionLevel::Required);
        }
    }
}

/// Keys the pinned driver reads from an ADO connection string, lowercased.
const RECOGNISED_KEYS: &[&str] = &[
    "server",
    "data source",
    "uid",
    "username",
    "user",
    "user id",
    "password",
    "pwd",
    "database",
    "initial catalog",
    "databasename",
    "application name",
    "applicationname",
    "applicationintent",
    "trustservercertificate",
    "trustservercertificateca",
    "encrypt",
    "integratedsecurity",
    "integrated security",
];

/// Fragments that mark an unrecognised key as security-relevant. The driver
/// ignores unknown keys, so a misspelt or unsupported spelling of one of these
/// would silently leave the connection weaker than the operator asked for.
const SECURITY_KEY_MARKERS: &[&str] = &[
    "encrypt",
    "trust",
    "ssl",
    "tls",
    "cert",
    "integrated",
    "authentication",
];

/// Parse an ADO connection string under `policy`.
///
/// The returned configuration's encryption level is final: `Required` unless
/// `Encrypt=false` is present and `policy` allows login-only encryption.
/// Errors name keys only and never echo values, so credentials do not leak
/// into diagnostics.
pub(crate) fn config_from_ado(
    connection_string: &str,
    policy: MssqlTransportPolicy,
) -> crate::Result<Config> {
    let encryption = validate_ado(connection_string, policy)?;
    let mut config = Config::from_ado_string(connection_string)
        .map_err(|error| configuration(format!("invalid SQL Server connection string: {error}")))?;
    config.encryption(encryption);
    Ok(config)
}

fn validate_ado(
    connection_string: &str,
    policy: MssqlTransportPolicy,
) -> crate::Result<EncryptionLevel> {
    let entries: connection_string::AdoNetString = connection_string.parse().map_err(|_| {
        configuration("invalid SQL Server connection string: it is not valid ADO.NET syntax")
    })?;
    let value = |key: &str| entries.get(key).map(|value| value.trim());

    for key in entries.keys() {
        let key = key.trim();
        if RECOGNISED_KEYS.contains(&key) {
            continue;
        }
        let compact: String = key
            .chars()
            .filter(|character| !matches!(character, ' ' | '_' | '-'))
            .collect();
        if SECURITY_KEY_MARKERS
            .iter()
            .any(|marker| compact.contains(marker))
        {
            return Err(configuration(format!(
                "SQL Server connection string key `{key}` is not supported by this driver and \
                 would be ignored, leaving the connection weaker than requested; use `Encrypt`, \
                 `TrustServerCertificate` or `TrustServerCertificateCA`"
            )));
        }
    }

    for key in ["integratedsecurity", "integrated security"] {
        if let Some(setting) = value(key)
            && !matches!(setting.to_ascii_lowercase().as_str(), "false" | "no")
        {
            return Err(configuration(
                "SQL Server integrated authentication is not supported by this build; the driver \
                 would silently fall back to SQL authentication. Remove `Integrated Security` and \
                 supply a SQL login",
            ));
        }
    }

    let trust_all = match value("trustservercertificate") {
        None => false,
        Some(setting) => parse_bool(setting).ok_or_else(|| {
            configuration(
                "SQL Server `TrustServerCertificate` must be `true`, `false`, `yes` or `no`",
            )
        })?,
    };
    if trust_all && value("trustservercertificateca").is_some() {
        return Err(configuration(
            "SQL Server `TrustServerCertificate=true` and `TrustServerCertificateCA` are mutually \
             exclusive; keep `TrustServerCertificateCA` so the certificate is still validated",
        ));
    }

    let encryption = match value("encrypt") {
        // The driver would choose login-only TLS here. Require the full session.
        None => EncryptionLevel::Required,
        Some("DANGER_PLAINTEXT") => {
            return Err(configuration(
                "SQL Server `Encrypt=DANGER_PLAINTEXT` sends the login and every statement \
                 without TLS and is not supported",
            ));
        }
        Some(setting) => match parse_bool(setting) {
            Some(true) => EncryptionLevel::Required,
            Some(false) if policy.allows_login_only_encryption() => EncryptionLevel::Off,
            Some(false) => {
                return Err(configuration(
                    "SQL Server `Encrypt=false` encrypts only the login; SQL, parameters and \
                     results would then cross the network in clear. Remove it to require \
                     full-session TLS, or opt in with \
                     `MssqlTransportPolicy::insecure_allow_login_only_encryption`",
                ));
            }
            None => {
                return Err(configuration(
                    "SQL Server `Encrypt` must be `true`, `false`, `yes` or `no`; this driver does \
                     not support `strict`, `mandatory` or `optional`",
                ));
            }
        },
    };

    if trust_all && !policy.allows_unverified_certificate() {
        return Err(configuration(
            "SQL Server `TrustServerCertificate=true` disables certificate chain and hostname \
             validation, so an encrypted connection can be intercepted by anyone on the network \
             path. Trust a private root with `TrustServerCertificateCA=<path>`, or opt in with \
             `MssqlTransportPolicy::insecure_allow_unverified_certificate`",
        ));
    }

    Ok(encryption)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
}

fn configuration(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Configuration(message.into().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str =
        "server=tcp:db.internal.example,1433;database=app;user id=app;password=S3cret;value=1";

    fn level(extra: &str, policy: MssqlTransportPolicy) -> crate::Result<EncryptionLevel> {
        validate_ado(&format!("{BASE};{extra}"), policy)
    }

    fn refused(extra: &str, policy: MssqlTransportPolicy) -> String {
        let error = level(extra, policy).expect_err("connection string must be refused");
        let message = error.to_string();
        assert!(matches!(error, sqlx::Error::Configuration(_)), "{message}");
        assert!(
            !message.contains("S3cret"),
            "diagnostics must not echo credentials"
        );
        message
    }

    #[test]
    fn omitted_encrypt_requires_full_session_tls() {
        let policy = MssqlTransportPolicy::verified();
        assert_eq!(level("", policy).unwrap(), EncryptionLevel::Required);
        assert_eq!(
            level("Encrypt=true", policy).unwrap(),
            EncryptionLevel::Required
        );
        assert_eq!(
            level("encrypt=YES", policy).unwrap(),
            EncryptionLevel::Required
        );
        assert_eq!(
            level("TrustServerCertificateCA=/etc/ssl/private-root.pem", policy).unwrap(),
            EncryptionLevel::Required
        );
        assert_eq!(
            level("TrustServerCertificate=false", policy).unwrap(),
            EncryptionLevel::Required
        );
        let config = config_from_ado(BASE, policy).unwrap();
        assert_eq!(config.get_addr(), "db.internal.example:1433");
    }

    #[test]
    fn login_only_encryption_is_an_explicit_opt_in() {
        let message = refused("Encrypt=false", MssqlTransportPolicy::verified());
        assert!(message.contains("insecure_allow_login_only_encryption"));
        refused("Encrypt=no", MssqlTransportPolicy::verified());
        let policy = MssqlTransportPolicy::verified().insecure_allow_login_only_encryption();
        assert_eq!(
            level("Encrypt=false", policy).unwrap(),
            EncryptionLevel::Off
        );
        // Allowing login-only encryption does not weaken an omitted key.
        assert_eq!(level("", policy).unwrap(), EncryptionLevel::Required);
    }

    #[test]
    fn unverified_certificates_are_an_explicit_opt_in() {
        for setting in ["TrustServerCertificate=true", "trustservercertificate=Yes"] {
            let message = refused(setting, MssqlTransportPolicy::verified());
            assert!(message.contains("TrustServerCertificateCA"));
            // Asking for encryption does not make an unverified certificate safe.
            refused(
                &format!("Encrypt=true;{setting}"),
                MssqlTransportPolicy::verified(),
            );
            refused(
                &format!("Encrypt=false;{setting}"),
                MssqlTransportPolicy::verified().insecure_allow_login_only_encryption(),
            );
        }
        let policy = MssqlTransportPolicy::verified().insecure_allow_unverified_certificate();
        assert_eq!(
            level("Encrypt=true;TrustServerCertificate=true", policy).unwrap(),
            EncryptionLevel::Required
        );
        assert_eq!(
            level("TrustServerCertificate=true", policy).unwrap(),
            EncryptionLevel::Required
        );
        // Allowing an unverified certificate does not allow login-only TLS.
        refused("Encrypt=false;TrustServerCertificate=true", policy);
    }

    #[test]
    fn misspelt_and_unsupported_security_keys_are_refused() {
        let everything = MssqlTransportPolicy::verified()
            .insecure_allow_unverified_certificate()
            .insecure_allow_login_only_encryption();
        for extra in [
            "Encryption=true",
            "Encrypted=true",
            "SslMode=require",
            "UseTls=true",
            "Trust Server Certificate=true",
            "Trust_Server_Certificate=false",
            "HostNameInCertificate=db.internal.example",
            "Column Encryption Setting=Enabled",
            "Authentication=ActiveDirectoryPassword",
        ] {
            let message = refused(extra, everything);
            assert!(message.contains("not supported"), "{extra}: {message}");
        }
        for extra in [
            "IntegratedSecurity=true",
            "Integrated Security=SSPI",
            "integrated security=yes",
        ] {
            let message = refused(extra, everything);
            assert!(message.contains("integrated authentication"), "{message}");
        }
        assert_eq!(
            level("Integrated Security=false;Connect Timeout=30", everything).unwrap(),
            EncryptionLevel::Required
        );
    }

    #[test]
    fn plaintext_and_contradictory_settings_are_always_refused() {
        let everything = MssqlTransportPolicy::verified()
            .insecure_allow_unverified_certificate()
            .insecure_allow_login_only_encryption();
        refused("Encrypt=DANGER_PLAINTEXT", everything);
        refused("Encrypt=strict", everything);
        refused("TrustServerCertificate=maybe", everything);
        // The driver panics on this combination; it must be refused first.
        let message = refused(
            "TrustServerCertificate=true;TrustServerCertificateCA=/etc/ssl/private-root.pem",
            everything,
        );
        assert!(message.contains("mutually exclusive"));
        assert!(
            config_from_ado(
                &format!(
                    "{BASE};TrustServerCertificate=true;TrustServerCertificateCA=/etc/ssl/root.pem"
                ),
                everything
            )
            .is_err()
        );
        assert!(matches!(
            validate_ado("server=;=;", everything),
            Err(sqlx::Error::Configuration(_))
        ));
    }
}
