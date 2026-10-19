//! Permission mediation: chat is automatic; work can require approval per request.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use async_trait::async_trait;
use github_copilot_sdk::{
    PermissionRequestData, RequestId, SessionId,
    handler::{PermissionHandler, PermissionResult},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};

use crate::{Error, EventKind, PermissionMode, Result, RuntimeEvent, SessionMode};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(600);

/// Approval is scoped to exactly one SDK request, never the whole session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

type PendingApprovals = HashMap<String, oneshot::Sender<ApprovalDecision>>;

pub(crate) struct Permissions {
    chat: bool,
    mode: PermissionMode,
    events: broadcast::Sender<RuntimeEvent>,
    pending: Mutex<PendingApprovals>,
}

impl Permissions {
    pub(crate) fn new(
        session_mode: &SessionMode,
        mode: PermissionMode,
        events: broadcast::Sender<RuntimeEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            chat: matches!(session_mode, SessionMode::Chat),
            mode,
            events,
            pending: Mutex::new(HashMap::new()),
        })
    }

    fn pending(&self) -> MutexGuard<'_, PendingApprovals> {
        // Critical sections only modify the registry and never execute user code.
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn resolve(&self, request_id: &str, decision: ApprovalDecision) -> Result<()> {
        self.pending()
            .remove(request_id)
            .ok_or(Error::UnknownApproval)?
            .send(decision)
            .map_err(|_| Error::UnknownApproval)
    }

    pub(crate) fn cancel_all(&self) {
        for (_, response) in self.pending().drain() {
            let _ = response.send(ApprovalDecision::Deny);
        }
    }
}

/// Removes the request even when the SDK drops its handler future mid-approval.
struct PendingApproval<'a> {
    permissions: &'a Permissions,
    request_id: String,
}

impl Drop for PendingApproval<'_> {
    fn drop(&mut self) {
        self.permissions.pending().remove(&self.request_id);
    }
}

fn describe_request(details: &Value) -> String {
    let intention = details
        .get("intention")
        .and_then(Value::as_str)
        .unwrap_or("Allow this agent action?");
    let mut sections = vec![intention.to_owned()];
    for key in ["fileName", "path", "url", "fullCommandText", "command"] {
        if let Some(value) = details
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            sections.push(value.to_owned());
        }
    }
    let preview = details
        .get("diff")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| details.get("newFileContents").and_then(Value::as_str));
    if let Some(preview) = preview {
        sections.push(preview.to_owned());
    }
    for key in ["warning", "requestSandboxBypassReason"] {
        if let Some(warning) = details.get(key).and_then(Value::as_str) {
            sections.push(warning.to_owned());
        }
    }
    if sections.len() == 1 {
        sections
            .push(serde_json::to_string_pretty(details).unwrap_or_else(|_| details.to_string()));
    }
    sections.join("\n\n")
}

#[async_trait]
impl PermissionHandler for Permissions {
    async fn handle(
        &self,
        session_id: SessionId,
        request_id: RequestId,
        data: PermissionRequestData,
    ) -> PermissionResult {
        if self.chat || self.mode == PermissionMode::AllowAll {
            return PermissionResult::approve_once();
        }
        let request_id = request_id.to_string();
        let (response, decision) = oneshot::channel();
        self.pending().insert(request_id.clone(), response);
        let _pending = PendingApproval {
            permissions: self,
            request_id: request_id.clone(),
        };
        let details = data.extra.get("permissionRequest").unwrap_or(&data.extra);
        let description = describe_request(details);
        let event = RuntimeEvent {
            session_id: session_id.into_inner(),
            kind: EventKind::Permission,
            data: json!({
                "requestId": request_id,
                "kind": data.kind,
                "description": description,
                "details": details,
            }),
        };
        if self.events.send(event).is_err() {
            return PermissionResult::user_not_available();
        }
        let outcome = tokio::time::timeout(APPROVAL_TIMEOUT, decision).await;
        match outcome {
            Ok(Ok(ApprovalDecision::Approve)) => PermissionResult::approve_once(),
            Ok(Ok(ApprovalDecision::Deny)) => {
                PermissionResult::reject(Some("User denied this action.".into()))
            }
            _ => PermissionResult::user_not_available(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use github_copilot_sdk::{PermissionDecision, PermissionRequestKind};

    use super::*;

    #[test]
    fn permission_review_includes_the_actual_impact() {
        for (request, expected) in [
            (
                json!({"intention":"Create file", "fileName":"/project/main.rs", "newFileContents":"fn main() {}"}),
                vec!["/project/main.rs", "fn main() {}"],
            ),
            (
                json!({"intention":"Edit file", "fileName":"/project/main.rs", "diff":"-old\n+new"}),
                vec!["/project/main.rs", "-old\n+new"],
            ),
            (
                json!({"intention":"Run command", "fullCommandText":"cargo test --workspace"}),
                vec!["cargo test --workspace"],
            ),
            (
                json!({"kind":"future", "resource":"review-me"}),
                vec!["review-me"],
            ),
        ] {
            let description = describe_request(&request);
            for value in expected {
                assert!(
                    description.contains(value),
                    "missing impact {value} in {description}"
                );
            }
        }
    }

    #[tokio::test]
    async fn chat_approves_every_request_without_publishing_or_waiting_for_approval() {
        for mode in [PermissionMode::Ask, PermissionMode::AllowAll] {
            for subscribed in [false, true] {
                let (events, receiver) = broadcast::channel(16);
                let mut receiver = subscribed.then_some(receiver);
                let permissions = Permissions::new(&SessionMode::Chat, mode, events);
                for kind in [
                    Some(PermissionRequestKind::Url),
                    Some(PermissionRequestKind::Shell),
                    Some(PermissionRequestKind::Read),
                    Some(PermissionRequestKind::Write),
                    Some(PermissionRequestKind::Mcp),
                    Some(PermissionRequestKind::CustomTool),
                    Some(PermissionRequestKind::Memory),
                    Some(PermissionRequestKind::Hook),
                    Some(PermissionRequestKind::Unknown),
                    None,
                ] {
                    let result = tokio::time::timeout(
                        Duration::from_secs(1),
                        permissions.handle(
                            "chat".into(),
                            "automatic".into(),
                            PermissionRequestData {
                                kind,
                                ..Default::default()
                            },
                        ),
                    )
                    .await
                    .expect("chat must not wait for a UI");
                    assert!(
                        matches!(
                            result,
                            PermissionResult::Decision {
                                decision: PermissionDecision::ApproveOnce(_),
                                ..
                            }
                        ),
                        "{mode:?} {kind:?}"
                    );
                    assert!(permissions.pending().is_empty());
                    if let Some(receiver) = &mut receiver {
                        assert!(receiver.try_recv().is_err());
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn authorized_work_does_not_publish_or_wait_for_approvals() {
        for subscribed in [false, true] {
            let (events, receiver) = broadcast::channel(16);
            let mut receiver = subscribed.then_some(receiver);
            let permissions = Permissions::new(
                &SessionMode::Work {
                    directory: "/project".into(),
                },
                PermissionMode::AllowAll,
                events,
            );
            for kind in [
                PermissionRequestKind::Read,
                PermissionRequestKind::Write,
                PermissionRequestKind::Shell,
            ] {
                let result = tokio::time::timeout(
                    Duration::from_secs(1),
                    permissions.handle(
                        "work".into(),
                        "automatic".into(),
                        PermissionRequestData {
                            kind: Some(kind),
                            ..Default::default()
                        },
                    ),
                )
                .await
                .expect("pre-authorized action must not wait for a UI");
                assert!(matches!(
                    result,
                    PermissionResult::Decision {
                        decision: PermissionDecision::ApproveOnce(_),
                        ..
                    }
                ));
                assert!(permissions.pending().is_empty());
                if let Some(receiver) = &mut receiver {
                    assert!(receiver.try_recv().is_err());
                }
            }
        }
    }

    #[tokio::test]
    async fn work_ask_waits_for_each_decision() {
        let work = SessionMode::Work {
            directory: "/project".into(),
        };
        for (session_mode, mode, kind) in [
            (&work, PermissionMode::Ask, PermissionRequestKind::Read),
            (&work, PermissionMode::Ask, PermissionRequestKind::Write),
            (&work, PermissionMode::Ask, PermissionRequestKind::Shell),
            (&work, PermissionMode::Ask, PermissionRequestKind::Url),
        ] {
            for decision in [ApprovalDecision::Approve, ApprovalDecision::Deny] {
                let (events, mut receiver) = broadcast::channel(16);
                let permissions = Permissions::new(session_mode, mode, events);
                let pending = permissions.handle(
                    "session".into(),
                    "review".into(),
                    PermissionRequestData {
                        kind: Some(kind),
                        ..Default::default()
                    },
                );
                tokio::pin!(pending);
                assert!(
                    tokio::time::timeout(Duration::from_millis(10), &mut pending)
                        .await
                        .is_err()
                );
                let event = receiver.try_recv().expect("approval was published");
                assert_eq!(event.kind, EventKind::Permission);
                assert_eq!(event.data["requestId"], "review");
                permissions
                    .resolve("review", decision)
                    .expect("pending approval");
                let result = pending.await;
                assert!(match decision {
                    ApprovalDecision::Approve => matches!(
                        result,
                        PermissionResult::Decision {
                            decision: PermissionDecision::ApproveOnce(_),
                            ..
                        }
                    ),
                    ApprovalDecision::Deny => matches!(
                        result,
                        PermissionResult::Decision {
                            decision: PermissionDecision::Reject(_),
                            ..
                        }
                    ),
                });
                assert!(permissions.pending().is_empty());
            }
        }
    }

    #[tokio::test]
    async fn approval_is_single_use_and_cancellation_denies_pending_actions() {
        let (events, _receiver) = broadcast::channel(16);
        let permissions = Permissions::new(&SessionMode::Chat, PermissionMode::Ask, events);
        let (sender, receiver) = oneshot::channel();
        permissions.pending().insert("request".into(), sender);
        permissions
            .resolve("request", ApprovalDecision::Approve)
            .expect("pending request");
        assert_eq!(receiver.await.expect("decision"), ApprovalDecision::Approve);
        assert!(
            permissions
                .resolve("request", ApprovalDecision::Approve)
                .is_err()
        );
        let (sender, receiver) = oneshot::channel();
        permissions.pending().insert("cancelled".into(), sender);
        permissions.cancel_all();
        assert_eq!(receiver.await.expect("decision"), ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn dropping_an_sdk_approval_future_removes_the_pending_request() {
        let (events, mut receiver) = broadcast::channel(16);
        let permissions = Permissions::new(
            &SessionMode::Work {
                directory: "/project".into(),
            },
            PermissionMode::Ask,
            events,
        );
        let handler = permissions.clone();
        let pending = tokio::spawn(async move {
            let request = PermissionRequestData {
                kind: Some(PermissionRequestKind::Url),
                extra: json!({"url": "https://example.com"}),
                ..Default::default()
            };
            handler
                .handle("work".into(), "cancelled".into(), request)
                .await
        });
        let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("approval published")
            .expect("approval event");
        assert_eq!(event.data["requestId"], "cancelled");
        pending.abort();
        assert!(pending.await.expect_err("cancelled handler").is_cancelled());
        assert!(permissions.pending().is_empty());
        assert!(matches!(
            permissions.resolve("cancelled", ApprovalDecision::Approve),
            Err(Error::UnknownApproval)
        ));
    }
}
