use std::{convert::Infallible, sync::Arc};

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
use serde_json::{Value, json};
use tandem_core::{App, ErrorCode, ErrorResponse};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct HttpState {
    app: Arc<App>,
    stopping: CancellationToken,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("Cannot bind the local development API: {0}")]
    Bind(#[source] std::io::Error),
    #[error("The local development API stopped unexpectedly: {0}")]
    Serve(#[source] std::io::Error),
}

pub async fn serve(app: Arc<App>, port: u16) -> Result<(), HttpError> {
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
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(HttpError::Bind)?;
    eprintln!("Tandem development API: http://127.0.0.1:{port}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            stopping.cancel();
        })
        .await
        .map_err(HttpError::Serve)?;
    Ok(())
}

async fn local_requests_only(request: Request, next: Next) -> Response {
    if !local_headers_allowed(request.headers()) {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                code: ErrorCode::InvalidInput,
                error: "This development API only accepts local application requests".into(),
            }),
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
    Json(request): Json<tandem_core::Request>,
) -> Response {
    match state.app.dispatch(request).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            (error_status(error.code()), Json(ErrorResponse::from(error))).into_response()
        }
    }
}

fn error_status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn events(
    State(state): State<HttpState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.app.subscribe()).map(|event| {
        let value = match event {
            Ok(event) => serde_json::to_value(event).unwrap_or(Value::Null),
            Err(_) => json!({"sessionId":"","kind":"error","data":{"message":"Event subscription was interrupted. Refresh the page to restore conversation history."}}),
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

    #[tokio::test]
    async fn service_failures_determine_http_status_by_type()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!(
            "tandem-http-errors-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
        ));
        let app = App::open(tandem_core::AppOptions::local(directory.clone()))?;
        for (request, expected) in [
            (
                tandem_core::Request::Messages {
                    session_id: "missing".into(),
                },
                StatusCode::NOT_FOUND,
            ),
            (
                tandem_core::Request::CreateSession {
                    project_id: None,
                    provider_id: "missing".into(),
                    model_id: "model".into(),
                    permission_mode: tandem_core::model::PermissionMode::AllowAll,
                },
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let response = dispatch(
                State(HttpState {
                    app: Arc::clone(&app),
                    stopping: CancellationToken::new(),
                }),
                Json(request),
            )
            .await;
            assert_eq!(response.status(), expected);
        }
        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

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
