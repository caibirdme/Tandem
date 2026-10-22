use std::{path::PathBuf, process::Command};

use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    directory: tempfile::TempDir,
    project: PathBuf,
    storage: PathBuf,
}

impl Fixture {
    fn new() -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let project = directory.path().join("project");
        let storage = directory.path().join("review");
        std::fs::create_dir(&project)?;
        let fixture = Self {
            directory,
            project,
            storage,
        };
        fixture.git(&["init", "-q", "--initial-branch=main"])?;
        fixture.git(&["config", "core.autocrlf", "false"])?;
        Ok(fixture)
    }

    fn git(&self, args: &[&str]) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Review Test",
                "-c",
                "user.email=review@example.invalid",
            ])
            .args(args)
            .current_dir(&self.project)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "Fixture Git command failed: {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(output.stdout)
    }

    fn write(&self, path: &str, content: &[u8]) -> std::io::Result<()> {
        let path = self.project.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }

    fn commit(&self) -> std::result::Result<(), Box<dyn std::error::Error>> {
        self.git(&["add", "--all"])?;
        self.git(&["commit", "-qm", "Fixture state"])?;
        Ok(())
    }

    fn inspector(&self) -> WorkspaceReview {
        WorkspaceReview::new(self.project.clone(), self.storage.clone())
    }
}

#[tokio::test]
async fn workspace_keeps_staging_cancellation_and_new_files_visible() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("tracked.txt", b"A\n")?;
    fixture.commit()?;
    fixture.write("tracked.txt", b"B\n")?;
    fixture.git(&["add", "tracked.txt"])?;
    fixture.write("tracked.txt", b"A\n")?;
    fixture.write("new.txt", b"new\n")?;
    let inspector = fixture.inspector();
    let index_before = std::fs::read(fixture.project.join(".git/index"))?;
    let net = inspector.workspace(WorkspaceComparison::Net).await?;
    assert_eq!(net.files_total, 2);
    let files = inspector
        .files(&net.review_id, &net.revision, 0)
        .await?
        .files;
    let cancelled = files
        .iter()
        .find(|file| file.path == "tracked.txt")
        .ok_or("missing cancelled file")?;
    assert_eq!(cancelled.status, FileStatus::Unchanged);
    assert_eq!(cancelled.index_status.as_deref(), Some("M"));
    assert_eq!(cancelled.worktree_status.as_deref(), Some("M"));
    assert_eq!(cancelled.additions, Some(0));
    let staged = inspector.workspace(WorkspaceComparison::Staged).await?;
    assert_eq!(staged.files_total, 1);
    let files = inspector
        .files(&staged.review_id, &staged.revision, 0)
        .await?
        .files;
    assert!(
        files
            .iter()
            .any(|file| file.path == "tracked.txt" && file.additions == Some(1))
    );
    assert_eq!(
        std::fs::read(fixture.project.join(".git/index"))?,
        index_before
    );
    Ok(())
}

#[tokio::test]
async fn workspace_uses_git_eol_rules_but_snapshots_keep_raw_bytes() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(".gitattributes", b"*.txt text eol=crlf\n")?;
    fixture.write("file.txt", b"one\r\ntwo\r\n")?;
    fixture.commit()?;
    let inspector = fixture.inspector();
    let before = inspector.capture(None).await?;
    let status = fixture.git(&["status", "--porcelain"])?;
    assert!(status.is_empty(), "{}", String::from_utf8_lossy(&status));
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    assert_eq!(review.files_total, 0);
    fixture.write("file.txt", b"one\ntwo\n")?;
    let after = inspector.capture(Some(&before.id)).await?;
    let raw = inspector
        .compare(&before.id, &after.id, ReviewSelection::Session)
        .await?;
    assert_eq!(raw.files_total, 1);
    Ok(())
}

#[tokio::test]
async fn workspace_filters_staged_and_unstaged_files() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("staged.txt", b"before\n")?;
    fixture.write("unstaged.txt", b"before\n")?;
    fixture.commit()?;
    fixture.write("staged.txt", b"after\n")?;
    fixture.git(&["add", "staged.txt"])?;
    fixture.write("unstaged.txt", b"after\n")?;
    fixture.write("new.txt", b"new\n")?;
    let inspector = fixture.inspector();
    let staged = inspector.workspace(WorkspaceComparison::Staged).await?;
    assert_eq!(staged.files_total, 1);
    let unstaged = inspector.workspace(WorkspaceComparison::Unstaged).await?;
    let files = inspector
        .files(&unstaged.review_id, &unstaged.revision, 0)
        .await?;
    assert_eq!(files.files.len(), 2);
    assert!(files.files.iter().all(|file| file.path != "staged.txt"));
    Ok(())
}

#[tokio::test]
async fn workspace_never_runs_content_filter_helpers() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("file.txt", b"before\n")?;
    fixture.commit()?;
    fixture.write(".gitattributes", b"*.txt filter=review-test\n")?;
    fixture.git(&[
        "config",
        "filter.review-test.clean",
        "touch helper-ran; cat",
    ])?;
    fixture.write("file.txt", b"after\n")?;
    let inspector = fixture.inspector();
    assert!(matches!(
        inspector.workspace(WorkspaceComparison::Net).await,
        Err(ReviewError::ContentFilters)
    ));
    assert!(!fixture.project.join("helper-ran").exists());
    assert!(inspector.capture(None).await?.complete);
    Ok(())
}

#[tokio::test]
async fn workspace_normalizes_modified_crlf_text_without_extra_changed_lines() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(".gitattributes", b"*.txt text eol=crlf\n")?;
    fixture.write("file.txt", b"one\r\ntwo\r\nthree\r\n")?;
    fixture.commit()?;
    fixture.write("file.txt", b"one\r\nchanged\r\nthree\r\n")?;
    let review = fixture
        .inspector()
        .workspace(WorkspaceComparison::Net)
        .await?;
    assert_eq!(review.additions, Some(1));
    assert_eq!(review.deletions, Some(1));
    Ok(())
}

#[tokio::test]
async fn automatic_eol_rules_preserve_crlf_already_in_the_index() -> TestResult {
    for automatic_attribute in [false, true] {
        let fixture = Fixture::new()?;
        fixture.write("file.txt", b"one\r\ntwo\r\n")?;
        fixture.commit()?;
        if automatic_attribute {
            std::fs::write(
                fixture.project.join(".git/info/attributes"),
                b"*.txt text=auto\n",
            )?;
        } else {
            fixture.git(&["config", "core.autocrlf", "true"])?;
        }
        assert_eq!(fixture.git(&["diff", "--name-only"])?, [] as [u8; 0]);
        let review = fixture
            .inspector()
            .workspace(WorkspaceComparison::Net)
            .await?;
        assert_eq!(review.files_total, 0);
    }
    Ok(())
}

#[tokio::test]
async fn submodule_filters_are_never_executed_and_coverage_is_explicit() -> TestResult {
    let child = Fixture::new()?;
    child.write("file.txt", b"before\n")?;
    child.commit()?;
    let fixture = Fixture::new()?;
    fixture.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        child.project.to_str().ok_or("invalid fixture path")?,
        "sub",
    ])?;
    fixture.commit()?;
    fixture.write("sub/.gitattributes", b"*.txt filter=review-test\n")?;
    fixture.git(&[
        "-C",
        "sub",
        "config",
        "filter.review-test.clean",
        "echo invoked > helper-ran; cat",
    ])?;
    fixture.write("sub/file.txt", b"after!\n")?;
    let review = fixture
        .inspector()
        .workspace(WorkspaceComparison::Net)
        .await?;
    assert!(!fixture.project.join("sub/helper-ran").exists());
    assert_eq!(review.files_total, 0);
    assert!(!review.complete);
    assert!(review.excluded_files > 0);
    Ok(())
}

#[tokio::test]
async fn private_patch_storage_ignores_inherited_binary_attributes() -> TestResult {
    let fixture = Fixture::new()?;
    let initialized = Command::new("git")
        .args(["init", "-q"])
        .current_dir(fixture.directory.path())
        .status()?;
    assert!(initialized.success());
    std::fs::write(
        fixture.directory.path().join(".gitattributes"),
        b"* -diff\n",
    )?;
    fixture.write("file.txt", b"before\n")?;
    fixture.commit()?;
    fixture.write("file.txt", b"after\n")?;
    let review = fixture
        .inspector()
        .workspace(WorkspaceComparison::Net)
        .await?;
    assert_eq!(review.additions, Some(1));
    assert_eq!(review.deletions, Some(1));
    Ok(())
}

#[tokio::test]
async fn snapshots_preserve_dirty_baselines_after_agent_commits() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("source.rs", b"original\n")?;
    fixture.commit()?;
    fixture.write("source.rs", b"user edit\n")?;
    fixture.write("existing.txt", b"user new file\n")?;
    let inspector = fixture.inspector();
    let before = inspector.capture(None).await?;
    fixture.write("source.rs", b"user edit\nagent edit\n")?;
    fixture.write("existing.txt", b"user new file\nagent appended\n")?;
    fixture.commit()?;
    let after = inspector.capture(Some(&before.id)).await?;
    let review = inspector
        .compare(
            &before.id,
            &after.id,
            ReviewSelection::Turn {
                message_id: "turn-one".into(),
            },
        )
        .await?;
    assert_eq!(review.files_total, 2);
    assert_eq!(review.additions, Some(2));
    assert_eq!(review.deletions, Some(0));
    assert_eq!(
        inspector
            .workspace(WorkspaceComparison::Net)
            .await?
            .files_total,
        0
    );
    let reopened = fixture.inspector();
    assert_eq!(
        reopened
            .summary(&review.review_id, &review.revision)
            .await?
            .files_total,
        2
    );
    Ok(())
}

#[tokio::test]
async fn captured_details_do_not_read_later_working_files() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("file.txt", b"before\n")?;
    fixture.commit()?;
    fixture.write("file.txt", b"first\n")?;
    let inspector = fixture.inspector();
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    let file = inspector
        .files(&review.review_id, &review.revision, 0)
        .await?
        .files
        .remove(0);
    fixture.write("file.txt", b"second\n")?;
    let frozen = inspector
        .file(&review.review_id, &review.revision, &file.id)
        .await?;
    assert!(
        frozen
            .patch
            .as_deref()
            .is_some_and(|patch| patch.contains("+first"))
    );
    assert!(matches!(
        inspector.file(&review.review_id, "wrong", &file.id).await,
        Err(ReviewError::RevisionExpired)
    ));
    let newer = inspector.workspace(WorkspaceComparison::Net).await?;
    assert_ne!(newer.revision, review.revision);
    let unchanged = inspector.workspace(WorkspaceComparison::Net).await?;
    assert_eq!(newer.revision, unchanged.revision);
    Ok(())
}

#[tokio::test]
async fn bulk_reviews_defer_details_and_keep_captured_bytes_after_project_removal() -> TestResult {
    let fixture = Fixture::new()?;
    for index in 0..200 {
        fixture.write(&format!("file-{index:03}.txt"), b"before\n")?;
    }
    let inspector = fixture.inspector();
    let before = inspector.capture(None).await?;
    for index in 0..200 {
        fixture.write(&format!("file-{index:03}.txt"), b"after\n")?;
    }
    let after = inspector.capture(Some(&before.id)).await?;
    let (turn, session) = tokio::time::timeout(std::time::Duration::from_secs(6), async {
        let turn = inspector
            .compare(
                &before.id,
                &after.id,
                ReviewSelection::Turn {
                    message_id: "bulk-turn".into(),
                },
            )
            .await?;
        let session = inspector
            .compare(&before.id, &after.id, ReviewSelection::Session)
            .await?;
        Ok::<_, ReviewError>((turn, session))
    })
    .await??;
    for review in [&turn, &session] {
        assert_eq!(review.files_total, 200);
        assert!(review.complete);
        assert_eq!(review.additions, None);
        assert_eq!(review.deletions, None);
        assert!(
            !fixture
                .storage
                .join("reviews")
                .join(&review.review_id)
                .exists()
        );
    }
    let files = inspector
        .files(&turn.review_id, &turn.revision, 0)
        .await?
        .files;
    assert!(
        files
            .iter()
            .all(|file| file.additions.is_none() && file.deletions.is_none())
    );
    fixture.write(&files[0].path, b"later live content\n")?;
    std::fs::rename(
        &fixture.project,
        fixture.directory.path().join("removed-project"),
    )?;
    let reopened = WorkspaceReview::new(inspector.project.clone(), fixture.storage.clone());
    let detail = reopened
        .file(&turn.review_id, &turn.revision, &files[0].id)
        .await?;
    let patch = detail
        .patch
        .as_deref()
        .ok_or("missing captured bulk patch")?;
    assert!(patch.contains("-before") && patch.contains("+after"));
    assert!(!patch.contains("later live content"));
    assert_eq!(detail.file.additions, Some(1));
    assert_eq!(detail.file.deletions, Some(1));
    let cached = reopened
        .file(&turn.review_id, &turn.revision, &files[0].id)
        .await?;
    assert_eq!(serde_json::to_vec(&detail)?, serde_json::to_vec(&cached)?);
    assert_eq!(
        reopened
            .summary(&turn.review_id, &turn.revision)
            .await?
            .additions,
        None
    );
    Ok(())
}

#[tokio::test]
async fn unborn_repository_and_special_names_have_numbered_hunks() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("- spaced 中文.txt", b"first\nsecond")?;
    let inspector = fixture.inspector();
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    assert_eq!(review.files_total, 1);
    let file = inspector
        .files(&review.review_id, &review.revision, 0)
        .await?
        .files
        .remove(0);
    let diff = inspector
        .file(&review.review_id, &review.revision, &file.id)
        .await?;
    assert_eq!(diff.file.status, FileStatus::Added);
    assert_eq!(diff.file.additions, Some(2));
    assert_eq!(diff.hunks[0].lines[0].new_line, Some(1));
    assert!(
        diff.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Note)
    );
    Ok(())
}

#[tokio::test]
async fn project_subdirectory_excludes_sibling_files() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("one/source.txt", b"a\n")?;
    fixture.write("two/source.txt", b"a\n")?;
    fixture.commit()?;
    fixture.write("one/source.txt", b"b\n")?;
    fixture.write("two/source.txt", b"b\n")?;
    let inspector = WorkspaceReview::new(fixture.project.join("one"), fixture.storage.clone());
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    assert_eq!(review.files_total, 1);
    assert_eq!(
        inspector
            .files(&review.review_id, &review.revision, 0)
            .await?
            .files[0]
            .path,
        "source.txt"
    );
    Ok(())
}

#[tokio::test]
async fn binary_large_and_renamed_files_remain_visible() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("old.txt", b"same\n")?;
    fixture.commit()?;
    std::fs::rename(
        fixture.project.join("old.txt"),
        fixture.project.join("renamed.txt"),
    )?;
    fixture.write("image.bin", &[0, 1, 2])?;
    fixture.write("large.txt", &vec![b'x'; MAX_FILE_BYTES + 1])?;
    let inspector = fixture.inspector();
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    let files = inspector
        .files(&review.review_id, &review.revision, 0)
        .await?
        .files;
    assert!(
        files.iter().any(|file| file.status == FileStatus::Renamed
            && file.old_path.as_deref() == Some("old.txt"))
    );
    assert!(
        files
            .iter()
            .any(|file| file.content_kind == ContentKind::Binary)
    );
    assert!(
        files
            .iter()
            .any(|file| file.content_kind == ContentKind::TooLarge)
    );
    assert!(!review.complete);
    Ok(())
}

#[tokio::test]
async fn changed_ignore_rules_do_not_hide_previously_captured_files() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("new.txt", b"before\n")?;
    let inspector = fixture.inspector();
    let before = inspector.capture(None).await?;
    fixture.write(".gitignore", b"new.txt\n")?;
    fixture.write("new.txt", b"after\n")?;
    let after = inspector.capture(Some(&before.id)).await?;
    let review = inspector
        .compare(&before.id, &after.id, ReviewSelection::Session)
        .await?;
    assert!(
        inspector
            .files(&review.review_id, &review.revision, 0)
            .await?
            .files
            .iter()
            .any(|file| file.path == "new.txt")
    );
    Ok(())
}

#[tokio::test]
async fn readers_reject_paths_and_survive_a_removed_project() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("source.txt", b"content\n")?;
    let inspector = fixture.inspector();
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    assert!(matches!(
        inspector.summary("../escape", "x").await,
        Err(ReviewError::InvalidInput)
    ));
    std::fs::rename(&fixture.project, fixture.directory.path().join("moved"))?;
    assert_eq!(
        inspector
            .summary(&review.review_id, &review.revision)
            .await?
            .files_total,
        1
    );
    Ok(())
}

#[tokio::test]
async fn structured_diff_respects_the_serialized_payload_limit() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("file.txt", "a\n".repeat(12_000).as_bytes())?;
    fixture.commit()?;
    fixture.write("file.txt", "b\n".repeat(12_000).as_bytes())?;
    let inspector = fixture.inspector();
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    let file = inspector
        .files(&review.review_id, &review.revision, 0)
        .await?
        .files
        .remove(0);
    let detail = inspector
        .file(&review.review_id, &review.revision, &file.id)
        .await?;
    assert_eq!(detail.file.content_kind, ContentKind::TooLarge);
    assert!(detail.file.additions.is_none());
    assert!(detail.hunks.is_empty());
    assert!(serde_json::to_vec(&detail)?.len() <= MAX_PATCH_BYTES);
    Ok(())
}

#[tokio::test]
async fn duplicate_content_rename_preserves_unpaired_deletions() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("one.txt", b"same\n")?;
    fixture.write("two.txt", b"same\n")?;
    fixture.commit()?;
    std::fs::remove_file(fixture.project.join("one.txt"))?;
    std::fs::rename(
        fixture.project.join("two.txt"),
        fixture.project.join("three.txt"),
    )?;
    let inspector = fixture.inspector();
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    let files = inspector
        .files(&review.review_id, &review.revision, 0)
        .await?
        .files;
    assert_eq!(files.len(), 2);
    assert_eq!(
        files
            .iter()
            .filter(|file| file.status == FileStatus::Renamed)
            .count(),
        1
    );
    assert_eq!(
        files
            .iter()
            .filter(|file| file.status == FileStatus::Deleted)
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn unmerged_index_is_explicitly_unavailable_for_text_diff() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("file.txt", b"before\n")?;
    fixture.commit()?;
    fixture.git(&["checkout", "-qb", "topic"])?;
    fixture.write("file.txt", b"topic\n")?;
    fixture.commit()?;
    fixture.git(&["checkout", "-q", "main"])?;
    fixture.write("file.txt", b"main\n")?;
    fixture.commit()?;
    assert!(fixture.git(&["merge", "--no-edit", "topic"]).is_err());
    let inspector = fixture.inspector();
    let review = inspector.workspace(WorkspaceComparison::Net).await?;
    let file = inspector
        .files(&review.review_id, &review.revision, 0)
        .await?
        .files
        .remove(0);
    assert_eq!(file.status, FileStatus::Conflict);
    assert_eq!(file.content_kind, ContentKind::Conflict);
    assert!(file.additions.is_none());
    assert!(!review.complete);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn snapshots_record_symlink_targets_and_executable_modes() -> TestResult {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let fixture = Fixture::new()?;
    let outside = fixture.directory.path().join("outside");
    std::fs::write(&outside, b"outside before")?;
    fixture.write("script", b"script\n")?;
    std::fs::set_permissions(
        fixture.project.join("script"),
        std::fs::Permissions::from_mode(0o644),
    )?;
    symlink(&outside, fixture.project.join("link"))?;
    let inspector = fixture.inspector();
    let before = inspector.capture(None).await?;
    std::fs::write(&outside, b"outside after")?;
    std::fs::set_permissions(
        fixture.project.join("script"),
        std::fs::Permissions::from_mode(0o755),
    )?;
    let after = inspector.capture(Some(&before.id)).await?;
    let review = inspector
        .compare(&before.id, &after.id, ReviewSelection::Session)
        .await?;
    let files = inspector
        .files(&review.review_id, &review.revision, 0)
        .await?
        .files;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "script");
    assert_eq!(files[0].old_mode.as_deref(), Some("100644"));
    assert_eq!(files[0].new_mode.as_deref(), Some("100755"));
    assert_eq!(files[0].additions, Some(0));
    Ok(())
}
