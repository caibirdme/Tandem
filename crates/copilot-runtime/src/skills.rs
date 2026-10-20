//! Project-scoped skill discovery and explicit invocation through the pinned SDK.

use std::path::{Path, PathBuf};

use github_copilot_sdk::{
    Client, MessageOptions,
    rpc::{
        CommandsInvokeRequest, CommandsListRequest, Skill, SkillsDisableRequest,
        SkillsDiscoverRequest, SlashCommandInvocationResult, SlashCommandKind,
    },
    session::Session,
};
use serde::{Deserialize, Serialize};

use crate::{Result, SessionMode, SkillError};

const PROJECT_SKILL_ROOTS: &[&str] = &[
    ".agents/skills",
    ".github/skills",
    ".claude/skills",
    "skills",
];
const MAX_SELECTED_SKILLS: usize = 16;

/// A user-invocable skill discovered by the SDK in the selected project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSkill {
    pub name: String,
    pub description: String,
    pub path: String,
}

/// The exact skill source selected in the composer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSelection {
    pub name: String,
    pub path: String,
}

pub(crate) fn directories(mode: &SessionMode, directory: &Path) -> Vec<PathBuf> {
    match mode {
        SessionMode::Chat => Vec::new(),
        SessionMode::Work { .. } => PROJECT_SKILL_ROOTS
            .iter()
            .map(|root| directory.join(root))
            .collect(),
    }
}

pub(crate) async fn discover(client: &Client, directory: &Path) -> Result<Vec<ProjectSkill>> {
    let roots = PROJECT_SKILL_ROOTS
        .iter()
        .map(|root| directory.join(root).to_string_lossy().into_owned())
        .collect();
    let discovered = client
        .rpc()
        .skills()
        .discover(SkillsDiscoverRequest {
            exclude_host_skills: Some(true),
            project_paths: Some(Vec::new()),
            skill_directories: Some(roots),
            ..Default::default()
        })
        .await?;
    if let Some(errors) = discovered.errors
        && !errors.is_empty()
    {
        return Err(SkillError::InvalidDefinitions { errors }.into());
    }
    let mut catalog = Vec::new();
    for skill in discovered.skills {
        if skill.enabled
            && skill.user_invocable
            && let Some(path) = project_skill_path(directory, skill.path.as_deref()).await
        {
            catalog.push(ProjectSkill {
                name: skill.name,
                description: skill.description,
                path,
            });
        }
    }
    catalog.sort_by(|left, right| left.name.cmp(&right.name).then(left.path.cmp(&right.path)));
    catalog.dedup_by(|left, right| left.name == right.name && left.path == right.path);
    Ok(catalog)
}

async fn project_skill_path(directory: &Path, candidate: Option<&str>) -> Option<String> {
    let candidate = tokio::fs::canonicalize(candidate?).await.ok()?;
    if !tokio::fs::metadata(&candidate).await.ok()?.is_file()
        || candidate.file_name()? != "SKILL.md"
        || !PROJECT_SKILL_ROOTS
            .iter()
            .any(|root| candidate.starts_with(directory.join(root)))
    {
        return None;
    }
    Some(candidate.to_string_lossy().into_owned())
}

pub(crate) async fn restrict_to_project(
    session: &Session,
    directory: &Path,
) -> Result<Vec<(Skill, String)>> {
    session.rpc().skills().ensure_loaded().await?;
    let mut catalog = Vec::new();
    for skill in session.rpc().skills().list().await?.skills {
        if let Some(path) = project_skill_path(directory, skill.path.as_deref()).await {
            catalog.push((skill, path));
        } else if skill.enabled {
            session
                .rpc()
                .skills()
                .disable(SkillsDisableRequest { name: skill.name })
                .await?;
        }
    }
    Ok(catalog)
}

pub(crate) async fn prepare_message(
    session: &Session,
    directory: Option<&Path>,
    prompt: &str,
    selected: &[SkillSelection],
) -> Result<MessageOptions> {
    validate_selection(directory.is_some(), selected)?;
    let Some(directory) = directory else {
        return Ok(MessageOptions::new(prompt));
    };
    session.rpc().skills().reload().await?;
    let catalog = restrict_to_project(session, directory).await?;
    if selected.is_empty() {
        return Ok(MessageOptions::new(prompt));
    }
    let commands = session
        .rpc()
        .commands()
        .list_with_params(CommandsListRequest {
            include_builtins: Some(false),
            include_client_commands: Some(false),
            include_skills: Some(true),
        })
        .await?;
    let mut selected_commands = Vec::new();
    for selection in selected {
        let skill = catalog
            .iter()
            .find(|(skill, path)| {
                skill.enabled
                    && skill.user_invocable
                    && skill.name == selection.name
                    && path == &selection.path
            })
            .map(|(skill, _)| skill)
            .ok_or_else(|| SkillError::Unavailable {
                name: selection.name.clone(),
            })?;
        let command_name = skill.command_name.as_deref().unwrap_or(skill.name.as_str());
        if !commands
            .commands
            .iter()
            .any(|command| command.kind == SlashCommandKind::Skill && command.name == command_name)
        {
            return Err(SkillError::Unavailable {
                name: selection.name.clone(),
            }
            .into());
        }
        selected_commands.push((selection, command_name));
    }
    let mut expanded = Vec::new();
    for (selection, command_name) in selected_commands {
        let invocation = session
            .rpc()
            .commands()
            .invoke(CommandsInvokeRequest {
                name: command_name.to_owned(),
                input: None,
                origin: None,
            })
            .await?;
        let SlashCommandInvocationResult::AgentPrompt(invocation) = invocation else {
            return Err(SkillError::UnsupportedInvocation {
                name: selection.name.clone(),
            }
            .into());
        };
        if invocation.mode.is_some()
            || invocation.notice.is_some()
            || invocation.runtime_settings_changed == Some(true)
        {
            return Err(SkillError::UnsupportedInvocation {
                name: selection.name.clone(),
            }
            .into());
        }
        expanded.push(invocation.prompt);
    }
    expanded.push(prompt.to_owned());
    Ok(MessageOptions::new(expanded.join("\n\n")).with_display_prompt(prompt))
}

fn validate_selection(work: bool, selected: &[SkillSelection]) -> Result<()> {
    if !work && !selected.is_empty() {
        return Err(SkillError::NotAvailableInChat.into());
    }
    if selected.len() > MAX_SELECTED_SKILLS {
        return Err(SkillError::TooManySelections {
            limit: MAX_SELECTED_SKILLS,
        }
        .into());
    }
    for (index, selection) in selected.iter().enumerate() {
        if selected[..index]
            .iter()
            .any(|previous| previous.name == selection.name)
        {
            return Err(SkillError::DuplicateSelection {
                name: selection.name.clone(),
            }
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_selections_require_work_and_have_a_bounded_unique_catalog() {
        let skill = SkillSelection {
            name: "review".into(),
            path: "/project/.agents/skills/review/SKILL.md".into(),
        };
        assert!(validate_selection(false, &[]).is_ok());
        assert!(matches!(
            validate_selection(false, std::slice::from_ref(&skill)),
            Err(crate::Error::Skills(SkillError::NotAvailableInChat))
        ));
        assert!(matches!(
            validate_selection(true, &vec![skill.clone(); MAX_SELECTED_SKILLS + 1]),
            Err(crate::Error::Skills(SkillError::TooManySelections { .. }))
        ));
        assert!(matches!(
            validate_selection(true, &[skill.clone(), skill]),
            Err(crate::Error::Skills(SkillError::DuplicateSelection { .. }))
        ));
    }

    #[test]
    fn chat_has_no_skill_roots_and_work_uses_only_the_selected_directory() {
        let project = PathBuf::from("/project");
        assert_eq!(
            directories(&SessionMode::Chat, &project),
            Vec::<PathBuf>::new()
        );
        let roots = directories(
            &SessionMode::Work {
                directory: project.clone(),
            },
            &project,
        );
        assert_eq!(roots.len(), PROJECT_SKILL_ROOTS.len());
        assert!(roots.iter().all(|root| root.starts_with(&project)));
    }
}
