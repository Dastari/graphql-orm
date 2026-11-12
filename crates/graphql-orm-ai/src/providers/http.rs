//! Shared HTTP client construction for provider adapters.
//!
//! The TLS stack is chosen at compile time by exactly one `provider-tls-*`
//! feature. With `provider-tls-ring` the client is built from an explicit
//! `ring` crypto provider and the platform certificate verifier, so the
//! process-wide default rustls provider is neither read nor installed.

use crate::ProviderError;

#[cfg(any(
    all(feature = "provider-tls-aws-lc", feature = "provider-tls-ring"),
    all(feature = "provider-tls-aws-lc", feature = "provider-tls-native"),
    all(feature = "provider-tls-ring", feature = "provider-tls-native"),
))]
compile_error!(
    "graphql-orm-ai provider adapters require exactly one TLS feature: \
     provider-tls-aws-lc, provider-tls-ring or provider-tls-native"
);

#[cfg(not(any(
    feature = "provider-tls-aws-lc",
    feature = "provider-tls-ring",
    feature = "provider-tls-native",
)))]
compile_error!(
    "graphql-orm-ai provider adapters (`provider-*-core` features) require one TLS feature: \
     provider-tls-aws-lc, provider-tls-ring or provider-tls-native"
);

/// Whether a provider client may use proxies from the process environment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProviderProxy {
    /// Ignore `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and platform proxy
    /// settings. The default.
    #[default]
    Disabled,
    /// Host opt-in: use the process proxy environment.
    Environment,
}

/// Returns a client builder using the compiled provider TLS stack, with the
/// process proxy environment ignored.
///
/// Callers add their own redirect policy, timeouts and limits.
#[cfg(test)]
pub(crate) fn client_builder() -> Result<reqwest::ClientBuilder, ProviderError> {
    client_builder_with_proxy(ProviderProxy::Disabled)
}

/// Returns a client builder using the compiled provider TLS stack and the
/// given proxy mode.
pub(crate) fn client_builder_with_proxy(
    proxy: ProviderProxy,
) -> Result<reqwest::ClientBuilder, ProviderError> {
    let builder = reqwest::Client::builder();
    let builder = match proxy {
        ProviderProxy::Disabled => builder.no_proxy(),
        ProviderProxy::Environment => builder,
    };
    #[cfg(feature = "provider-tls-ring")]
    let builder = builder.tls_backend_preconfigured(ring_client_config()?);
    Ok(builder)
}

/// Builds one provider client: redirects disabled, an overall timeout, and
/// the requested proxy mode.
///
/// A loopback endpoint never goes through a proxy, and `localhost` is pinned
/// to the loopback addresses rather than resolved through the system
/// resolver.
///
/// # Errors
///
/// Returns [`ProviderError::InvalidConfiguration`] naming `provider` when the
/// client cannot be built.
pub(crate) fn provider_client(
    provider: &str,
    timeout: std::time::Duration,
    endpoint: Option<&url::Url>,
    proxy: ProviderProxy,
) -> Result<reqwest::Client, ProviderError> {
    let loopback = endpoint.is_some_and(crate::configuration::provider_endpoint_is_loopback);
    let proxy = if loopback {
        ProviderProxy::Disabled
    } else {
        proxy
    };
    let mut builder = client_builder_with_proxy(proxy)?
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout);
    if endpoint
        .and_then(url::Url::host_str)
        .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
    {
        builder = builder.resolve_to_addrs(
            "localhost",
            &[
                std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
                std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 0)),
            ],
        );
    }
    builder.build().map_err(|_| {
        ProviderError::InvalidConfiguration(format!(
            "{provider} HTTP client could not be constructed"
        ))
    })
}

#[cfg(feature = "provider-tls-ring")]
fn ring_client_config() -> Result<rustls::ClientConfig, ProviderError> {
    use rustls_platform_verifier::BuilderVerifierExt;

    let unavailable = || {
        ProviderError::InvalidConfiguration("provider TLS configuration is unavailable".to_owned())
    };
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| unavailable())?
        .with_platform_verifier()
        .map_err(|_| unavailable())?
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(all(test, feature = "provider-tls-ring"))]
mod tests {
    #[test]
    fn ring_client_uses_an_explicit_provider_without_a_process_default() {
        let config = super::ring_client_config().expect("ring TLS config");
        assert_eq!(
            config.alpn_protocols,
            [b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        super::client_builder()
            .expect("builder")
            .build()
            .expect("client should build without a process default provider");
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    }

    /// A local rustls server records the client's ALPN offer, then presents a
    /// self-signed certificate that the platform verifier must refuse.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ring_client_offers_h2_and_refuses_an_untrusted_certificate() {
        use std::io::Write;
        use std::sync::Arc;

        let generated =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("test cert");
        let certificate = rustls::pki_types::CertificateDer::from(generated.cert.der().to_vec());
        let key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(generated.key_pair.serialize_der().into());
        let server_config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("server protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .expect("server certificate"),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client connects");
            let mut acceptor = rustls::server::Acceptor::default();
            let accepted = loop {
                acceptor.read_tls(&mut stream).expect("client hello");
                if let Some(accepted) = acceptor.accept().map_err(|_| ()).expect("valid hello") {
                    break accepted;
                }
            };
            let offered = accepted
                .client_hello()
                .alpn()
                .map(|protocols| protocols.map(<[u8]>::to_vec).collect::<Vec<_>>())
                .unwrap_or_default();
            let mut connection = accepted
                .into_connection(server_config)
                .map_err(|_| ())
                .expect("server connection");
            // The client aborts after it sees the untrusted certificate.
            let _ = connection.complete_io(&mut stream);
            let _ = stream.flush();
            offered
        });
        let client = super::client_builder()
            .expect("builder")
            .build()
            .expect("ring client");
        let result = client
            .get(format!("https://127.0.0.1:{port}/"))
            .send()
            .await;
        assert!(result.is_err(), "an untrusted certificate must be refused");
        let offered = server.join().expect("server thread");
        assert!(offered.contains(&b"h2".to_vec()), "{offered:?}");
        assert!(offered.contains(&b"http/1.1".to_vec()), "{offered:?}");
    }
}

/// Proxy environment variables are process-wide, so the probe runs in a child
/// copy of this test binary instead of mutating the parent environment.
#[cfg(test)]
mod proxy_environment_tests {
    const CHILD_MODE: &str = "GRAPHQL_ORM_AI_PROXY_PROBE";
    const CHILD_TEST: &str = "providers::http::proxy_environment_tests::proxy_environment_probe";

    /// Sends one request through a provider client. Runs only when spawned by
    /// [`run_probe`].
    #[tokio::test]
    #[ignore = "spawned by the proxy environment tests"]
    async fn proxy_environment_probe() {
        let Some(mode) = std::env::var_os(CHILD_MODE) else {
            return;
        };
        let builder = if mode == "environment" {
            super::client_builder_with_proxy(super::ProviderProxy::Environment)
        } else {
            super::client_builder()
        };
        let client = builder
            .expect("builder")
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("client");
        let _ = client.get("http://proxy-probe.invalid/").send().await;
    }

    /// Returns whether the child's request reached the proxy named in its
    /// environment.
    fn run_probe(mode: &str) -> bool {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("proxy listener");
        let proxy = format!("http://{}", listener.local_addr().expect("proxy address"));
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                CHILD_TEST,
                "--exact",
                "--ignored",
                "--test-threads=1",
                "--quiet",
            ])
            .env(CHILD_MODE, mode)
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("all_proxy", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .status()
            .expect("probe child should run");
        assert!(status.success(), "probe child failed: {status}");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        listener.accept().is_ok()
    }

    #[test]
    fn provider_clients_ignore_proxy_environment_by_default() {
        assert!(
            !run_probe("default"),
            "a provider client followed the proxy environment"
        );
    }

    #[test]
    fn provider_clients_follow_proxy_environment_only_when_opted_in() {
        assert!(
            run_probe("environment"),
            "an opted-in provider client should use the proxy environment"
        );
    }
}
