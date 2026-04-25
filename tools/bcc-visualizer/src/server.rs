use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::event_bus::{EventBus, WsEnvelope};
use crate::scenarios::run_scenario;

static INDEX_HTML: &str = include_str!("assets/index.html");

/// Starts the visualizer HTTP + WebSocket server on `addr`.
///
/// A single [`TcpListener`] accepts all connections.  Each connection is peeked
/// to detect a WebSocket upgrade handshake; non-WS connections are served as
/// plain HTTP.
pub async fn run_server(
    addr:   std::net::SocketAddr,
    bus:    Arc<EventBus>,
    ports:  Vec<String>,
    cancel: CancellationToken,
) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l)  => l,
        Err(e) => { tracing::error!(%addr, err=%e, "server: failed to bind"); return; }
    };

    info!(%addr, "visualizer listening — open http://{addr} in your browser");
    println!("Open http://{addr} in your browser");

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            result = listener.accept() => match result {
                Ok((stream, _peer)) => {
                    let bus   = Arc::clone(&bus);
                    let ports = ports.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, bus, ports).await;
                    });
                }
                Err(e) => tracing::warn!(err=%e, "server: accept error"),
            }
        }
    }

    info!("visualizer stopped");
}

/// Peeks at the first bytes of `stream` to decide whether this is a WebSocket
/// upgrade or a plain HTTP request, then dispatches accordingly.
async fn handle_connection(stream: TcpStream, bus: Arc<EventBus>, ports: Vec<String>) {
    const PEEK_BUF: usize = 4096;
    let mut peek = [0u8; PEEK_BUF];
    let n = match stream.peek(&mut peek).await {
        Ok(n)  => n,
        Err(_) => return,
    };
    let preview = &peek[..n];

    // "upgrade: websocket" = 18 bytes — case-insensitive (browsers may send "Upgrade:").
    let is_ws = preview
        .windows(18)
        .any(|w| w.eq_ignore_ascii_case(b"upgrade: websocket"));

    if is_ws {
        handle_ws(stream, bus).await;
    } else {
        handle_http(stream, preview, bus, ports).await;
    }
}

// ── WebSocket ─────────────────────────────────────────────────────────────────

async fn handle_ws(stream: TcpStream, bus: Arc<EventBus>) {
    let ws = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => { debug!(err=%e, "WS: handshake failed"); return; }
    };

    debug!("WS: client connected");
    let (mut sender, mut receiver) = ws.split();
    let mut sub: broadcast::Receiver<WsEnvelope> = bus.subscribe();

    // Replay recent events
    for env in bus.recent_sync() {
        if let Ok(json) = serde_json::to_string(&env) {
            if sender.send(Message::Text(json.into())).await.is_err() {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            msg = sub.recv() => match msg {
                Ok(env) => {
                    let Ok(json) = serde_json::to_string(&env) else { continue };
                    if sender.send(Message::Text(json.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let notice = format!(r#"{{"lagged":{n}}}"#);
                    let _ = sender.send(Message::Text(notice.into())).await;
                }
                Err(_) => break,
            },
            msg = receiver.next() => {
                if msg.is_none() { break; }
            }
        }
    }

    debug!("WS: client disconnected");
}

// ── HTTP ──────────────────────────────────────────────────────────────────────

async fn handle_http(
    mut stream: TcpStream,
    preview:    &[u8],
    bus:        Arc<EventBus>,
    ports:      Vec<String>,
) {
    let header_str = String::from_utf8_lossy(preview);
    let first_line = header_str.lines().next().unwrap_or("");

    let response = route(first_line, &bus, &ports).await;
    let _ = stream.write_all(&response).await;
}

async fn route(first_line: &str, bus: &Arc<EventBus>, ports: &[String]) -> Vec<u8> {
    // GET /
    if first_line.starts_with("GET / ") || first_line == "GET / HTTP/1.1" {
        return http_200("text/html; charset=utf-8", INDEX_HTML.as_bytes());
    }

    // GET /api/events
    if first_line.contains("/api/events") {
        let body = serde_json::to_vec(&bus.recent_sync()).unwrap_or_default();
        return http_200("application/json", &body);
    }

    // POST /api/scenario/:name
    if first_line.starts_with("POST /api/scenario/") {
        let name = extract_path_tail(first_line, "/api/scenario/");
        if !name.is_empty() {
            let bus     = Arc::clone(bus);
            let ports   = ports.to_vec();
            let name_s  = name.to_string();
            let name_r  = name.to_string();
            tokio::spawn(async move { run_scenario(&name_s, &ports, bus).await; });
            let body = format!(r#"{{"status":"started","scenario":"{}"}}"#, name_r);
            return http_200("application/json", body.as_bytes());
        }
    }

    // OPTIONS (CORS preflight from browser)
    if first_line.starts_with("OPTIONS") {
        return b"HTTP/1.1 204 No Content\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Access-Control-Allow-Methods: GET, POST\r\n\
                 Connection: close\r\n\r\n"
            .to_vec();
    }

    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}

fn http_200(content_type: &str, body: &[u8]) -> Vec<u8> {
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n",
        len = body.len(),
    );
    let mut resp = header.into_bytes();
    resp.extend_from_slice(body);
    resp
}

fn extract_path_tail<'a>(first_line: &'a str, prefix: &str) -> &'a str {
    // "POST /api/scenario/my_name HTTP/1.1" → "my_name"
    let after_method = first_line.splitn(2, ' ').nth(1).unwrap_or("");
    let path = after_method.splitn(2, ' ').next().unwrap_or("");
    path.strip_prefix(prefix).unwrap_or("").trim_end_matches('/')
}
