//! The loopback HTTP transport both fixture subgraphs serve on.
//!
//! The fixture proves router and ORM behaviour, not HTTP framework behaviour,
//! so the transport is a hand-written listener with no framework dependency.
//! Keeping both subgraphs inside the test process is what makes the ORM's
//! process-global statement counter observable from the assertions.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One decoded `POST /graphql` request.
pub struct GraphqlRequest {
    pub body: String,
    headers: Vec<(String, String)>,
}

impl GraphqlRequest {
    /// Returns a forwarded header value, matched case-insensitively because
    /// the router chooses its own casing on the wire.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Binds an ephemeral loopback port, serves `GET /sdl` and `POST /graphql`,
/// and returns the bound address once the listener is accepting.
pub async fn serve<H, F>(sdl: String, handler: H) -> SocketAddr
where
    H: Fn(GraphqlRequest) -> F + Send + Sync + 'static,
    F: Future<Output = String> + Send + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind a loopback subgraph listener");
    let address = listener.local_addr().expect("loopback listener address");
    let sdl = Arc::new(sdl);
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let sdl = sdl.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                let _ = handle(stream, sdl, handler).await;
            });
        }
    });
    address
}

async fn handle<H, F>(
    mut stream: TcpStream,
    sdl: Arc<String>,
    handler: Arc<H>,
) -> std::io::Result<()>
where
    H: Fn(GraphqlRequest) -> F + Send + Sync + 'static,
    F: Future<Output = String> + Send + 'static,
{
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    let head_end = loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(position) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect::<Vec<_>>();

    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = raw[head_end..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    let body = String::from_utf8_lossy(&body[..content_length.min(body.len())]).into_owned();

    let (status, content_type, payload) = if request_line.starts_with("GET /sdl ") {
        ("200 OK", "text/plain; charset=utf-8", sdl.to_string())
    } else if request_line.starts_with("POST /graphql ") {
        let response = handler(GraphqlRequest { body, headers }).await;
        ("200 OK", "application/json; charset=utf-8", response)
    } else {
        ("404 Not Found", "text/plain; charset=utf-8", String::new())
    };

    stream
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(payload.as_bytes()).await?;
    stream.flush().await
}
