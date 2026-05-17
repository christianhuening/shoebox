//! Reverse proxy for libSQL wire traffic. Forwards authenticated HTTP and
//! WebSocket requests from the mTLS public listener to the embedded `sqld`
//! subprocess bound on loopback.
//!
//! ## Security model
//!
//! The [`ClientIdentity`] extractor is the sole gate: it enforces that the
//! caller presented a valid (non-revoked) mTLS client certificate before any
//! libSQL traffic is forwarded. Once a request reaches `sqld`, it is trusted
//! fully — `sqld` is bound to loopback and only this proxy talks to it.
//!
//! ## Hrana / libSQL paths covered
//!
//! - `/v1/*path` — Hrana v1 endpoints (e.g. `/v1/health`)
//! - `/v2/*path` — Hrana v2 pipeline + streaming (e.g. `/v2/pipeline`,
//!   `/v2/pipeline?baton=...`, WebSocket upgrades on `/v2/streams`)

use std::sync::OnceLock;

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Request, State, WebSocketUpgrade};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;

use crate::http::AppState;
use crate::identity::ClientIdentity;

/// Process-wide hyper client for forwarding HTTP requests to the loopback
/// `sqld` subprocess. Built once on first use so that the legacy hyper client's
/// connection pool actually does its job — rebuilding per request would force a
/// fresh TCP handshake on every `/v2/pipeline` POST and defeat keep-alive.
static UPSTREAM_HTTP_CLIENT: OnceLock<HyperClient<HttpConnector, Body>> = OnceLock::new();

fn upstream_http_client() -> &'static HyperClient<HttpConnector, Body> {
    UPSTREAM_HTTP_CLIENT.get_or_init(|| HyperClient::builder(TokioExecutor::new()).build_http())
}

/// Build the `/v1/*` + `/v2/*` catch-all routes that forward to `sqld`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/*path", any(forward_http))
        .route("/v2/*path", any(forward_http))
}

/// Forward either a regular HTTP request or a WebSocket upgrade to the
/// embedded `sqld` on loopback.
///
/// The `ClientIdentity` extractor runs before this handler is entered, so a
/// missing or revoked client cert will already have been rejected with a 401.
async fn forward_http(
    State(state): State<AppState>,
    _identity: ClientIdentity,
    websocket_upgrade: Option<WebSocketUpgrade>,
    mut req: Request,
) -> Response {
    if let Some(websocket_upgrade) = websocket_upgrade {
        let upstream_url = build_upstream_url(&state.sqld_url, req.uri(), true);
        return websocket_upgrade.on_upgrade(move |client_socket| async move {
            if let Err(forward_error) = forward_ws(client_socket, upstream_url).await {
                tracing::warn!(event = "proxy.ws.error", error = %forward_error);
            }
        });
    }

    let upstream_uri: Uri = match build_upstream_url(&state.sqld_url, req.uri(), false).parse() {
        Ok(parsed_uri) => parsed_uri,
        Err(parse_error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("bad upstream URI: {parse_error}"),
            )
                .into_response();
        }
    };

    *req.uri_mut() = upstream_uri;
    let request_headers = req.headers_mut();
    // Strip hop-by-hop headers (RFC 7230 §6.1). The upstream connection
    // is a fresh hyper connection; reusing these from the client request
    // would corrupt the proxied exchange.
    request_headers.remove(header::HOST);
    request_headers.remove(header::CONNECTION);
    request_headers.remove("keep-alive");
    request_headers.remove("proxy-connection");
    request_headers.remove(header::TRANSFER_ENCODING);
    request_headers.remove(header::UPGRADE);

    match upstream_http_client().request(req).await {
        Ok(upstream_response) => upstream_response.into_response(),
        Err(forward_error) => {
            tracing::warn!(event = "proxy.http.error", error = %forward_error);
            (
                StatusCode::BAD_GATEWAY,
                format!("upstream sqld unreachable: {forward_error}"),
            )
                .into_response()
        }
    }
}

/// Pump messages bidirectionally between the downstream axum WebSocket and the
/// upstream tungstenite WebSocket until either side closes.
async fn forward_ws(
    mut client_socket: axum::extract::ws::WebSocket,
    upstream_url: String,
) -> Result<()> {
    use axum::extract::ws::Message as AxumMessage;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

    let (upstream_socket, _upgrade_response) =
        tokio_tungstenite::connect_async(upstream_url).await?;
    let (mut upstream_tx, mut upstream_rx) = upstream_socket.split();

    // Run the bidirectional pump in an inner async block so we can perform
    // unconditional best-effort graceful close on both sides afterwards,
    // regardless of whether the pump exited cleanly or via an error.
    let pump_result: Result<()> = async {
        loop {
            tokio::select! {
                client_to_upstream = client_socket.recv() => {
                    let Some(client_msg_result) = client_to_upstream else { break };
                    let client_msg = client_msg_result?;
                    let upstream_msg = match client_msg {
                        AxumMessage::Text(text) => TungsteniteMessage::Text(text),
                        AxumMessage::Binary(bytes) => TungsteniteMessage::Binary(bytes),
                        AxumMessage::Ping(payload) => TungsteniteMessage::Ping(payload),
                        AxumMessage::Pong(payload) => TungsteniteMessage::Pong(payload),
                        AxumMessage::Close(_) => break,
                    };
                    upstream_tx.send(upstream_msg).await?;
                }
                upstream_to_client = upstream_rx.next() => {
                    let Some(upstream_msg_result) = upstream_to_client else { break };
                    let upstream_msg = upstream_msg_result?;
                    let downstream_msg = match upstream_msg {
                        TungsteniteMessage::Text(text) => AxumMessage::Text(text),
                        TungsteniteMessage::Binary(bytes) => AxumMessage::Binary(bytes),
                        TungsteniteMessage::Ping(payload) => AxumMessage::Ping(payload),
                        TungsteniteMessage::Pong(payload) => AxumMessage::Pong(payload),
                        TungsteniteMessage::Close(_) => break,
                        // Raw frames are an internal tungstenite detail and not
                        // emitted to library users; the maintainers recommend
                        // ignoring them. See snapview/tungstenite-rs#268.
                        TungsteniteMessage::Frame(_) => continue,
                    };
                    client_socket.send(downstream_msg).await?;
                }
                else => break,
            }
        }
        Ok(())
    }
    .await;

    // Best-effort graceful close in both directions so `sqld` and the client
    // both see a proper WS Close frame rather than an abrupt TCP RST. Errors
    // here are ignored — the peer may already be gone, and the original
    // `pump_result` is the meaningful return value for the caller's logging.
    let _ = upstream_tx.send(TungsteniteMessage::Close(None)).await;
    let _ = upstream_tx.close().await;
    let _ = client_socket.send(AxumMessage::Close(None)).await;
    let _ = client_socket.close().await;

    pump_result
}

/// Build an upstream URL for `sqld`, swapping the scheme to `ws[s]://` for
/// WebSocket upgrades and preserving the original path + query.
fn build_upstream_url(sqld_base: &str, req_uri: &Uri, ws: bool) -> String {
    let base = if ws {
        sqld_base
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1)
    } else {
        sqld_base.to_string()
    };
    let path_and_query = req_uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);
    format!("{base}{path_and_query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_upstream_url_preserves_path_and_query() {
        let base = "http://127.0.0.1:53421";
        let request_uri: Uri = "/v2/pipeline?baton=abc".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, false);
        assert_eq!(upstream, "http://127.0.0.1:53421/v2/pipeline?baton=abc");
    }

    #[test]
    fn build_upstream_url_swaps_http_to_ws_when_upgrading() {
        let base = "http://127.0.0.1:53421";
        let request_uri: Uri = "/v2/streams".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, true);
        assert_eq!(upstream, "ws://127.0.0.1:53421/v2/streams");
    }

    #[test]
    fn build_upstream_url_swaps_https_to_wss_when_upgrading() {
        let base = "https://127.0.0.1:53421";
        let request_uri: Uri = "/v2/streams".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, true);
        assert_eq!(upstream, "wss://127.0.0.1:53421/v2/streams");
    }

    #[test]
    fn build_upstream_url_defaults_path_to_root_when_uri_has_none() {
        let base = "http://127.0.0.1:53421";
        // A bare authority URI has no path-and-query.
        let request_uri: Uri = Uri::default();
        let upstream = build_upstream_url(base, &request_uri, false);
        assert_eq!(upstream, "http://127.0.0.1:53421/");
    }
}
