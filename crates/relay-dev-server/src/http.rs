use std::{convert::Infallible, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::{Stream, StreamExt};
use relay_core::App;
use serde_json::{Value, json};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct HttpState {
    app: Arc<App>,
    stopping: CancellationToken,
}

pub async fn serve(app: Arc<App>, port: u16) -> Result<()> {
    let stopping = CancellationToken::new();
    let state = HttpState {
        app,
        stopping: stopping.clone(),
    };
    let router = Router::new()
        .route("/api/dispatch", post(dispatch))
        .route("/api/events", get(events))
        .route(
            "/api/health",
            get(|| async { Json(json!({"status":"ok"})) }),
        )
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(local_requests_only))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    eprintln!("Relay development API: http://127.0.0.1:{port}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            stopping.cancel();
        })
        .await?;
    Ok(())
}

async fn local_requests_only(request: Request, next: Next) -> Response {
    if !local_headers_allowed(request.headers()) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"This development API only accepts local application requests"})),
        )
            .into_response();
    }
    next.run(request).await
}

fn local_headers_allowed(headers: &HeaderMap) -> bool {
    let host_allowed = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .is_some_and(local_host);
    let origin_allowed = headers.get("origin").is_none_or(|value| {
        matches!(
            value.to_str(),
            Ok("http://localhost:1420"
                | "http://127.0.0.1:1420"
                | "http://127.0.0.1:4317"
                | "http://localhost:4317")
        )
    });
    let same_site = headers
        .get("sec-fetch-site")
        .is_none_or(|value| value != "cross-site");
    host_allowed && origin_allowed && same_site
}

fn local_host(authority: &str) -> bool {
    let Some((host, port)) = authority.split_once(':') else {
        return false;
    };
    matches!(host, "127.0.0.1" | "localhost")
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port != 0)
}

async fn dispatch(
    State(state): State<HttpState>,
    Json(request): Json<relay_core::Request>,
) -> Response {
    match state.app.dispatch(request).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":format!("{error:#}")})),
        )
            .into_response(),
    }
}

async fn events(
    State(state): State<HttpState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.app.subscribe()).map(|event| {
        let value = match event {
            Ok(event) => serde_json::to_value(event).unwrap_or(Value::Null),
            Err(_) => json!({"sessionId":"","kind":"error","data":{"message":"事件订阅中断，请刷新页面恢复历史记录"}}),
        };
        Ok(Event::default().data(value.to_string()))
    });
    let ready = futures_util::stream::once(async { Ok(Event::default().comment("connected")) });
    Sse::new(ready.chain(stream.take_until(state.stopping.cancelled_owned())))
        .keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn local_request_policy_rejects_rebinding_and_cross_site_requests() {
        for (host, origin, site, allowed) in [
            (Some("localhost:1420"), None, None, true),
            (Some("127.0.0.1:58132"), None, None, true),
            (
                Some("127.0.0.1:4317"),
                Some("http://localhost:1420"),
                Some("same-site"),
                true,
            ),
            (
                Some("127.0.0.1:4317"),
                Some("http://127.0.0.1:4317"),
                Some("same-origin"),
                true,
            ),
            (None, None, None, false),
            (Some("attacker.example:4317"), None, None, false),
            (Some("localhost:4317.attacker.example"), None, None, false),
            (Some("localhost:4317@attacker.example"), None, None, false),
            (Some("localhost:+4317"), None, None, false),
            (Some("localhost:65536"), None, None, false),
            (Some("localhost:0"), None, None, false),
            (Some("localhost:"), None, None, false),
            (
                Some("127.0.0.1:4317"),
                Some("https://attacker.example"),
                None,
                false,
            ),
            (Some("127.0.0.1:4317"), Some("null"), None, false),
            (Some("127.0.0.1:4317"), None, Some("cross-site"), false),
        ] {
            let mut headers = HeaderMap::new();
            for (name, value) in [("host", host), ("origin", origin), ("sec-fetch-site", site)] {
                if let Some(value) = value {
                    headers.insert(name, HeaderValue::from_static(value));
                }
            }
            assert_eq!(
                local_headers_allowed(&headers),
                allowed,
                "{host:?} {origin:?} {site:?}"
            );
        }
    }
}
