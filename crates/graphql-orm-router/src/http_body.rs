//! Bounded outbound HTTP response bodies shared by every router fetch: schema
//! and descriptor polling, dynamic registration, JWKS, and the role-scope
//! catalogue. Callers keep their own redacted error messages.

use futures::StreamExt;

/// The router's outbound `User-Agent`, tracking the crate version.
pub(crate) const USER_AGENT: &str = concat!("graphql-orm-router/", env!("CARGO_PKG_VERSION"));

/// Why a bounded body could not be read. Neither variant carries the URL,
/// header values, or body bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundedBodyError {
    /// The declared `Content-Length` or the bytes received exceed the limit.
    TooLarge,
    /// The response stream failed before it completed.
    Read,
}

/// Reads at most `maximum` body bytes. A declared `Content-Length` above the
/// limit is refused before any byte is read, and a streamed body is refused
/// as soon as it would cross the limit.
pub(crate) async fn read_bounded_body(
    response: reqwest::Response,
    maximum: usize,
) -> Result<Vec<u8>, BoundedBodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(BoundedBodyError::TooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| BoundedBodyError::Read)?;
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(BoundedBodyError::TooLarge);
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
                let _ = stream.read(&mut request);
                // The client may close after refusing the declared length.
                let _ = stream.write_all(response);
            }
        });
        format!("http://{address}/")
    }

    fn read(response: &'static [u8], maximum: usize) -> Result<Vec<u8>, BoundedBodyError> {
        let url = serve_once(response);
        ntex::rt::System::build()
            .name("bounded-body-test")
            .build(ntex::rt::DefaultRuntime)
            .block_on(async move {
                let response = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap()
                    .get(url)
                    .send()
                    .await
                    .unwrap();
                read_bounded_body(response, maximum).await
            })
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
            Err(BoundedBodyError::TooLarge)
        );
        // Without a declared length the streamed bytes are counted.
        assert_eq!(
            read(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n",
                5
            ),
            Err(BoundedBodyError::TooLarge)
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
            Err(BoundedBodyError::Read)
        );
        assert!(USER_AGENT.starts_with("graphql-orm-router/"));
        assert!(USER_AGENT.ends_with(env!("CARGO_PKG_VERSION")));
    }
}
