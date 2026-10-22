use copilot_runtime::{AgentLensSettings, TelemetryProtocol, TelemetrySettings};
use serde_json::Value;

use crate::{
    App, Result,
    model::{Telemetry, ValidationError},
};

impl App {
    pub(crate) fn get_telemetry_api_key(&self) -> Result<Option<String>> {
        Ok(self.options.secrets.get_telemetry_api_key()?)
    }

    pub(crate) fn telemetry_settings(
        &self,
        telemetry: &Telemetry,
    ) -> Result<Option<TelemetrySettings>> {
        telemetry.validate()?;
        if !telemetry.enabled {
            return Ok(None);
        }
        let agentlens = telemetry
            .agentlens
            .as_ref()
            .map(|config| {
                let api_key = self
                    .get_telemetry_api_key()?
                    .filter(|key| !key.trim().is_empty())
                    .ok_or(ValidationError::MissingAgentLensApiKey)?;
                Ok::<_, crate::Error>(AgentLensSettings {
                    service_name: config.service_name.clone(),
                    api_key,
                })
            })
            .transpose()?;
        Ok(Some(TelemetrySettings {
            endpoint: telemetry.endpoint.clone(),
            protocol: if telemetry.protocol == "http/json" {
                TelemetryProtocol::HttpJson
            } else {
                TelemetryProtocol::HttpProtobuf
            },
            capture_content: telemetry.capture_content,
            agentlens,
        }))
    }

    /// Resolve private settings only for the encrypted SSH transport.
    pub(crate) fn remote_telemetry(&self) -> Result<Value> {
        let telemetry = self.store.preferences()?.telemetry;
        let Some(settings) = self.telemetry_settings(&telemetry)? else {
            return Ok(serde_json::json!({"enabled":false}));
        };
        let mut payload = serde_json::to_value(settings)?;
        payload["enabled"] = Value::Bool(true);
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::{AppOptions, Request, model::AgentLensConfig};

    fn agentlens() -> Telemetry {
        Telemetry {
            enabled: true,
            endpoint: "https://collector.example:4318".into(),
            protocol: "http/json".into(),
            agentlens: Some(AgentLensConfig {
                service_name: "tandem-test".into(),
            }),
            ..Telemetry::default()
        }
    }

    #[tokio::test]
    async fn telemetry_key_is_separate_persistent_and_only_loaded_on_demand() -> Result<()> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        let key = "fixture-agentlens-credential";
        let request = serde_json::from_value(json!({
            "command":"save_preferences", "payload":{
                "telemetry":agentlens(), "telemetryApiKey":key, "cliPath":null
            }
        }))?;
        app.dispatch(request).await?;
        assert_eq!(app.dispatch(Request::GetTelemetryApiKey {}).await?, key);
        let preferences = app.store.preferences()?;
        assert!(!serde_json::to_string(&preferences)?.contains(key));
        assert!(!format!("{preferences:?}").contains(key));
        assert!(!serde_json::to_string(&app.snapshot().await?)?.contains(key));
        let settings = app
            .telemetry_settings(&preferences.telemetry)?
            .context("telemetry enabled")?;
        assert_eq!(
            settings
                .agentlens
                .as_ref()
                .map(|config| config.api_key.as_str()),
            Some(key)
        );
        assert!(!format!("{settings:?}").contains(key));
        let remote = app.remote_telemetry()?;
        assert_eq!(remote["enabled"], true);
        assert_eq!(remote["agentlens"]["serviceName"], "tandem-test");
        assert_eq!(remote["agentlens"]["apiKey"], key);
        drop(app);

        let reopened = App::open(AppOptions::local(directory.path().into()))?;
        assert_eq!(reopened.get_telemetry_api_key()?.as_deref(), Some(key));
        let database = rusqlite::Connection::open(directory.path().join("tandem.sqlite3"))?;
        let persisted: String = database.query_row(
            "SELECT secret FROM telemetry_secrets WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(persisted, key);
        let ordinary: String =
            database.query_row("SELECT data FROM preferences WHERE id=1", [], |row| {
                row.get(0)
            })?;
        assert!(!ordinary.contains(key));
        let provider_keys: i64 =
            database.query_row("SELECT COUNT(*) FROM provider_secrets", [], |row| {
                row.get(0)
            })?;
        assert_eq!(provider_keys, 0);
        Ok(())
    }

    #[tokio::test]
    async fn telemetry_key_preserve_replace_clear_and_missing_validation() -> Result<()> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        for key in [None, Some(String::new()), Some("  ".into())] {
            let error = app
                .save_preferences(agentlens(), None, key)
                .await
                .err()
                .context("missing AgentLens key must reject enabled telemetry")?;
            assert!(matches!(
                error,
                crate::Error::Validation(ValidationError::MissingAgentLensApiKey)
            ));
            assert!(!app.store.preferences()?.telemetry.enabled);
            assert!(app.get_telemetry_api_key()?.is_none());
        }
        for key in ["fixture-first-key", "fixture-replacement-key"] {
            app.save_preferences(agentlens(), None, Some(key.into()))
                .await?;
            app.save_preferences(agentlens(), None, None).await?;
            assert_eq!(app.get_telemetry_api_key()?.as_deref(), Some(key));
        }
        assert!(matches!(
            app.save_preferences(agentlens(), None, Some(String::new()))
                .await,
            Err(crate::Error::Validation(
                ValidationError::MissingAgentLensApiKey
            ))
        ));
        assert_eq!(
            app.get_telemetry_api_key()?.as_deref(),
            Some("fixture-replacement-key")
        );
        app.save_preferences(Telemetry::default(), None, None)
            .await?;
        assert_eq!(
            app.get_telemetry_api_key()?.as_deref(),
            Some("fixture-replacement-key")
        );
        assert_eq!(app.remote_telemetry()?, json!({"enabled":false}));
        app.save_preferences(Telemetry::default(), None, Some(String::new()))
            .await?;
        assert!(app.get_telemetry_api_key()?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn failed_preferences_save_restores_the_previous_telemetry_key() -> Result<()> {
        let directory = TempDir::new()?;
        let app = App::open(AppOptions::local(directory.path().into()))?;
        app.options
            .secrets
            .set_telemetry_api_key(Some("fixture-existing-tenant"))?;
        let database = rusqlite::Connection::open(directory.path().join("tandem.sqlite3"))?;
        database.execute_batch(
            "CREATE TRIGGER reject_preferences BEFORE INSERT ON preferences
             BEGIN SELECT RAISE(ABORT, 'fixture preference failure'); END;",
        )?;
        let result = app
            .save_preferences(agentlens(), None, Some("fixture-replacement-tenant".into()))
            .await;
        assert!(matches!(result, Err(crate::Error::Storage(_))));
        assert_eq!(
            app.get_telemetry_api_key()?.as_deref(),
            Some("fixture-existing-tenant")
        );
        assert!(!app.store.preferences()?.telemetry.enabled);
        Ok(())
    }

    #[test]
    fn telemetry_failures_keep_typed_transport_policy() {
        use copilot_runtime::TelemetryError;

        use crate::ErrorCode;

        for (failure, expected) in [
            (
                TelemetryError::HttpStatus { status: 401 },
                ErrorCode::InvalidInput,
            ),
            (
                TelemetryError::HttpStatus { status: 429 },
                ErrorCode::Unavailable,
            ),
            (
                TelemetryError::HttpStatus { status: 503 },
                ErrorCode::Unavailable,
            ),
            (
                TelemetryError::RejectedSpans { count: 1 },
                ErrorCode::InvalidInput,
            ),
            (TelemetryError::InvalidResponse, ErrorCode::Unavailable),
            (TelemetryError::FlushTimeout, ErrorCode::Unavailable),
            (TelemetryError::InvalidSpan, ErrorCode::Internal),
        ] {
            let error = crate::Error::Runtime(failure.into());
            assert_eq!(error.code(), expected);
        }
    }

    #[test]
    fn agentlens_service_names_are_validated_without_echoing_values() {
        for name in ["", "  ", "line\nbreak", &"x".repeat(257)] {
            let mut telemetry = agentlens();
            telemetry.agentlens = Some(AgentLensConfig {
                service_name: name.into(),
            });
            assert!(matches!(
                telemetry.validate(),
                Err(ValidationError::InvalidAgentLensServiceName)
            ));
        }
        assert!(agentlens().validate().is_ok());
        let mut wrong_protocol = agentlens();
        wrong_protocol.protocol = "http/protobuf".into();
        assert!(matches!(
            wrong_protocol.validate(),
            Err(ValidationError::AgentLensRequiresJson)
        ));
    }
}
