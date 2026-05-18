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
use axum::extract::{FromRequestParts, Request, State, WebSocketUpgrade};
use axum::http::request::Parts;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;

use crate::http::AppState;
use crate::identity::ClientIdentity;

/// axum 0.8 dropped the blanket `Option<T>` extractor impl for things
/// that didn't also implement `OptionalFromRequestParts` (which
/// `WebSocketUpgrade` doesn't). This thin wrapper restores the
/// "extract if the request is a WS upgrade, otherwise None" behaviour
/// we relied on under axum 0.7.
struct OptionalWebSocketUpgrade(Option<WebSocketUpgrade>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalWebSocketUpgrade {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(OptionalWebSocketUpgrade(
            WebSocketUpgrade::from_request_parts(parts, state)
                .await
                .ok(),
        ))
    }
}

/// Process-wide hyper clients for forwarding requests to the loopback `sqld`
/// subprocess. One client for HTTP/1.1 Hrana traffic, one for HTTP/2 gRPC
/// replication traffic. Built once on first use so that the legacy hyper
/// client's connection pool actually does its job — rebuilding per request
/// would force a fresh TCP handshake on every `/v2/pipeline` POST or gRPC
/// `LogEntries` call and defeat keep-alive.
static UPSTREAM_HTTP_CLIENT: OnceLock<HyperClient<HttpConnector, Body>> = OnceLock::new();
static UPSTREAM_GRPC_CLIENT: OnceLock<HyperClient<HttpConnector, Body>> = OnceLock::new();

fn upstream_http_client() -> &'static HyperClient<HttpConnector, Body> {
    UPSTREAM_HTTP_CLIENT.get_or_init(|| HyperClient::builder(TokioExecutor::new()).build_http())
}

/// HTTP/2-only client for forwarding gRPC traffic to sqld's `--grpc-listen-addr`
/// loopback port. `http2_only(true)` enables h2-prior-knowledge mode — the
/// connection sends the HTTP/2 preface immediately without HTTP/1.1 upgrade,
/// which is what sqld's gRPC listener expects on a plaintext port.
fn upstream_grpc_client() -> &'static HyperClient<HttpConnector, Body> {
    UPSTREAM_GRPC_CLIENT.get_or_init(|| {
        HyperClient::builder(TokioExecutor::new())
            .http2_only(true)
            .build_http()
    })
}

/// Returns true if the request is gRPC, i.e. has `Content-Type: application/grpc`
/// (RFC 9113-ish; gRPC over HTTP/2). Hrana queries carry `application/json`.
fn is_grpc_request(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with("application/grpc"))
}

/// Build the `/v1/*` + `/v2/*` catch-all routes that forward to `sqld`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/{*path}", any(forward_http))
        .route("/v2/{*path}", any(forward_http))
}

/// Forward either a regular HTTP request or a WebSocket upgrade to the
/// embedded `sqld` on loopback.
///
/// The `ClientIdentity` extractor runs before this handler is entered, so a
/// missing or revoked client cert will already have been rejected with a 401.
async fn forward_http(
    State(state): State<AppState>,
    _identity: ClientIdentity,
    OptionalWebSocketUpgrade(websocket_upgrade): OptionalWebSocketUpgrade,
    mut req: Request,
) -> Response {
    if let Some(websocket_upgrade) = websocket_upgrade {
        let upstream_url = build_upstream_url(&state.sqld_url, req.uri(), true, false);
        return websocket_upgrade.on_upgrade(move |client_socket| async move {
            if let Err(forward_error) = forward_ws(client_socket, upstream_url).await {
                tracing::warn!(event = "proxy.ws.error", error = %forward_error);
            }
        });
    }

    // Branch by Content-Type. gRPC requests get forwarded over HTTP/2 to
    // sqld's --grpc-listen-addr port with the /v1 or /v2 path prefix
    // stripped (tonic preserves the path of the sync URL the client gave,
    // so requests land as `/v1/wal_log.ReplicationLog/Hello` here — sqld's
    // gRPC server expects `/wal_log.ReplicationLog/Hello`). Non-gRPC
    // requests continue down the existing HTTP/1.1 path to sqld's
    // --http-listen-addr port, unchanged.
    let is_grpc = is_grpc_request(&req);
    let (upstream_base, client, strip_proxy_prefix) = if is_grpc {
        (
            state.sqld_grpc_url.as_str(),
            upstream_grpc_client(),
            true,
        )
    } else {
        (state.sqld_url.as_str(), upstream_http_client(), false)
    };

    let upstream_uri: Uri = match build_upstream_url(
        upstream_base,
        req.uri(),
        false,
        strip_proxy_prefix,
    )
    .parse()
    {
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
    // would corrupt the proxied exchange. **Crucially**, `TE: trailers`
    // is hop-by-hop on HTTP/1 but is the contract that tells the upstream
    // gRPC server it may emit trailers. Preserve it for gRPC requests so
    // sqld actually sends `grpc-status` back as a trailer.
    request_headers.remove(header::HOST);
    request_headers.remove(header::CONNECTION);
    request_headers.remove("keep-alive");
    request_headers.remove("proxy-connection");
    request_headers.remove(header::TRANSFER_ENCODING);
    request_headers.remove(header::UPGRADE);
    if !is_grpc {
        request_headers.remove("te");
    }

    if is_grpc {
        // libsql clients (0.6 through at least 0.9.30) send their bearer
        // token in `x-authorization`. sqld's WriteProxy gRPC service
        // ignores that header and instead expects `x-proxy-authorization`
        // to contain a JSON-serialized `libsql_server::auth::Authenticated`
        // enum (sqld's intra-cluster auth handoff format — designed for
        // replica nodes forwarding an already-authenticated client
        // identity to the primary). When the header is missing, sqld
        // returns `"x-proxy-authorization not set"`; when it's any non-JSON
        // string (e.g. `Bearer <token>`), sqld's `serde_json::from_str
        // (...).unwrap()` panics inside the request handler and the gRPC
        // stream dies with a cryptic `"Invalid header bit X expected 0 or
        // 1"` h2 error.
        //
        // The shoebox-server proxy already authenticated the caller via
        // mTLS (the `ClientIdentity` extractor ran before this handler
        // was entered) and `sqld` is bound to loopback inside the same
        // process tree, so vouching for the request with `FullAccess`
        // (sqld's no-permission-check variant) is honest: we've already
        // verified the client cert chains to our internal CA.
        //
        // Tracked upstream as tursodatabase/go-libsql#42 (OPEN since 2024)
        // and #52; unlikely to be fixed since Turso has deprecated this
        // codepath in favour of "Turso Sync".
        request_headers.insert(
            "x-proxy-authorization",
            axum::http::HeaderValue::from_static("\"FullAccess\""),
        );
    }

    match client.request(req).await {
        Ok(upstream_response) => upstream_response.into_response(),
        Err(forward_error) => {
            tracing::warn!(
                event = "proxy.http.error",
                grpc = is_grpc,
                error = %forward_error
            );
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
                        // axum 0.8 and tungstenite use distinct (re-exported)
                        // Utf8Bytes / Bytes types — convert by way of String /
                        // raw bytes rather than relying on identical types.
                        AxumMessage::Text(text) => {
                            TungsteniteMessage::Text(text.as_str().into())
                        }
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
                        TungsteniteMessage::Text(text) => {
                            AxumMessage::Text(text.as_str().into())
                        }
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
/// WebSocket upgrades and preserving the original path + query. When
/// `strip_proxy_prefix` is true (gRPC forwarding), the leading `/v1` or
/// `/v2` prefix is stripped from the path so that gRPC method paths
/// reach sqld in the form sqld registers them — `/wal_log.ReplicationLog/Hello`
/// rather than `/v1/wal_log.ReplicationLog/Hello`.
fn build_upstream_url(
    sqld_base: &str,
    req_uri: &Uri,
    ws: bool,
    strip_proxy_prefix: bool,
) -> String {
    let base = if ws {
        sqld_base
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1)
    } else {
        sqld_base.to_string()
    };
    let raw_path_and_query = req_uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);
    let path_and_query = if strip_proxy_prefix {
        strip_v1_or_v2_prefix(raw_path_and_query)
    } else {
        raw_path_and_query.to_string()
    };
    format!("{base}{path_and_query}")
}

/// Strip a leading `/v1` or `/v2` segment from a path+query string.
/// `/v1/wal_log.ReplicationLog/Hello?baton=x` → `/wal_log.ReplicationLog/Hello?baton=x`.
/// A bare `/v1` or `/v1/` becomes `/`.
fn strip_v1_or_v2_prefix(path_and_query: &str) -> String {
    for prefix in ["/v1/", "/v2/"] {
        if let Some(rest) = path_and_query.strip_prefix(prefix) {
            return format!("/{rest}");
        }
    }
    for bare in ["/v1", "/v2"] {
        if path_and_query == bare {
            return "/".to_string();
        }
    }
    path_and_query.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_upstream_url_preserves_path_and_query() {
        let base = "http://127.0.0.1:53421";
        let request_uri: Uri = "/v2/pipeline?baton=abc".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, false, false);
        assert_eq!(upstream, "http://127.0.0.1:53421/v2/pipeline?baton=abc");
    }

    #[test]
    fn build_upstream_url_swaps_http_to_ws_when_upgrading() {
        let base = "http://127.0.0.1:53421";
        let request_uri: Uri = "/v2/streams".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, true, false);
        assert_eq!(upstream, "ws://127.0.0.1:53421/v2/streams");
    }

    #[test]
    fn build_upstream_url_swaps_https_to_wss_when_upgrading() {
        let base = "https://127.0.0.1:53421";
        let request_uri: Uri = "/v2/streams".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, true, false);
        assert_eq!(upstream, "wss://127.0.0.1:53421/v2/streams");
    }

    #[test]
    fn build_upstream_url_defaults_path_to_root_when_uri_has_none() {
        let base = "http://127.0.0.1:53421";
        // A bare authority URI has no path-and-query.
        let request_uri: Uri = Uri::default();
        let upstream = build_upstream_url(base, &request_uri, false, false);
        assert_eq!(upstream, "http://127.0.0.1:53421/");
    }

    #[test]
    fn build_upstream_url_strips_v1_prefix_for_grpc() {
        let base = "http://127.0.0.1:53422";
        let request_uri: Uri = "/v1/wal_log.ReplicationLog/Hello".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, false, true);
        assert_eq!(
            upstream,
            "http://127.0.0.1:53422/wal_log.ReplicationLog/Hello"
        );
    }

    #[test]
    fn build_upstream_url_strips_v2_prefix_for_grpc() {
        let base = "http://127.0.0.1:53422";
        let request_uri: Uri = "/v2/wal_log.ReplicationLog/LogEntries?token=x"
            .parse()
            .unwrap();
        let upstream = build_upstream_url(base, &request_uri, false, true);
        assert_eq!(
            upstream,
            "http://127.0.0.1:53422/wal_log.ReplicationLog/LogEntries?token=x"
        );
    }

    #[test]
    fn build_upstream_url_passes_through_when_no_prefix_to_strip() {
        let base = "http://127.0.0.1:53422";
        let request_uri: Uri = "/wal_log.ReplicationLog/Hello".parse().unwrap();
        let upstream = build_upstream_url(base, &request_uri, false, true);
        assert_eq!(
            upstream,
            "http://127.0.0.1:53422/wal_log.ReplicationLog/Hello"
        );
    }

    #[test]
    fn strip_prefix_handles_bare_v1() {
        assert_eq!(strip_v1_or_v2_prefix("/v1"), "/");
        assert_eq!(strip_v1_or_v2_prefix("/v2"), "/");
        assert_eq!(strip_v1_or_v2_prefix("/v1/"), "/");
        assert_eq!(strip_v1_or_v2_prefix("/other/path"), "/other/path");
    }
}
