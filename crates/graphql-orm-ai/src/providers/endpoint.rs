//! Exact endpoint normalization and request-time authorization for
//! deployment-configured provider routes.

use std::sync::Arc;

use url::Url;

use crate::{AiProviderEndpointPolicy, AiProviderKindInput, ProviderError};

/// Normalizes one deployment-configured endpoint: `http` or `https`, a host,
/// no user information, query or fragment, and a non-root path of at most
/// 2 KiB. The caller still authorizes the exact result through
/// [`ProviderEndpointGuard::authorize`].
#[cfg(any(
    feature = "provider-openai-compatible-core",
    feature = "provider-chat-completions-core",
    feature = "provider-anthropic-core"
))]
pub(super) fn normalized_provider_endpoint(value: &str) -> Result<Url, ProviderError> {
    let url = crate::configuration::parse_provider_endpoint(value).ok_or_else(unsafe_endpoint)?;
    if url.path().is_empty() || url.path() == "/" || url.path().len() > 2_048 {
        return Err(unsafe_endpoint());
    }
    Ok(url)
}

/// Normalizes one deployment-configured origin with an empty or root path to
/// its root URL.
#[cfg(feature = "provider-ollama-core")]
pub(super) fn normalized_provider_origin(value: &str) -> Result<Url, ProviderError> {
    let mut url =
        crate::configuration::parse_provider_endpoint(value).ok_or_else(unsafe_endpoint)?;
    if !matches!(url.path(), "" | "/") {
        return Err(unsafe_endpoint());
    }
    url.set_path("/");
    Ok(url)
}

fn unsafe_endpoint() -> ProviderError {
    ProviderError::InvalidConfiguration("unsafe provider endpoint".to_owned())
}

/// An endpoint the deployment policy approved at construction, together with
/// the policy that must approve it again before every request.
#[derive(Clone)]
pub(super) struct ProviderEndpointGuard {
    policy: Arc<dyn AiProviderEndpointPolicy>,
    kind: AiProviderKindInput,
    endpoint: Url,
}

impl ProviderEndpointGuard {
    /// Applies the transport rule and the deployment policy once, at
    /// construction. Returns `None` when either refuses.
    pub(super) fn authorize(
        policy: Arc<dyn AiProviderEndpointPolicy>,
        kind: AiProviderKindInput,
        endpoint: Url,
    ) -> Option<Self> {
        let guard = Self {
            policy,
            kind,
            endpoint,
        };
        guard.permitted().then_some(guard)
    }

    /// The approved normalized endpoint.
    #[cfg(any(
        feature = "provider-openai-compatible-core",
        feature = "provider-anthropic-core",
        feature = "provider-ollama-core"
    ))]
    pub(super) fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// Re-applies the transport rule and the current deployment policy
    /// immediately before a request.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::EgressDenied`] when either now refuses.
    pub(super) fn recheck(&self) -> Result<(), ProviderError> {
        if self.permitted() {
            Ok(())
        } else {
            Err(ProviderError::EgressDenied)
        }
    }

    fn permitted(&self) -> bool {
        crate::configuration::provider_endpoint_permitted(
            self.policy.as_ref(),
            self.kind,
            &self.endpoint,
        )
    }
}

#[cfg(all(
    test,
    any(
        feature = "provider-openai-compatible-core",
        feature = "provider-chat-completions-core",
        feature = "provider-anthropic-core"
    )
))]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_exact_and_unsafe_forms_are_refused() {
        assert_eq!(
            normalized_provider_endpoint("http://127.0.0.1:11434/v1/chat/completions")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        for unsafe_endpoint in [
            "ftp://host/v1",
            "https:///v1",
            "https://user:pass@host/v1",
            "https://host/v1?x=1",
            "https://host/v1#fragment",
            "https://host/",
            "https://host",
            "not a url",
        ] {
            assert!(
                normalized_provider_endpoint(unsafe_endpoint).is_err(),
                "{unsafe_endpoint}"
            );
        }
    }

    struct Policy {
        plaintext: bool,
    }

    impl AiProviderEndpointPolicy for Policy {
        fn authorize_endpoint(&self, _: AiProviderKindInput, _: &str) -> bool {
            true
        }

        fn authorize_plaintext_endpoint(&self, _: AiProviderKindInput, _: &str) -> bool {
            self.plaintext
        }
    }

    fn guarded(endpoint: &str, plaintext: bool) -> bool {
        ProviderEndpointGuard::authorize(
            Arc::new(Policy { plaintext }),
            AiProviderKindInput::OpenAiCompatible,
            normalized_provider_endpoint(endpoint).expect("endpoint shape"),
        )
        .is_some()
    }

    #[test]
    fn plaintext_needs_a_loopback_host_or_an_explicit_opt_in() {
        for loopback in [
            "http://127.0.0.1:8080/v1",
            "http://127.9.9.9/v1",
            "http://[::1]:8080/v1",
            "http://[::ffff:127.0.0.1]/v1",
            "http://localhost:8080/v1",
            "http://LOCALHOST/v1",
        ] {
            assert!(guarded(loopback, false), "{loopback}");
        }
        for remote in [
            "http://provider.example/v1",
            "http://localhost.example/v1",
            "http://localhost./v1",
            "http://10.0.0.5:8080/v1",
            "http://[2001:db8::5]/v1",
        ] {
            assert!(!guarded(remote, false), "{remote}");
            assert!(guarded(remote, true), "{remote}");
        }
        assert!(guarded("https://provider.example/v1", false));
    }
}
