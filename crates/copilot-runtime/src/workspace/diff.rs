use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::{
    ContentKind, DiffHunk, DiffLine, DiffLineKind, Entry, FileStatus, MAX_PATCH_BYTES, Manifest,
    Result, ReviewError, ReviewFile, ReviewFileDiff, ReviewSelection, ReviewSummary, SavedFile,
    SavedReview, WorkspaceComparison, WorkspaceReview,
    capture::{GitTree, digest},
    git, read_json, save_json, timestamp,
};

impl WorkspaceReview {
    /// Inspect the current workspace without mutating Git or starting an agent.
    ///
    /// # Errors
    /// Returns typed inspection, size, consistency, and persistence failures.
    pub async fn workspace(&self, comparison: WorkspaceComparison) -> Result<ReviewSummary> {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.inspect_workspace(comparison),
        )
        .await
        .map_err(|_| ReviewError::Timeout)?
    }

    async fn inspect_workspace(&self, comparison: WorkspaceComparison) -> Result<ReviewSummary> {
        self.initialize_storage().await?;
        let guard = self.scan_gate().await?;
        let head = self.git_manifest(GitTree::Head).await?;
        let index = self.git_manifest(GitTree::Index).await?;
        let tracked = head
            .entries
            .keys()
            .chain(index.entries.keys())
            .cloned()
            .collect();
        self.check_content_filters(&tracked).await?;
        let current = self.capture_manifest(Some(&index)).await?;
        let mut current = normalize_modes(
            current,
            &index,
            !self.config_bool("core.filemode", true).await?,
            !self.config_bool("core.symlinks", true).await?,
        );
        self.normalize_workspace_eol(&mut current, &index).await?;
        self.check_content_filters(&tracked).await?;
        let confirmed_head = self.git_manifest(GitTree::Head).await?;
        let confirmed_index = self.git_manifest(GitTree::Index).await?;
        if head.entries != confirmed_head.entries || index.entries != confirmed_index.entries {
            return Err(ReviewError::WorkspaceChanged);
        }
        // Nested repositories are outside capture coverage. Preserve known index pointers
        // without claiming an unread child repository was modified.
        for (path, entry) in &mut current.entries {
            if entry.mode == "160000"
                && let Some(index) = index.entries.get(path)
            {
                entry.clone_from(index);
            }
        }
        let keys: BTreeSet<_> = head
            .entries
            .keys()
            .chain(index.entries.keys())
            .chain(current.entries.keys())
            .cloned()
            .collect();
        let inventory = keys
            .into_iter()
            .filter_map(|key| {
                let staged = state(head.entries.get(&key), index.entries.get(&key));
                let unstaged = state(index.entries.get(&key), current.entries.get(&key));
                let visible = match comparison {
                    WorkspaceComparison::Net => staged != "." || unstaged != ".",
                    WorkspaceComparison::Staged => staged != ".",
                    WorkspaceComparison::Unstaged => unstaged != ".",
                };
                visible.then_some((key, (staged, unstaged)))
            })
            .collect();
        drop(guard);
        let (before, after) = match comparison {
            WorkspaceComparison::Net => (&head, &current),
            WorkspaceComparison::Staged => (&head, &index),
            WorkspaceComparison::Unstaged => (&index, &current),
        };
        self.build_review(
            before,
            after,
            ReviewSelection::Workspace { comparison },
            Some(&inventory),
        )
        .await
    }

    /// Compare two captured working states, independent of later commits or edits.
    ///
    /// # Errors
    /// Returns typed inspection, size, identifier, and persistence failures.
    pub async fn compare(
        &self,
        before: &str,
        after: &str,
        selection: ReviewSelection,
    ) -> Result<ReviewSummary> {
        if matches!(selection, ReviewSelection::Workspace { .. }) {
            return Err(ReviewError::InvalidInput);
        }
        let before = self.manifest(before).await?;
        let after = self.manifest(after).await?;
        self.build_review(&before, &after, selection, None).await
    }

    async fn build_review(
        &self,
        before: &Manifest,
        after: &Manifest,
        selection: ReviewSelection,
        inventory: Option<&BTreeMap<String, (String, String)>>,
    ) -> Result<ReviewSummary> {
        let identity = serde_json::to_string(&(
            &before.project,
            &selection,
            &before.entries,
            &after.entries,
            inventory,
            before.snapshot.complete,
            after.snapshot.complete,
        ))?;
        let directory = self.storage.join("reviews");
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(ReviewError::Storage)?;
        let prefix = digest(identity.as_bytes());
        let mut review_id = String::new();
        for suffix in 0..1000 {
            let candidate = format!("review-{prefix}-{suffix}");
            match read_json::<SavedReview>(&directory.join(format!("{candidate}.json"))).await {
                Ok(existing) if existing.identity == identity => return Ok(existing.summary),
                Ok(_) => {}
                Err(ReviewError::NotFound) => {
                    review_id = candidate;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if review_id.is_empty() {
            return Err(ReviewError::Limit);
        }
        let files = self
            .build_files(&review_id, before, after, inventory)
            .await?;
        let additions = files.iter().try_fold(0u64, |count, file| {
            file.metadata
                .additions
                .map(|value| count.saturating_add(value))
        });
        let deletions = files.iter().try_fold(0u64, |count, file| {
            file.metadata
                .deletions
                .map(|value| count.saturating_add(value))
        });
        let excluded_files = before
            .snapshot
            .excluded_files
            .saturating_add(after.snapshot.excluded_files);
        let summary = ReviewSummary {
            review_id: review_id.clone(),
            revision: review_id.clone(),
            workspace_path: before.project.clone(),
            repository_path: before.repository.clone(),
            selection,
            captured_at: timestamp(),
            files_total: files.len(),
            additions,
            deletions,
            complete: before.snapshot.complete
                && after.snapshot.complete
                && files.iter().all(|file| {
                    !matches!(
                        file.metadata.content_kind,
                        ContentKind::TooLarge | ContentKind::Unsupported | ContentKind::Conflict
                    )
                }),
            excluded_files,
        };
        save_json(
            &directory.join(format!("{review_id}.json")),
            &SavedReview {
                identity,
                summary: summary.clone(),
                files,
            },
        )
        .await?;
        Ok(summary)
    }

    async fn build_files(
        &self,
        review_id: &str,
        before: &Manifest,
        after: &Manifest,
        inventory: Option<&BTreeMap<String, (String, String)>>,
    ) -> Result<Vec<SavedFile>> {
        let keys: BTreeSet<_> = if let Some(inventory) = inventory {
            inventory.keys().cloned().collect()
        } else {
            before
                .entries
                .keys()
                .chain(after.entries.keys())
                .filter(|key| !equal(before.entries.get(*key), after.entries.get(*key)))
                .cloned()
                .collect()
        };
        let (consumed, renames) = pair_renames(&keys, before, after);
        let eager = keys.len().saturating_sub(consumed.len()) <= 10;
        let mut files = Vec::new();
        let mut workers = tokio::task::JoinSet::new();
        for (ordinal, key) in keys.iter().enumerate() {
            if consumed.contains(key) {
                continue;
            }
            let old = before.entries.get(renames.get(key).unwrap_or(key));
            let new = after.entries.get(key);
            let flags = inventory.and_then(|inventory| inventory.get(key));
            let mut metadata =
                file_metadata(ordinal, key, old, new, flags, renames.contains_key(key))?;
            if metadata.content_kind == ContentKind::Text
                && old.and_then(|entry| entry.object.as_ref())
                    == new.and_then(|entry| entry.object.as_ref())
            {
                metadata.additions = Some(0);
                metadata.deletions = Some(0);
            }
            let saved = SavedFile {
                metadata,
                before: old.cloned(),
                after: new.cloned(),
            };
            if !eager {
                files.push(saved);
                continue;
            }
            let inspector = self.clone();
            let review_id = review_id.to_owned();
            workers.spawn(async move { inspector.resolve_saved_file(review_id, saved).await });
            if workers.len() >= 8 {
                files.push(
                    workers
                        .join_next()
                        .await
                        .ok_or(ReviewError::CoordinatorUnavailable)?
                        .map_err(ReviewError::Worker)??,
                );
            }
        }
        while let Some(file) = workers.join_next().await {
            files.push(file.map_err(ReviewError::Worker)??);
        }
        files.sort_by(|left, right| left.metadata.path.cmp(&right.metadata.path));
        Ok(files)
    }

    async fn resolve_saved_file(
        &self,
        review_id: String,
        mut saved: SavedFile,
    ) -> Result<SavedFile> {
        let detail = self
            .build_file_detail(
                review_id,
                saved.metadata.clone(),
                saved.before.clone(),
                saved.after.clone(),
            )
            .await?;
        saved.metadata = detail.file;
        Ok(saved)
    }

    pub(super) async fn build_file_detail(
        &self,
        review_id: String,
        mut file: ReviewFile,
        old: Option<Entry>,
        new: Option<Entry>,
    ) -> Result<ReviewFileDiff> {
        let (patch, hunks) = self
            .make_diff(&mut file, old.as_ref(), new.as_ref())
            .await?;
        let detail = ReviewFileDiff {
            review_id: review_id.clone(),
            revision: review_id.clone(),
            file: file.clone(),
            patch,
            hunks,
        };
        save_json(
            &self
                .storage
                .join("reviews")
                .join(review_id)
                .join(format!("{}.json", file.id)),
            &detail,
        )
        .await?;
        Ok(detail)
    }

    async fn normalize_workspace_eol(
        &self,
        current: &mut Manifest,
        index: &Manifest,
    ) -> Result<()> {
        let paths = self.eol_paths(current.entries.keys()).await?;
        for (key, automatic) in paths {
            if automatic && let Some(index) = index.entries.get(&key) {
                let indexed = self.object(Some(index)).await?;
                if indexed.windows(2).any(|bytes| bytes == b"\r\n") {
                    continue;
                }
            }
            let Some(entry) = current
                .entries
                .get_mut(&key)
                .filter(|entry| entry.kind == ContentKind::Text && entry.mode.starts_with("100"))
            else {
                continue;
            };
            let bytes = self.object(Some(entry)).await?;
            if !bytes.windows(2).any(|bytes| bytes == b"\r\n") {
                continue;
            }
            let normalized: Vec<_> = bytes
                .iter()
                .enumerate()
                .filter(|(index, byte)| **byte != b'\r' || bytes.get(index + 1) != Some(&b'\n'))
                .map(|(_, byte)| *byte)
                .collect();
            entry.object = Some(self.save_object(&normalized).await?);
            entry.size = u64::try_from(normalized.len()).unwrap_or(u64::MAX);
        }
        Ok(())
    }

    async fn make_diff(
        &self,
        file: &mut ReviewFile,
        old: Option<&Entry>,
        new: Option<&Entry>,
    ) -> Result<(Option<String>, Vec<DiffHunk>)> {
        if file.content_kind != ContentKind::Text {
            return Ok((None, Vec::new()));
        }
        let original = self.object(old).await?;
        let modified = self.object(new).await?;
        if original == modified {
            file.additions = Some(0);
            file.deletions = Some(0);
            return Ok((None, Vec::new()));
        }
        let directory = tempfile::Builder::new()
            .prefix("diff-")
            .tempdir_in(&self.storage)
            .map_err(ReviewError::Storage)?;
        let original_path = directory.path().join("before");
        let modified_path = directory.path().join("after");
        tokio::fs::write(&original_path, original)
            .await
            .map_err(ReviewError::Storage)?;
        tokio::fs::write(&modified_path, modified)
            .await
            .map_err(ReviewError::Storage)?;
        let output = git::run(
            directory.path(),
            &[
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.attributesFile=",
                "diff",
                "--no-index",
                "--text",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                "--unified=3",
                "--",
                "before",
                "after",
            ],
            "comparing captured files",
            true,
            MAX_PATCH_BYTES,
        )
        .await;
        let output = match output {
            Ok(output) => output,
            Err(ReviewError::Limit) => {
                file.content_kind = ContentKind::TooLarge;
                return Ok((None, Vec::new()));
            }
            Err(error) => return Err(error),
        };
        let raw = String::from_utf8(output.bytes).map_err(|_| ReviewError::InvalidGitOutput)?;
        let patch = rewrite_headers(&raw, old, new, file);
        let hunks = parse_hunks(&patch)?;
        file.additions = Some(
            hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == DiffLineKind::Addition)
                .count()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        file.deletions = Some(
            hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.kind == DiffLineKind::Deletion)
                .count()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        let bounded = ReviewFileDiff {
            review_id: "r".repeat(100),
            revision: "r".repeat(100),
            file: file.clone(),
            patch: Some(patch.clone()),
            hunks: hunks.clone(),
        };
        if serde_json::to_vec(&bounded)?.len() > MAX_PATCH_BYTES {
            file.content_kind = ContentKind::TooLarge;
            file.additions = None;
            file.deletions = None;
            return Ok((None, Vec::new()));
        }
        Ok((Some(patch), hunks))
    }
}

fn pair_renames(
    keys: &BTreeSet<String>,
    before: &Manifest,
    after: &Manifest,
) -> (BTreeSet<String>, BTreeMap<String, String>) {
    let mut sources: BTreeMap<(&str, &str), Vec<&String>> = BTreeMap::new();
    for (key, entry) in before.entries.iter().rev() {
        if keys.contains(key)
            && !after.entries.contains_key(key)
            && let Some(object) = &entry.object
        {
            sources.entry((object, &entry.mode)).or_default().push(key);
        }
    }
    let mut consumed = BTreeSet::new();
    let mut renames = BTreeMap::new();
    for target in keys {
        if before.entries.contains_key(target) {
            continue;
        }
        if let Some(new) = after.entries.get(target)
            && let Some(object) = &new.object
            && let Some(candidates) = sources.get_mut(&(object.as_str(), new.mode.as_str()))
            && let Some(source) = candidates.pop()
        {
            consumed.insert(source.clone());
            renames.insert(target.clone(), source.clone());
        }
    }
    (consumed, renames)
}

fn file_metadata(
    ordinal: usize,
    key: &str,
    old: Option<&Entry>,
    new: Option<&Entry>,
    flags: Option<&(String, String)>,
    renamed: bool,
) -> Result<ReviewFile> {
    let decoded = STANDARD
        .decode(key)
        .map_err(|_| ReviewError::InvalidInput)?;
    let path = new.or(old).map_or_else(
        || String::from_utf8_lossy(&decoded).into_owned(),
        |entry| entry.path.clone(),
    );
    let conflict = flags.is_some_and(|(index, worktree)| index == "U" || worktree == "U")
        || old
            .into_iter()
            .chain(new)
            .any(|entry| entry.kind == ContentKind::Conflict);
    Ok(ReviewFile {
        id: format!("file-{ordinal}-{}", digest(key.as_bytes())),
        path,
        old_path: if renamed {
            old.map(|entry| entry.path.clone())
        } else {
            None
        },
        status: if conflict {
            FileStatus::Conflict
        } else if renamed {
            FileStatus::Renamed
        } else {
            status(old, new)
        },
        index_status: flags.map(|(index, _)| index.clone()),
        worktree_status: flags.map(|(_, worktree)| worktree.clone()),
        content_kind: if conflict {
            ContentKind::Conflict
        } else {
            combined_kind(old, new)
        },
        additions: None,
        deletions: None,
        old_mode: old.map(|entry| entry.mode.clone()),
        new_mode: new.map(|entry| entry.mode.clone()),
    })
}

fn equal(old: Option<&Entry>, new: Option<&Entry>) -> bool {
    match (old, new) {
        (None, None) => true,
        (Some(old), Some(new)) => {
            old.object.is_some()
                && old.object == new.object
                && old.mode == new.mode
                && old.kind == new.kind
        }
        _ => false,
    }
}

fn status(old: Option<&Entry>, new: Option<&Entry>) -> FileStatus {
    match (old, new) {
        (None, Some(_)) => FileStatus::Added,
        (Some(_), None) => FileStatus::Deleted,
        (Some(old), Some(new)) if old.mode.get(..3) != new.mode.get(..3) => FileStatus::TypeChanged,
        _ if equal(old, new) => FileStatus::Unchanged,
        _ => FileStatus::Modified,
    }
}

fn normalize_modes(
    mut current: Manifest,
    index: &Manifest,
    ignore_mode: bool,
    symlink_files: bool,
) -> Manifest {
    for (path, entry) in &mut current.entries {
        if let Some(index) = index.entries.get(path) {
            let executable_mode = (ignore_mode || cfg!(windows))
                && entry.mode.starts_with("100")
                && index.mode.starts_with("100");
            let symlink_mode = symlink_files && entry.mode == "100644" && index.mode == "120000";
            if executable_mode || symlink_mode {
                entry.mode.clone_from(&index.mode);
            }
        }
    }
    current
}

fn state(old: Option<&Entry>, new: Option<&Entry>) -> String {
    if old
        .into_iter()
        .chain(new)
        .any(|entry| entry.kind == ContentKind::Conflict)
    {
        return "U".into();
    }
    match status(old, new) {
        FileStatus::Added => "A",
        FileStatus::Deleted => "D",
        FileStatus::TypeChanged => "T",
        FileStatus::Unchanged => ".",
        _ => "M",
    }
    .into()
}

fn combined_kind(old: Option<&Entry>, new: Option<&Entry>) -> ContentKind {
    [
        ContentKind::Conflict,
        ContentKind::Unsupported,
        ContentKind::TooLarge,
        ContentKind::Binary,
    ]
    .into_iter()
    .find(|kind| old.into_iter().chain(new).any(|entry| entry.kind == *kind))
    .unwrap_or(ContentKind::Text)
}

fn rewrite_headers(
    raw: &str,
    old: Option<&Entry>,
    new: Option<&Entry>,
    file: &ReviewFile,
) -> String {
    let source = format!("a/{}", file.old_path.as_ref().unwrap_or(&file.path));
    let destination = format!("b/{}", file.path);
    let mut patch = format!("diff --git {source:?} {destination:?}\n");
    if old.is_none() {
        let _ = writeln!(
            patch,
            "new file mode {}",
            file.new_mode.as_deref().unwrap_or("100644")
        );
    }
    if new.is_none() {
        let _ = writeln!(
            patch,
            "deleted file mode {}",
            file.old_mode.as_deref().unwrap_or("100644")
        );
    }
    let _ = writeln!(
        patch,
        "--- {}\n+++ {}",
        if old.is_some() {
            format!("{source:?}")
        } else {
            "/dev/null".into()
        },
        if new.is_some() {
            format!("{destination:?}")
        } else {
            "/dev/null".into()
        }
    );
    if let Some(start) = raw.find("@@ ") {
        patch.push_str(&raw[start..]);
    }
    patch
}

fn parse_hunks(patch: &str) -> Result<Vec<DiffHunk>> {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut old_line = 0u64;
    let mut new_line = 0u64;
    for line in patch.split_terminator('\n') {
        if line.starts_with("@@ ") {
            let fields: Vec<_> = line.split_whitespace().collect();
            let old = fields
                .get(1)
                .and_then(|range| range.strip_prefix('-'))
                .and_then(|range| range.split(',').next())
                .and_then(|number| number.parse().ok())
                .ok_or(ReviewError::InvalidGitOutput)?;
            let new = fields
                .get(2)
                .and_then(|range| range.strip_prefix('+'))
                .and_then(|range| range.split(',').next())
                .and_then(|number| number.parse().ok())
                .ok_or(ReviewError::InvalidGitOutput)?;
            old_line = old;
            new_line = new;
            hunks.push(DiffHunk {
                old_start: old,
                new_start: new,
                header: line.into(),
                lines: Vec::new(),
            });
        } else if let Some(hunk) = hunks.last_mut() {
            let (kind, old_number, new_number) = match line.as_bytes().first() {
                Some(b' ') => {
                    let result = (DiffLineKind::Context, Some(old_line), Some(new_line));
                    old_line += 1;
                    new_line += 1;
                    result
                }
                Some(b'+') => {
                    let result = (DiffLineKind::Addition, None, Some(new_line));
                    new_line += 1;
                    result
                }
                Some(b'-') => {
                    let result = (DiffLineKind::Deletion, Some(old_line), None);
                    old_line += 1;
                    result
                }
                Some(b'\\') => (DiffLineKind::Note, None, None),
                _ => return Err(ReviewError::InvalidGitOutput),
            };
            hunk.lines.push(DiffLine {
                kind,
                old_line: old_number,
                new_line: new_number,
                text: line.get(1..).unwrap_or_default().into(),
            });
        }
    }
    Ok(hunks)
}
