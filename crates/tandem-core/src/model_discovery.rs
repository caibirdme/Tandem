//! Explicit model discovery through the Anthropic list-models wire protocol.

use std::{collections::HashSet, time::Duration};

use reqwest::{Client, header::HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    ErrorCode, EventDecodeError,
    model::{ValidationError, validate_endpoint, validate_token_count},
};

const PAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_MODELS: usize = 10_000;
const MAX_PAGES: usize = 100;

/// Discovery errors never retain response bodies, request URLs, or credentials.
#[derive(Debug, thiserror::Error)]
pub enum ModelDiscoveryError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("The API key contains invalid HTTP header characters.")]
    InvalidApiKey,
    #[error("Could not connect to the model-list endpoint. Check the URL and network connection.")]
    Transport(#[source] reqwest::Error),
    #[error("Model discovery timed out. Check the endpoint and try again.")]
    Timeout,
    #[error("Model discovery was rejected (HTTP {status}). Check the API key and model-list path.")]
    HttpStatus { status: u16 },
    #[error("The model-list response is not valid Anthropic list-models JSON.")]
    Decode(#[source] EventDecodeError),
    #[error("The model-list response contains an invalid model ID or display name.")]
    InvalidModel,
    #[error("The model-list response contains invalid token limits.")]
    InvalidMetadata(#[source] ValidationError),
    #[error(
        "The model-list response has invalid or repeated pagination. Check the endpoint protocol."
    )]
    InvalidPagination,
    #[error("The model list exceeds the supported size. Use a more specific model-list endpoint.")]
    TooLarge,
}

impl ModelDiscoveryError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) | Self::InvalidApiKey => ErrorCode::InvalidInput,
            _ => ErrorCode::Unavailable,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiscoveredModel {
    id: String,
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supported_reasoning_efforts: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct WireModel {
    id: String,
    display_name: String,
    max_input_tokens: Option<i64>,
    max_tokens: Option<i64>,
    capabilities: Option<WireCapabilities>,
}

#[derive(Deserialize)]
struct WireCapabilities {
    effort: Option<WireEffort>,
}

#[derive(Deserialize)]
struct WireSupport {
    supported: bool,
}

#[derive(Deserialize)]
struct WireEffort {
    supported: bool,
    low: Option<WireSupport>,
    medium: Option<WireSupport>,
    high: Option<WireSupport>,
    xhigh: Option<WireSupport>,
    max: Option<WireSupport>,
}

impl WireEffort {
    fn supported_levels(self) -> Vec<String> {
        if !self.supported {
            return Vec::new();
        }
        [
            ("low", self.low),
            ("medium", self.medium),
            ("high", self.high),
            ("xhigh", self.xhigh),
            ("max", self.max),
        ]
        .into_iter()
        .filter(|(_, capability)| {
            capability
                .as_ref()
                .is_some_and(|capability| capability.supported)
        })
        .map(|(level, _)| level.to_owned())
        .collect()
    }
}

impl WireModel {
    fn into_discovered(self) -> Result<DiscoveredModel, ModelDiscoveryError> {
        if self.id.trim() != self.id || !valid_label(&self.id) || !valid_label(&self.display_name) {
            return Err(ModelDiscoveryError::InvalidModel);
        }
        // Catalog fields advertise independent ceilings; a chosen output budget
        // is validated against the context only when the provider is saved.
        for count in [self.max_input_tokens, self.max_tokens]
            .into_iter()
            .flatten()
        {
            validate_token_count(count).map_err(ModelDiscoveryError::InvalidMetadata)?;
        }
        Ok(DiscoveredModel {
            id: self.id,
            display_name: self.display_name,
            context_window: self.max_input_tokens,
            max_output_tokens: self.max_tokens,
            supported_reasoning_efforts: self
                .capabilities
                .and_then(|capabilities| capabilities.effort)
                .map(WireEffort::supported_levels),
        })
    }
}

#[derive(Deserialize)]
struct Page {
    data: Vec<WireModel>,
    has_more: bool,
    last_id: Option<String>,
}

pub(crate) fn validate_path(path: &str) -> Result<(), ValidationError> {
    let path_part = path.split('?').next().unwrap_or_default();
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(|c| c.is_control() || c.is_whitespace())
        || path.contains(['\\', '#'])
        || path_part.starts_with("//")
        || path_part.contains(':')
        || path_part.trim_matches('/').is_empty()
        || path_part.split('/').any(|segment| {
            let decoded = segment.to_ascii_lowercase().replace("%2e", ".");
            matches!(decoded.as_str(), "." | "..")
        })
    {
        return Err(ValidationError::InvalidModelListPath);
    }
    Ok(())
}

fn endpoint(base: &str, path: &str) -> Result<Url, ValidationError> {
    validate_endpoint(base)?;
    validate_path(path)?;
    // Append rather than URL-join: a leading slash must not discard gateway prefixes.
    Url::parse(&format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
    .map_err(|source| ValidationError::InvalidEndpoint { source })
}

fn transport(error: reqwest::Error) -> ModelDiscoveryError {
    if error.is_timeout() {
        ModelDiscoveryError::Timeout
    } else {
        ModelDiscoveryError::Transport(error.without_url())
    }
}

pub(crate) async fn list_models(
    base: &str,
    path: &str,
    api_key: Option<&str>,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let endpoint = endpoint(base.trim(), path.trim())?;
    let key = api_key
        .filter(|key| !key.trim().is_empty())
        .map(|key| {
            let mut header = HeaderValue::from_str(key.trim())
                .map_err(|_| ModelDiscoveryError::InvalidApiKey)?;
            header.set_sensitive(true);
            Ok::<_, ModelDiscoveryError>(header)
        })
        .transpose()?;
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(transport)?;
    tokio::time::timeout(Duration::from_secs(60), fetch_pages(&client, endpoint, key))
        .await
        .map_err(|_| ModelDiscoveryError::Timeout)?
}

async fn fetch_pages(
    client: &Client,
    endpoint: Url,
    key: Option<HeaderValue>,
) -> Result<Vec<DiscoveredModel>, ModelDiscoveryError> {
    let query: Vec<(String, String)> = endpoint
        .query_pairs()
        .filter(|(name, _)| !matches!(name.as_ref(), "limit" | "after_id" | "before_id"))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let mut cursor: Option<String> = None;
    let mut cursors = HashSet::new();
    let mut ids = HashSet::new();
    let mut models = Vec::new();
    for _ in 0..MAX_PAGES {
        let mut url = endpoint.clone();
        url.set_query(None);
        {
            let mut parameters = url.query_pairs_mut();
            parameters.extend_pairs(&query).append_pair("limit", "1000");
            if let Some(cursor) = &cursor {
                parameters.append_pair("after_id", cursor);
            }
        }
        let mut request = client.get(url).header("anthropic-version", "2023-06-01");
        if let Some(key) = &key {
            request = request.header("x-api-key", key);
        }
        let response = request.send().await.map_err(transport)?;
        if !response.status().is_success() {
            return Err(ModelDiscoveryError::HttpStatus {
                status: response.status().as_u16(),
            });
        }
        let page = read_page(response).await?;
        if page.has_more
            && (page.data.is_empty()
                || page.last_id.as_ref() != page.data.last().map(|model| &model.id))
        {
            return Err(ModelDiscoveryError::InvalidPagination);
        }
        for model in page.data {
            let model = model.into_discovered()?;
            if ids.insert(model.id.clone()) {
                models.push(model);
                if models.len() > MAX_MODELS {
                    return Err(ModelDiscoveryError::TooLarge);
                }
            }
        }
        if !page.has_more {
            return Ok(models);
        }
        let next = page.last_id.ok_or(ModelDiscoveryError::InvalidPagination)?;
        if !cursors.insert(next.clone()) {
            return Err(ModelDiscoveryError::InvalidPagination);
        }
        cursor = Some(next);
    }
    Err(ModelDiscoveryError::TooLarge)
}

fn valid_label(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 256 && !value.chars().any(char::is_control)
}

async fn read_page(mut response: reqwest::Response) -> Result<Page, ModelDiscoveryError> {
    if response
        .content_length()
        .is_some_and(|size| size > PAGE_BYTES as u64)
    {
        return Err(ModelDiscoveryError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if chunk.len() > PAGE_BYTES - bytes.len() {
            return Err(ModelDiscoveryError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|error| ModelDiscoveryError::Decode(error.into()))
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;
    use crate::{App, AppOptions, model::Provider};

    #[test]
    fn paths_append_without_changing_origin() -> Result<()> {
        for (base, path, expected) in [
            (
                "https://example.com",
                "/v1/models",
                "https://example.com/v1/models",
            ),
            (
                "https://example.com/gateway/",
                "/v1/models",
                "https://example.com/gateway/v1/models",
            ),
            (
                "https://example.com/gateway",
                "catalog?tenant=a%20b",
                "https://example.com/gateway/catalog?tenant=a%20b",
            ),
        ] {
            assert_eq!(endpoint(base, path)?.as_str(), expected);
        }
        for path in [
            "",
            "//evil.example/models",
            "https://evil.example/models",
            "\\evil",
            "/v1/models#fragment",
            "../models",
            "/%2E%2e/models",
            "/models\n",
            "?q=a",
        ] {
            assert!(
                endpoint("https://example.com/base", path).is_err(),
                "accepted unsafe path"
            );
        }
        for base in [
            "file:///tmp/a",
            "https://user:pass@example.com",
            "https://example.com?key=secret",
            "https://example.com#fragment",
        ] {
            assert!(endpoint(base, "/v1/models").is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn missing_path_defaults_and_custom_path_persists() -> Result<()> {
        let directory = TempDir::new()?;
        let provider: Provider = serde_json::from_value(json!({
            "id":"", "name":"Fixture", "protocol":"anthropic", "baseUrl":"https://example.com",
            "models":[{"id":"fixture", "name":"Fixture", "contextWindow":200_000}]
        }))?;
        assert_eq!(provider.list_models_path, "/v1/models");
        let app = App::open(AppOptions::local(directory.path().into()))?;
        let mut provider = app.save_provider(provider, None).await?;
        provider.list_models_path = "/catalog?tenant=fixture".into();
        app.save_provider(provider.clone(), None).await?;
        drop(app);
        let app = App::open(AppOptions::local(directory.path().into()))?;
        assert_eq!(
            app.store.provider(&provider.id)?.list_models_path,
            provider.list_models_path
        );
        Ok(())
    }

    async fn fixture(
        responses: Vec<String>,
    ) -> Result<(String, tokio::task::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await?;
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let count = socket.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request)?);
                socket.write_all(response.as_bytes()).await?;
                socket.shutdown().await?;
            }
            Ok(requests)
        });
        Ok((base, task))
    }

    fn response(status: &str, body: &str, headers: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn pagination_uses_protocol_headers_and_deduplicates() -> Result<()> {
        let (base, server) = fixture(vec![
            response("200 OK", r#"{"data":[{"id":"one","display_name":"One"}],"has_more":true,"last_id":"one"}"#, ""),
            response("200 OK", r#"{"data":[{"id":"one","display_name":"One"},{"id":"two","display_name":"Two"}],"has_more":false,"last_id":"two"}"#, ""),
        ]).await?;
        let models = list_models(
            &format!("{base}/gateway/"),
            "/catalog?tenant=a&after_id=ignored",
            Some("fixture-key"),
        )
        .await?;
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        let requests = server.await??;
        assert!(requests[0].starts_with("GET /gateway/catalog?tenant=a&limit=1000 HTTP/1.1"));
        assert!(
            requests[1]
                .starts_with("GET /gateway/catalog?tenant=a&limit=1000&after_id=one HTTP/1.1")
        );
        for request in requests {
            assert!(request.contains("x-api-key: fixture-key\r\n"));
            assert!(request.contains("anthropic-version: 2023-06-01\r\n"));
            assert!(!request.contains("authorization:"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn catalog_metadata_preserves_limits_and_explicit_effort_support() -> Result<()> {
        let body = json!({
            "data": [
                {
                    "id": "million", "display_name": "Million",
                    "max_input_tokens": 1_000_000, "max_tokens": 128_000,
                    "capabilities": {"effort": {
                        "supported": true,
                        "low": {"supported": true}, "medium": {"supported": true},
                        "high": {"supported": true}, "xhigh": {"supported": true},
                        "max": {"supported": true}
                    }}
                },
                {
                    "id": "subset", "display_name": "Subset",
                    "capabilities": {"effort": {
                        "supported": true, "low": {"supported": false},
                        "high": {"supported": true}, "xhigh": {"supported": true},
                        "unknown": {"supported": true}
                    }}
                },
                {
                    "id": "unsupported", "display_name": "Unsupported",
                    "capabilities": {"effort": {
                        "supported": false, "high": {"supported": true}
                    }}
                },
                {
                    "id": "unspecified", "display_name": "Unspecified",
                    "max_input_tokens": null, "max_tokens": null, "capabilities": null
                },
                {"id": "missing", "display_name": "Missing"}
            ],
            "has_more": false
        });
        let (base, server) = fixture(vec![response("200 OK", &body.to_string(), "")]).await?;
        let models = list_models(&base, "/v1/models", None).await?;
        assert_eq!(
            serde_json::to_value(models)?,
            json!([
                {
                    "id": "million", "displayName": "Million",
                    "contextWindow": 1_000_000, "maxOutputTokens": 128_000,
                    "supportedReasoningEfforts": ["low", "medium", "high", "xhigh", "max"]
                },
                {"id": "subset", "displayName": "Subset", "supportedReasoningEfforts": ["high", "xhigh"]},
                {"id": "unsupported", "displayName": "Unsupported", "supportedReasoningEfforts": []},
                {"id": "unspecified", "displayName": "Unspecified"},
                {"id": "missing", "displayName": "Missing"}
            ])
        );
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn independent_catalog_ceilings_do_not_block_other_models() -> Result<()> {
        let body = json!({
            "data": [
                {"id": "opus", "display_name": "Opus", "max_input_tokens": 1_000_000, "max_tokens": 128_000},
                {"id": "k3", "display_name": "K3", "max_input_tokens": 1_000_000, "max_tokens": 1_000_000},
                {"id": "independent", "display_name": "Independent", "max_input_tokens": 128_000, "max_tokens": 256_000}
            ],
            "has_more": false
        });
        let (base, server) = fixture(vec![response("200 OK", &body.to_string(), "")]).await?;
        assert_eq!(
            serde_json::to_value(list_models(&base, "/v1/models", None).await?)?,
            json!([
                {"id": "opus", "displayName": "Opus", "contextWindow": 1_000_000, "maxOutputTokens": 128_000},
                {"id": "k3", "displayName": "K3", "contextWindow": 1_000_000, "maxOutputTokens": 1_000_000},
                {"id": "independent", "displayName": "Independent", "contextWindow": 128_000, "maxOutputTokens": 256_000}
            ])
        );
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn invalid_catalog_limits_fail_without_replacing_them_with_defaults() -> Result<()> {
        for (context, output) in [
            (Some(0), None),
            (Some(-1), None),
            (Some(10_000_001), None),
            (None, Some(0)),
            (None, Some(-1)),
            (None, Some(10_000_001)),
        ] {
            let body = json!({
                "data": [{"id": "fixture", "display_name": "Fixture",
                    "max_input_tokens": context, "max_tokens": output}],
                "has_more": false
            });
            let (base, server) = fixture(vec![response("200 OK", &body.to_string(), "")]).await?;
            assert!(matches!(
                list_models(&base, "/v1/models", Some("fixture-key")).await,
                Err(ModelDiscoveryError::InvalidMetadata(_))
            ));
            server.await??;
        }
        Ok(())
    }

    #[tokio::test]
    async fn protocol_errors_and_redirects_are_safe() -> Result<()> {
        for (status, body, header, expected) in [
            ("401 Unauthorized", "fixture-key", "", "HTTP 401"),
            (
                "302 Found",
                "fixture-key",
                "Location: https://evil.example/\r\n",
                "HTTP 302",
            ),
            ("200 OK", r#"{"data":"fixture-key"}"#, "", "not valid"),
            (
                "200 OK",
                r#"{"data":[{"id":"one","display_name":"One","max_input_tokens":"fixture-key"}],"has_more":false}"#,
                "",
                "not valid",
            ),
            (
                "200 OK",
                r#"{"data":[{"id":"one","display_name":"One","capabilities":{"effort":{"supported":true,"high":{"supported":"fixture-key"}}}}],"has_more":false}"#,
                "",
                "not valid",
            ),
            (
                "200 OK",
                r#"{"data":[],"has_more":true,"last_id":"x"}"#,
                "",
                "pagination",
            ),
            (
                "200 OK",
                r#"{"data":[{"id":"","display_name":"A"}],"has_more":false}"#,
                "",
                "invalid model",
            ),
            (
                "200 OK",
                r#"{"data":[{"id":" one ","display_name":"One"}],"has_more":false}"#,
                "",
                "invalid model",
            ),
        ] {
            let (base, server) = fixture(vec![response(status, body, header)]).await?;
            let error = list_models(&base, "/models", Some("fixture-key"))
                .await
                .err()
                .context("response must fail")?;
            assert!(error.to_string().contains(expected));
            assert!(!format!("{error:?}").contains("fixture-key"));
            server.await??;
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejects_invalid_key_before_connecting_and_accepts_empty_list() -> Result<()> {
        assert!(matches!(
            list_models(
                "http://127.0.0.1:1",
                "/models",
                Some("key\r\nInjected: true")
            )
            .await,
            Err(ModelDiscoveryError::InvalidApiKey)
        ));
        let (base, server) = fixture(vec![response(
            "200 OK",
            r#"{"data":[],"has_more":false,"last_id":null}"#,
            "",
        )])
        .await?;
        assert!(list_models(&base, "/models", None).await?.is_empty());
        assert!(!server.await??[0].contains("x-api-key:"));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_repeated_cursor_and_oversized_body() -> Result<()> {
        let page =
            r#"{"data":[{"id":"one","display_name":"One"}],"has_more":true,"last_id":"one"}"#;
        let (base, server) = fixture(vec![
            response("200 OK", page, ""),
            response("200 OK", page, ""),
        ])
        .await?;
        assert!(matches!(
            list_models(&base, "/models", None).await,
            Err(ModelDiscoveryError::InvalidPagination)
        ));
        server.await??;
        let (base, server) = fixture(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            PAGE_BYTES + 1
        )])
        .await?;
        assert!(matches!(
            list_models(&base, "/models", None).await,
            Err(ModelDiscoveryError::TooLarge)
        ));
        server.await??;
        Ok(())
    }
}
