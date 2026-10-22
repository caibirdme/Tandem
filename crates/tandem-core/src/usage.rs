//! Local, event-based usage accounting, independent of optional telemetry exports.

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::{App, Error, Result};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageDay {
    pub date: String,
    pub start: String,
    pub end: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyTokens {
    pub date: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailySkills {
    pub date: String,
    pub skill_name: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub session_id: String,
    pub title: String,
    pub updated_at: String,
    pub total_tokens: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDashboard {
    pub tracking_started_at: String,
    pub daily_tokens: Vec<DailyTokens>,
    pub daily_skills: Vec<DailySkills>,
    pub recent_sessions: Vec<SessionUsage>,
    pub total_tokens: i64,
    pub skill_invocations: i64,
}

pub(crate) struct DayRange {
    pub date: String,
    pub start: i64,
    pub end: i64,
}

impl App {
    pub(crate) fn usage_dashboard(&self, days: Vec<UsageDay>) -> Result<UsageDashboard> {
        let days = validate_days(days)?;
        Ok(self.store.usage_dashboard(&days)?)
    }
}

fn validate_days(days: Vec<UsageDay>) -> Result<Vec<DayRange>> {
    if !matches!(days.len(), 7 | 14 | 30) {
        return Err(Error::InvalidUsageRange);
    }
    let mut previous: Option<(NaiveDate, i64)> = None;
    days.into_iter()
        .map(|day| {
            let date = NaiveDate::parse_from_str(&day.date, "%Y-%m-%d")
                .map_err(|_| Error::InvalidUsageRange)?;
            let start = DateTime::parse_from_rfc3339(&day.start)
                .map_err(|_| Error::InvalidUsageRange)?
                .timestamp_millis();
            let end = DateTime::parse_from_rfc3339(&day.end)
                .map_err(|_| Error::InvalidUsageRange)?
                .timestamp_millis();
            // Local calendar days may be shortened or lengthened by daylight saving.
            if !(22 * 3_600_000..=26 * 3_600_000).contains(&(end - start))
                || date.format("%Y-%m-%d").to_string() != day.date
                || previous.is_some_and(|(previous_date, previous_end)| {
                    previous_date.succ_opt() != Some(date) || previous_end != start
                })
            {
                return Err(Error::InvalidUsageRange);
            }
            previous = Some((date, end));
            Ok(DayRange {
                date: day.date,
                start,
                end,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days() -> Vec<UsageDay> {
        (1..=7)
            .map(|day| UsageDay {
                date: format!("2026-10-{day:02}"),
                start: format!("2026-10-{day:02}T00:00:00Z"),
                end: format!("2026-10-{:02}T00:00:00Z", day + 1),
            })
            .collect()
    }

    #[test]
    fn validates_calendar_ranges() {
        assert!(validate_days(days()).is_ok());
        assert!(validate_days(Vec::new()).is_err());
        let mut invalid = days();
        invalid[2].start = invalid[1].start.clone();
        assert!(validate_days(invalid).is_err());
        let mut invalid = days();
        invalid[2].date = "2026-10-04".into();
        assert!(validate_days(invalid).is_err());
    }
    #[test]
    fn accepts_dst_days_and_half_open_boundaries() -> Result<()> {
        let days = (1..=7)
            .map(|day| {
                let offset = if day == 1 { "-04:00" } else { "-05:00" };
                UsageDay {
                    date: format!("2026-11-{day:02}"),
                    start: format!("2026-11-{day:02}T00:00:00{offset}"),
                    end: format!("2026-11-{:02}T00:00:00-05:00", day + 1),
                }
            })
            .collect();
        let validated = validate_days(days)?;
        assert_eq!(validated[0].end - validated[0].start, 25 * 3_600_000);
        assert_eq!(validated[0].end, validated[1].start);
        Ok(())
    }
}
