//! Exact endpoint normalization for deployment-configured provider routes.

use url::Url;

use crate::ProviderError;

/// Normalizes one deployment-configured endpoint: `http` or `https`, a host,
/// no user information, query or fragment, and a non-root path of at most
/// 2 KiB. The caller still authorizes the exact result through
/// [`crate::AiProviderEndpointPolicy`].
pub(super) fn normalized_provider_endpoint(value: &str) -> Result<Url, ProviderError> {
    let invalid = || ProviderError::InvalidConfiguration("unsafe provider endpoint".to_owned());
    let mut url = Url::parse(value).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().is_empty()
        || url.path() == "/"
        || url.path().len() > 2_048
    {
        return Err(invalid());
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::normalized_provider_endpoint;

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
}
