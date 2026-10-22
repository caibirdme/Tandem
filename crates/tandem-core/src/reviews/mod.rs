mod lifecycle;
mod location;
#[cfg(test)]
mod tests;

use std::{collections::HashMap, sync::Arc};

use copilot_runtime::workspace::{ReviewFileDiff, ReviewFilePage, ReviewSelection, ReviewSummary};
use location::WorkspaceHandle;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};

use crate::{App, Error, Result, model::Session};

#[derive(Default)]
pub(crate) struct ReviewCoordinator {
    state: Arc<Mutex<CoordinatorState>>,
    locations: Mutex<HashMap<String, WorkspaceHandle>>,
}

#[derive(Default)]
struct CoordinatorState {
    active: HashMap<String, ActiveReview>,
    finishing: HashMap<String, watch::Receiver<bool>>,
}

struct ActiveReview {
    handle: Option<WorkspaceHandle>,
    record: TurnRecord,
    committed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UnavailableCode {
    NotRepository,
    GitUnavailable,
    RemoteUnavailable,
    BaselineUnavailable,
    Capturing,
    InspectionFailed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReviewUnavailable {
    pub code: UnavailableCode,
    pub message: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnRecord {
    message_id: String,
    captured_at: u64,
    before: Option<String>,
    after: Option<String>,
    complete: bool,
    unavailable: Option<ReviewUnavailable>,
    summary: Option<ReviewSummary>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    baseline: Option<String>,
    latest: Option<ReviewSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewTurn {
    message_id: String,
    captured_at: u64,
    complete: bool,
    available: bool,
}

#[derive(Serialize)]
pub(crate) struct ReviewOverview {
    summary: Option<ReviewSummary>,
    unavailable: Option<ReviewUnavailable>,
    turns: Vec<ReviewTurn>,
}

impl App {
    pub(crate) async fn get_review_summary(
        &self,
        session_id: &str,
        selection: ReviewSelection,
    ) -> Result<ReviewOverview> {
        let session = self.review_session(session_id)?;
        let records = self.store.review_turns::<TurnRecord>(session_id)?;
        let turns = records
            .iter()
            .map(|turn| ReviewTurn {
                message_id: turn.message_id.clone(),
                captured_at: turn.captured_at,
                complete: turn.complete,
                available: turn.summary.is_some(),
            })
            .collect();
        let selected_turn = match &selection {
            ReviewSelection::Turn { message_id } => Some(message_id.clone()),
            _ => None,
        };
        let recorded_unavailable = match &selection {
            ReviewSelection::Turn { message_id } => {
                records.iter().find(|turn| &turn.message_id == message_id)
            }
            ReviewSelection::Session => records.last(),
            ReviewSelection::Workspace { .. } => None,
        }
        .and_then(|turn| turn.unavailable.clone());
        let result = match selection {
            ReviewSelection::Workspace { comparison } => {
                match self.review_location(&session, None).await {
                    Ok(handle) => handle.workspace(comparison).await,
                    Err(error) => Err(error),
                }
            }
            ReviewSelection::Session => self
                .store
                .review_session::<SessionRecord>(session_id)?
                .and_then(|record| record.latest)
                .ok_or(Error::ReviewBaselineUnavailable),
            ReviewSelection::Turn { message_id } => records
                .iter()
                .find(|turn| turn.message_id == message_id)
                .and_then(|turn| turn.summary.clone())
                .ok_or(Error::ReviewBaselineUnavailable),
        };
        match result {
            Ok(summary) => {
                self.store
                    .save_review_result(session_id, &summary.review_id, &summary)?;
                Ok(ReviewOverview {
                    summary: Some(summary),
                    unavailable: None,
                    turns,
                })
            }
            Err(error) => {
                let mut unavailable =
                    recorded_unavailable.unwrap_or_else(|| describe_unavailable(&error));
                if matches!(error, Error::ReviewBaselineUnavailable) {
                    let state = self.reviews.state.lock().await;
                    let capturing = state.active.get(session_id).is_some_and(|active| {
                        selected_turn
                            .as_ref()
                            .is_none_or(|id| id == &active.record.message_id)
                    }) || state.finishing.contains_key(session_id)
                        && selected_turn.as_ref().is_none_or(|id| {
                            records.last().is_some_and(|turn| &turn.message_id == id)
                        });
                    if capturing {
                        unavailable = ReviewUnavailable {
                            code: UnavailableCode::Capturing,
                            message:
                                "The turn's changes will be available when its capture finishes."
                                    .into(),
                        };
                    }
                }
                Ok(ReviewOverview {
                    summary: None,
                    unavailable: Some(unavailable),
                    turns,
                })
            }
        }
    }

    pub(crate) async fn list_review_files(
        &self,
        session_id: &str,
        review_id: &str,
        revision: &str,
        cursor: usize,
    ) -> Result<ReviewFilePage> {
        let session = self.review_session(session_id)?;
        self.registered_review(session_id, review_id, revision)?;
        let cache = self.review_cache(review_id, revision, &format!("page-{cursor}"))?;
        if let Some(page) = location::cached(&cache).await? {
            return Ok(page);
        }
        let handle = self.review_location(&session, None).await?;
        let page = handle.files(review_id, revision, cursor).await?;
        location::cache(&cache, &page).await?;
        Ok(page)
    }

    pub(crate) async fn get_review_file(
        &self,
        session_id: &str,
        review_id: &str,
        revision: &str,
        file_id: &str,
    ) -> Result<ReviewFileDiff> {
        let session = self.review_session(session_id)?;
        self.registered_review(session_id, review_id, revision)?;
        location::validate_id(file_id)?;
        let cache = self.review_cache(review_id, revision, &format!("file-{file_id}"))?;
        if let Some(file) = location::cached(&cache).await? {
            return Ok(file);
        }
        let handle = self.review_location(&session, None).await?;
        let file = handle.file(review_id, revision, file_id).await?;
        location::cache(&cache, &file).await?;
        Ok(file)
    }

    fn review_session(&self, session_id: &str) -> Result<Session> {
        let session = self.store.session(session_id)?;
        if session.project_id.is_none() {
            return Err(Error::WorkSessionRequired);
        }
        Ok(session)
    }

    fn registered_review(
        &self,
        session_id: &str,
        review_id: &str,
        revision: &str,
    ) -> Result<ReviewSummary> {
        let summary = self
            .store
            .review_result::<ReviewSummary>(session_id, review_id)?
            .ok_or(Error::ReviewNotRegistered)?;
        if summary.revision != revision {
            return Err(Error::ReviewRevisionExpired);
        }
        Ok(summary)
    }
}

fn describe_unavailable(error: &Error) -> ReviewUnavailable {
    use copilot_runtime::workspace::ReviewErrorCode;
    let code = match error {
        Error::Review(error) => match error.code() {
            ReviewErrorCode::NotRepository => UnavailableCode::NotRepository,
            ReviewErrorCode::GitUnavailable => UnavailableCode::GitUnavailable,
            _ => UnavailableCode::InspectionFailed,
        },
        Error::Remote(error) => match error.code() {
            tandem_remote::RpcErrorCode::ReviewNotRepository => UnavailableCode::NotRepository,
            tandem_remote::RpcErrorCode::ReviewGitUnavailable => UnavailableCode::GitUnavailable,
            _ => UnavailableCode::RemoteUnavailable,
        },
        Error::ReviewBaselineUnavailable => UnavailableCode::BaselineUnavailable,
        _ => UnavailableCode::InspectionFailed,
    };
    ReviewUnavailable {
        code,
        message: error.to_string(),
    }
}
