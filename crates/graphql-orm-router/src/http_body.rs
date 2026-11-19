//! Bounded outbound HTTP retrieval shared by every router fetch: schema
//! and descriptor polling, dynamic registration, JWKS, and the role-scope
//! catalogue. Callers keep their own redacted error messages.

use futures::StreamExt;
use reqwest::{RequestBuilder, Response, StatusCode, header::ETAG};

/// The router's outbound `User-Agent`, tracking the crate version.
pub(crate) const USER_AGENT: &str = concat!("graphql-orm-router/", env!("CARGO_PKG_VERSION"));

/// Transport failures without URLs, header values, or body bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FetchError {
    Send,
    Status(StatusCode),
    TooLarge,
    Read,
}

pub(crate) struct BoundedResponse {
    pub(crate) status: StatusCode,
    pub(crate) bytes: Vec<u8>,
    pub(crate) etag: Option<String>,
}

/// Sends the caller's request and bounds the response by both declared and
/// streamed length. Dynamic registration verifies the response peer before
/// its status or body is used. Only schema polling accepts a 304.
pub(crate) async fn fetch_bounded<E>(
    request: RequestBuilder,
    maximum: usize,
    accept_not_modified: bool,
    verify_peer: impl FnOnce(&Response) -> Result<(), E>,
    map_error: impl Fn(FetchError) -> E,
) -> Result<BoundedResponse, E> {
    let response = request
        .send()
        .await
        .map_err(|_| map_error(FetchError::Send))?;
    verify_peer(&response)?;
    let status = response.status();
    if accept_not_modified && status == StatusCode::NOT_MODIFIED {
        return Ok(BoundedResponse {
            status,
            bytes: Vec::new(),
            etag: None,
        });
    }
    if !status.is_success() {
        return Err(map_error(FetchError::Status(status)));
    }
    let etag = response
        .headers()
        .get(ETAG)
        // An unreadable ETag disables conditional fetches for this response.
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = read_bounded_body(response, maximum)
        .await
        .map_err(map_error)?;
    Ok(BoundedResponse {
        status,
        bytes,
        etag,
    })
}

async fn read_bounded_body(response: Response, maximum: usize) -> Result<Vec<u8>, FetchError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(FetchError::TooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| FetchError::Read)?;
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    /// Serves one response on a loopback listener and returns its URL.
    fn serve_once(response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 1024];
                // The loopback client may close this fixture connection early.
                let _ = stream.read(&mut request);
                // The client may close after refusing the declared length.
                let _ = stream.write_all(response);
            }
        });
        format!("http://{address}/")
    }

    fn request(response: &'static [u8]) -> RequestBuilder {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(serve_once(response))
    }

    fn read(response: &'static [u8], maximum: usize) -> Result<Vec<u8>, FetchError> {
        ntex::rt::System::build()
            .name("bounded-body-test")
            .build(ntex::rt::DefaultRuntime)
            .block_on(async move {
                fetch_bounded(request(response), maximum, false, |_| Ok(()), |error| error)
                    .await
                    .map(|response| response.bytes)
            })
    }

    #[ntex::test]
    async fn conditional_status_and_peer_validation() {
        let not_modified =
            b"HTTP/1.1 304 Not Modified\r\nContent-Length: 100\r\nConnection: close\r\n\r\n";
        let response = fetch_bounded(request(not_modified), 0, true, |_| Ok(()), |error| error)
            .await
            .unwrap();
        assert_eq!(response.status, StatusCode::NOT_MODIFIED);
        assert!(response.bytes.is_empty());
        assert!(matches!(
            fetch_bounded(request(not_modified), 0, false, |_| Ok(()), |error| error).await,
            Err(FetchError::Status(StatusCode::NOT_MODIFIED))
        ));

        let unavailable =
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        assert!(matches!(
            fetch_bounded(
                request(unavailable),
                0,
                false,
                |_| Err("peer refused"),
                |_| "unexpected transport error",
            )
            .await,
            Err("peer refused")
        ));
    }

    #[test]
    fn bounded_body_enforces_declared_and_streamed_limits() {
        assert_eq!(
            read(
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody",
                4
            ),
            Ok(b"body".to_vec())
        );
        assert_eq!(
            read(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nbodyX",
                4
            ),
            Err(FetchError::TooLarge)
        );
        // Without a declared length the streamed bytes are counted.
        assert_eq!(
            read(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
                5
            ),
            Err(FetchError::TooLarge)
        );
        assert_eq!(
            read(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
                6
            ),
            Ok(b"abcdef".to_vec())
        );
        // A truncated body is a read failure, not a short success.
        assert_eq!(
            read(
                b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nabc",
                64
            ),
            Err(FetchError::Read)
        );
        assert!(USER_AGENT.starts_with("graphql-orm-router/"));
        assert!(USER_AGENT.ends_with(env!("CARGO_PKG_VERSION")));
    }
}
