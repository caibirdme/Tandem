use chrono::{DateTime, Utc};
use copilot_runtime::{EventKind, RuntimeEvent};
use rusqlite::{Connection, params};

use super::{Result, Store};
use crate::usage::{DailySkills, DailyTokens, DayRange, SessionUsage, UsageDashboard};

pub(super) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS usage_metadata (
             id INTEGER PRIMARY KEY CHECK(id=1), tracking_started_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS usage_events (
             session_id TEXT NOT NULL, event_id TEXT NOT NULL, kind TEXT NOT NULL,
             occurred_at INTEGER NOT NULL,
             provider_id TEXT NOT NULL, provider_name TEXT NOT NULL,
             model_id TEXT NOT NULL, input_tokens INTEGER NOT NULL,
             output_tokens INTEGER NOT NULL, skill_name TEXT NOT NULL,
             PRIMARY KEY(session_id,event_id,kind)
         );
         CREATE INDEX IF NOT EXISTS usage_events_time ON usage_events(occurred_at);",
    )?;
    db.execute(
        "INSERT OR IGNORE INTO usage_metadata(id,tracking_started_at) VALUES(1,?1)",
        [Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

impl Store {
    /// Store only accounting metadata; never store skill contents, prompts or credentials.
    /// Event IDs survive replay and the primary key makes delivery idempotent.
    pub(crate) fn record_usage(&self, event: &RuntimeEvent) -> Result<bool> {
        let data = &event.data;
        let Some(event_id) = data["eventId"].as_str().filter(|id| !id.is_empty()) else {
            return Ok(false);
        };
        let Some(timestamp) = data["timestamp"]
            .as_str()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        else {
            return Ok(false);
        };
        let Some(provider_id) = data["providerId"].as_str() else {
            return Ok(false);
        };
        let Some(provider_name) = data["providerName"].as_str() else {
            return Ok(false);
        };
        let (kind, model, input, output, skill) = match event.kind {
            EventKind::Usage => {
                let Some(model) = data["model"].as_str().filter(|model| !model.is_empty()) else {
                    return Ok(false);
                };
                let (Some(input), Some(output)) =
                    (data["inputTokens"].as_i64(), data["outputTokens"].as_i64())
                else {
                    return Ok(false);
                };
                if input < 0
                    || output < 0
                    || input
                        .checked_add(output)
                        .is_none_or(|total| total > 9_007_199_254_740_991)
                {
                    return Ok(false);
                }
                ("tokens", model, input, output, "")
            }
            EventKind::SkillUsage => {
                let Some(skill) = data["skillName"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                else {
                    return Ok(false);
                };
                ("skill", "", 0, 0, skill)
            }
            _ => return Ok(false),
        };
        self.with(|db| Ok(db.execute(
            "INSERT OR IGNORE INTO usage_events(session_id,event_id,kind,occurred_at,provider_id,provider_name,model_id,input_tokens,output_tokens,skill_name) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![event.session_id,event_id,kind,timestamp.timestamp_millis(),provider_id,provider_name,model,input,output,skill],
        )? > 0))
    }

    pub(crate) fn usage_dashboard(&self, days: &[DayRange]) -> Result<UsageDashboard> {
        self.with(|db| {
            let tracking_started_at = db.query_row(
                "SELECT tracking_started_at FROM usage_metadata WHERE id=1",
                [],
                |row| row.get(0),
            )?;
            let mut daily_tokens = Vec::new();
            let mut daily_skills = Vec::new();
            for day in days {
                daily_tokens.extend(token_totals(db, day)?);
                daily_skills.extend(skill_totals(db, day)?);
            }
            let start = days.first().map_or(0, |day| day.start);
            let end = days.last().map_or(0, |day| day.end);
            let recent_sessions = recent_sessions(db, start, end)?;
            let total_tokens = daily_tokens
                .iter()
                .fold(0_i64, |sum, day| sum.saturating_add(day.total_tokens));
            let skill_invocations = daily_skills.iter().map(|day| day.count).sum();
            Ok(UsageDashboard {
                tracking_started_at,
                daily_tokens,
                daily_skills,
                recent_sessions,
                total_tokens,
                skill_invocations,
            })
        })
    }
}

fn token_totals(db: &Connection, day: &DayRange) -> Result<Vec<DailyTokens>> {
    let mut statement = db.prepare_cached(
        "SELECT provider_id, MAX(provider_name), model_id, SUM(input_tokens), SUM(output_tokens)
         FROM usage_events WHERE kind='tokens' AND occurred_at>=?1 AND occurred_at<?2
         GROUP BY provider_id,model_id ORDER BY provider_id,model_id",
    )?;
    let rows = statement.query_map(params![day.start, day.end], |row| {
        let input: i64 = row.get(3)?;
        let output: i64 = row.get(4)?;
        Ok(DailyTokens {
            date: day.date.clone(),
            provider_id: row.get(0)?,
            provider_name: row.get(1)?,
            model_id: row.get(2)?,
            input_tokens: input,
            output_tokens: output,
            total_tokens: input.saturating_add(output),
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn skill_totals(db: &Connection, day: &DayRange) -> Result<Vec<DailySkills>> {
    let mut statement = db.prepare_cached(
        "SELECT skill_name, COUNT(*) FROM usage_events
         WHERE kind='skill' AND occurred_at>=?1 AND occurred_at<?2
         GROUP BY skill_name ORDER BY skill_name",
    )?;
    let rows = statement.query_map(params![day.start, day.end], |row| {
        Ok(DailySkills {
            date: day.date.clone(),
            skill_name: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn recent_sessions(db: &Connection, start: i64, end: i64) -> Result<Vec<SessionUsage>> {
    // Message activity, not title edits or settings changes, determines recency.
    let mut statement = db.prepare_cached(
        "WITH activity AS (
            SELECT session_id, MAX(json_extract(data,'$.createdAt')) AS latest
            FROM messages GROUP BY session_id
         ), totals AS (
            SELECT session_id, SUM(input_tokens+output_tokens) AS tokens
            FROM usage_events WHERE kind='tokens' AND occurred_at>=?1 AND occurred_at<?2
            GROUP BY session_id
         )
         SELECT sessions.id,json_extract(sessions.data,'$.title'),activity.latest,COALESCE(totals.tokens,0)
         FROM sessions JOIN activity ON activity.session_id=sessions.id
         LEFT JOIN totals ON totals.session_id=sessions.id
         WHERE unixepoch(activity.latest,'subsec')*1000>=?1 AND unixepoch(activity.latest,'subsec')*1000<?2
         ORDER BY activity.latest DESC,sessions.id LIMIT 10",
    )?;
    let rows = statement.query_map(params![start, end], |row| {
        Ok(SessionUsage {
            session_id: row.get(0)?,
            title: row.get(1)?,
            updated_at: row.get(2)?,
            total_tokens: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(id: &str, timestamp: &str, model: &str) -> RuntimeEvent {
        RuntimeEvent {
            session_id: "session-1".into(),
            kind: EventKind::Usage,
            data: json!({"eventId":id,"timestamp":timestamp,"providerId":"provider-1","providerName":"Provider","model":model,"inputTokens":100,"outputTokens":20}),
        }
    }

    fn day() -> DayRange {
        DayRange {
            date: "2026-10-22".into(),
            start: 1_792_627_200_000,
            end: 1_792_713_600_000,
        }
    }

    #[test]
    fn persists_deduplicates_and_attributes_usage()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("usage.sqlite3");
        let started;
        {
            let store = Store::open(&path)?;
            started = store.usage_dashboard(&[day()])?.tracking_started_at;
            let first = event("event-1", "2026-10-22T00:00:00Z", "model-a");
            assert!(store.record_usage(&first)?);
            assert!(!store.record_usage(&first)?);
            assert!(store.record_usage(&event("event-2", "2026-10-22T01:00:00Z", "model-b"))?);
            let mut skill = event("skill-1", "2026-10-22T01:00:00Z", "model-b");
            skill.kind = EventKind::SkillUsage;
            skill.data["skillName"] = json!("review");
            assert!(store.record_usage(&skill)?);
            assert!(!store.record_usage(&skill)?);
            // End boundaries are exclusive; the midnight call belongs to tomorrow.
            store.record_usage(&event("tomorrow", "2026-10-23T00:00:00Z", "model-a"))?;
        }
        let store = Store::open(&path)?;
        let result = store.usage_dashboard(&[day()])?;
        assert_eq!(result.tracking_started_at, started);
        assert_eq!(result.total_tokens, 240);
        assert_eq!(result.daily_tokens.len(), 2);
        assert_eq!(result.daily_tokens[0].model_id, "model-a");
        assert_eq!(result.daily_tokens[1].model_id, "model-b");
        assert_eq!(result.skill_invocations, 1);
        assert_eq!(result.daily_skills[0].skill_name, "review");
        Ok(())
    }

    #[test]
    fn malformed_events_are_not_recorded() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        let valid = event("event", "2026-10-22T01:00:00Z", "model");
        for (field, value) in [
            ("inputTokens", json!(-1)),
            ("outputTokens", json!(1.5)),
            ("timestamp", json!("bad")),
            ("eventId", json!("")),
            ("providerId", json!(null)),
            ("model", json!(null)),
        ] {
            let mut invalid = valid.clone();
            invalid.data[field] = value;
            assert!(!store.record_usage(&invalid)?, "{field}");
        }
        assert_eq!(store.usage_dashboard(&[day()])?.total_tokens, 0);
        Ok(())
    }

    #[test]
    fn recent_sessions_use_message_activity_and_selected_window() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        store.with(|db| {
            db.execute("INSERT INTO providers(id,data) VALUES('provider-1','{}')", [])?;
            for index in 0..12 {
                let id = format!("session-{index}");
                db.execute("INSERT INTO sessions(id,provider_id,data) VALUES(?1,'provider-1',?2)", params![id,json!({"title":format!("Session {index}"),"updatedAt":"2026-11-01T00:00:00Z"}).to_string()])?;
                db.execute("INSERT INTO messages(id,session_id,data) VALUES(?1,?1,?2)", params![id,json!({"createdAt":format!("2026-10-22T{index:02}:00:00Z")}).to_string()])?;
            }
            Ok(())
        })?;
        let mut usage = event("today", "2026-10-22T11:00:00Z", "model");
        usage.session_id = "session-11".into();
        store.record_usage(&usage)?;
        usage.data["eventId"] = json!("yesterday");
        usage.data["timestamp"] = json!("2026-10-21T11:00:00Z");
        store.record_usage(&usage)?;
        let result = store.usage_dashboard(&[day()])?;
        assert_eq!(result.recent_sessions.len(), 10);
        assert_eq!(result.recent_sessions[0].session_id, "session-11");
        assert_eq!(result.recent_sessions[0].total_tokens, 120);
        assert_eq!(result.recent_sessions[9].session_id, "session-2");
        Ok(())
    }
    #[test]
    fn historical_attribution_survives_provider_and_session_deletion() -> Result<()> {
        let store = Store::open(std::path::Path::new(":memory:"))?;
        store.with(|db| {
            db.execute(
                "INSERT INTO providers(id,data) VALUES('provider-1','{}')",
                [],
            )?;
            db.execute(
                "INSERT INTO sessions(id,provider_id,data) VALUES('session-1','provider-1','{}')",
                [],
            )?;
            Ok(())
        })?;
        let usage = event("event", "2026-10-22T01:00:00Z", "removed-model");
        store.record_usage(&usage)?;
        store.with(|db| {
            db.execute("DELETE FROM sessions WHERE id='session-1'", [])?;
            db.execute("DELETE FROM providers WHERE id='provider-1'", [])?;
            Ok(())
        })?;
        // No current provider/session lookup is used to reconstruct historical data.
        let result = store.usage_dashboard(&[day()])?;
        assert_eq!(result.daily_tokens[0].provider_name, "Provider");
        assert_eq!(result.daily_tokens[0].model_id, "removed-model");
        assert!(result.recent_sessions.is_empty());
        Ok(())
    }
}
