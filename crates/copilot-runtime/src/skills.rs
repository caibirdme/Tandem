//! Skill discovery from explicit agent roots and invocation through the pinned SDK.

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

const SKILL_ROOT: &str = ".agents/skills";
const MAX_SELECTED_SKILLS: usize = 16;

/// A user-invocable skill discovered in the project or user's agent skill root.
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
        SessionMode::Work { .. } => skill_roots(directory, std::env::home_dir().as_deref()),
    }
}

fn work_directories(directory: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<_> = skill_roots(directory, std::env::home_dir().as_deref())
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .filter(|root| root.is_dir())
        .collect();
    roots.dedup();
    roots
}

fn skill_roots(directory: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![directory.join(SKILL_ROOT)];
    if let Some(home) = home {
        let root = home.join(SKILL_ROOT);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

pub(crate) async fn discover(client: &Client, directory: &Path) -> Vec<ProjectSkill> {
    let roots = work_directories(directory);
    if roots.is_empty() {
        return Vec::new();
    }
    let Ok(discovered) = client
        .rpc()
        .skills()
        .discover(SkillsDiscoverRequest {
            exclude_host_skills: Some(true),
            project_paths: Some(Vec::new()),
            skill_directories: Some(
                roots
                    .iter()
                    .map(|root| root.to_string_lossy().into_owned())
                    .collect(),
            ),
            ..Default::default()
        })
        .await
    else {
        return Vec::new();
    };
    let mut catalog = Vec::new();
    for skill in discovered.skills {
        if skill.enabled
            && skill.user_invocable
            && let Some(path) = skill_path(&roots, skill.path.as_deref()).await
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
    catalog
}

async fn skill_path(roots: &[PathBuf], candidate: Option<&str>) -> Option<String> {
    let candidate = tokio::fs::canonicalize(candidate?).await.ok()?;
    if !tokio::fs::metadata(&candidate).await.ok()?.is_file()
        || candidate.file_name()? != "SKILL.md"
        || !roots.iter().any(|root| candidate.starts_with(root))
    {
        return None;
    }
    Some(candidate.to_string_lossy().into_owned())
}

pub(crate) async fn restrict_to_roots(
    session: &Session,
    directory: &Path,
) -> Result<Vec<(Skill, String)>> {
    session.rpc().skills().ensure_loaded().await?;
    let roots = work_directories(directory);
    let mut catalog = Vec::new();
    for skill in session.rpc().skills().list().await?.skills {
        if let Some(path) = skill_path(&roots, skill.path.as_deref()).await {
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
    let catalog = restrict_to_roots(session, directory).await?;
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> std::io::Result<Self> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let directory = std::env::temp_dir().join(format!(
                "tandem-skill-roots-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory)?;
            Ok(Self(directory))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
    fn chat_has_no_skill_roots_and_work_uses_only_agent_roots() {
        let project = PathBuf::from("/project");
        assert_eq!(
            directories(&SessionMode::Chat, &project),
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            skill_roots(&project, Some(Path::new("/home/user"))),
            vec![
                project.join(SKILL_ROOT),
                PathBuf::from("/home/user").join(SKILL_ROOT)
            ]
        );
        assert_eq!(skill_roots(&project, None), vec![project.join(SKILL_ROOT)]);
        assert_eq!(
            skill_roots(&project, Some(&project)),
            vec![project.join(SKILL_ROOT)]
        );
    }

    #[tokio::test]
    async fn skill_paths_stay_inside_allowed_roots()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let temporary = TestDirectory::new()?;
        let project = temporary.0.join("project");
        let home = temporary.0.join("home");
        let roots = skill_roots(&project, Some(&home));
        for root in &roots {
            std::fs::create_dir_all(root)?;
        }
        let roots: Vec<_> = roots
            .iter()
            .map(|root| root.canonicalize())
            .collect::<std::io::Result<_>>()?;
        for (path, accepted) in [
            (project.join(".agents/skills/review/SKILL.md"), true),
            (home.join(".agents/skills/review/SKILL.md"), true),
            (project.join(".github/skills/review/SKILL.md"), false),
            (project.join(".claude/skills/review/SKILL.md"), false),
            (project.join("skills/review/SKILL.md"), false),
            (home.join(".copilot/skills/review/SKILL.md"), false),
            (project.join(".agents/skills-other/review/SKILL.md"), false),
            (project.join(".agents/skills/review/README.md"), false),
        ] {
            std::fs::create_dir_all(path.parent().ok_or("skill path has no parent")?)?;
            std::fs::write(&path, "skill fixture")?;
            assert_eq!(
                skill_path(&roots, path.to_str()).await.is_some(),
                accepted,
                "{}",
                path.display()
            );
        }
        assert!(skill_path(&roots, None).await.is_none());
        assert!(
            skill_path(
                &roots,
                project.join(".agents/skills/missing/SKILL.md").to_str()
            )
            .await
            .is_none()
        );
        let directory = project.join(".agents/skills/directory/SKILL.md");
        std::fs::create_dir_all(&directory)?;
        assert!(skill_path(&roots, directory.to_str()).await.is_none());
        #[cfg(unix)]
        {
            let link = project.join(".agents/skills/escape");
            std::os::unix::fs::symlink(project.join(".github/skills/review"), &link)?;
            assert!(
                skill_path(&roots, link.join("SKILL.md").to_str())
                    .await
                    .is_none()
            );
        }
        Ok(())
    }
}
