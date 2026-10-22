use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    hash::{DefaultHasher, Hash, Hasher},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::io::AsyncReadExt;

use super::{
    ContentKind, Entry, MAX_CAPTURE_BYTES, MAX_FILE_BYTES, MAX_FILES, Manifest, Result,
    ReviewError, Snapshot, WorkspaceReview, git, save_json, timestamp,
};

#[derive(Clone, Copy)]
pub(super) enum GitTree {
    Head,
    Index,
}

impl WorkspaceReview {
    /// Capture actual working files, including paths from a previous snapshot.
    ///
    /// # Errors
    /// Returns a typed failure for unavailable Git, invalid paths, or inaccessible storage.
    pub async fn capture(&self, previous_snapshot: Option<&str>) -> Result<Snapshot> {
        self.initialize_storage().await?;
        let _guard = self.scan_gate().await?;
        let previous = if let Some(id) = previous_snapshot {
            Some(self.manifest(id).await?)
        } else {
            None
        };
        let manifest = self.capture_manifest(previous.as_ref()).await?;
        let snapshot = manifest.snapshot.clone();
        save_json(
            &self
                .storage
                .join("snapshots")
                .join(format!("{}.json", snapshot.id)),
            &manifest,
        )
        .await?;
        Ok(snapshot)
    }

    pub(super) async fn roots(&self) -> Result<(PathBuf, PathBuf)> {
        let project = tokio::fs::canonicalize(&self.project)
            .await
            .map_err(ReviewError::WorkspaceUnavailable)?;
        let output = git::run(
            &project,
            &["rev-parse", "--show-toplevel"],
            "discovering the repository",
            false,
            32 * 1024,
        )
        .await
        .map_err(|error| {
            if matches!(error, ReviewError::GitFailed { .. }) {
                ReviewError::NotRepository
            } else {
                error
            }
        })?;
        let root = output.bytes.strip_suffix(b"\n").unwrap_or(&output.bytes);
        let repository = tokio::fs::canonicalize(path_from_bytes(root)?)
            .await
            .map_err(ReviewError::WorkspaceUnavailable)?;
        if !project.starts_with(&repository) {
            return Err(ReviewError::InvalidInput);
        }
        Ok((project, repository))
    }

    pub(super) async fn capture_manifest(&self, previous: Option<&Manifest>) -> Result<Manifest> {
        let started = Instant::now();
        let (project, repository) = self.roots().await?;
        let output = git::run(
            &project,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ".",
            ],
            "listing workspace files",
            false,
            git::MAX_OUTPUT,
        )
        .await?;
        let mut paths: BTreeSet<String> = output
            .bytes
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| STANDARD.encode(path))
            .collect();
        if let Some(previous) = previous {
            paths.extend(previous.entries.keys().cloned());
        }
        let mut entries = BTreeMap::new();
        let mut total = 0usize;
        let mut excluded = 0usize;
        for (index, encoded) in paths.into_iter().enumerate() {
            let bytes = STANDARD
                .decode(&encoded)
                .map_err(|_| ReviewError::InvalidInput)?;
            let relative = path_from_bytes(&bytes)?;
            validate_relative(&relative)?;
            let display = String::from_utf8_lossy(&bytes).into_owned();
            if index >= MAX_FILES {
                return Err(ReviewError::Limit);
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err(ReviewError::Timeout);
            }
            let path = project.join(&relative);
            let result = self
                .read_working_entry(&project, &path, display, &mut total)
                .await?;
            if let Some(entry) = result {
                if matches!(entry.kind, ContentKind::TooLarge | ContentKind::Unsupported) {
                    excluded += 1;
                }
                entries.insert(encoded, entry);
            }
        }
        let id = unique_id(&self.storage, "snapshot").await?;
        Ok(Manifest {
            snapshot: Snapshot {
                id,
                captured_at: timestamp(),
                complete: excluded == 0,
                excluded_files: excluded,
            },
            project: project.to_string_lossy().into_owned(),
            repository: repository.to_string_lossy().into_owned(),
            entries,
        })
    }

    async fn read_working_entry(
        &self,
        root: &Path,
        path: &Path,
        display: String,
        total: &mut usize,
    ) -> Result<Option<Entry>> {
        let metadata = match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Ok(Some(unsupported_entry(display, "000000", 0))),
        };
        let Some(parent) = path.parent() else {
            return Err(ReviewError::InvalidInput);
        };
        let Ok(canonical_parent) = tokio::fs::canonicalize(parent).await else {
            return Ok(Some(unsupported_entry(display, "000000", metadata.len())));
        };
        if !canonical_parent.starts_with(root) {
            return Ok(Some(unsupported_entry(display, "000000", metadata.len())));
        }
        let mode = file_mode(&metadata);
        if !metadata.is_file() && !metadata.file_type().is_symlink() {
            return Ok(Some(unsupported_entry(display, &mode, metadata.len())));
        }
        if metadata.len() > u64::try_from(MAX_FILE_BYTES).unwrap_or(u64::MAX)
            || total.saturating_add(usize::try_from(metadata.len()).unwrap_or(usize::MAX))
                > MAX_CAPTURE_BYTES
        {
            return Ok(Some(Entry {
                path: display,
                object: None,
                mode,
                kind: ContentKind::TooLarge,
                size: metadata.len(),
            }));
        }
        let bytes = if metadata.file_type().is_symlink() {
            match tokio::fs::read_link(path).await {
                Ok(target) => os_bytes(target.as_os_str()),
                Err(_) => return Ok(Some(unsupported_entry(display, &mode, metadata.len()))),
            }
        } else {
            let canonical = match tokio::fs::canonicalize(path).await {
                Ok(canonical) if canonical.starts_with(root) => canonical,
                _ => return Ok(Some(unsupported_entry(display, &mode, metadata.len()))),
            };
            let mut bytes = Vec::new();
            let Ok(file) = tokio::fs::File::open(&canonical).await else {
                return Ok(Some(unsupported_entry(display, &mode, metadata.len())));
            };
            file.take(u64::try_from(MAX_FILE_BYTES + 1).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)
                .await
                .map_err(ReviewError::Storage)?;
            let after = tokio::fs::symlink_metadata(path)
                .await
                .map_err(ReviewError::WorkspaceUnavailable)?;
            if after.len() != metadata.len()
                || after.modified().ok() != metadata.modified().ok()
                || after.file_type().is_symlink()
            {
                return Ok(Some(unsupported_entry(display, &mode, after.len())));
            }
            bytes
        };
        if bytes.len() > MAX_FILE_BYTES {
            return Ok(Some(Entry {
                path: display,
                object: None,
                mode,
                kind: ContentKind::TooLarge,
                size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            }));
        }
        *total = total.saturating_add(bytes.len());
        let kind = content_kind(&bytes);
        let object = self.save_object(&bytes).await?;
        Ok(Some(Entry {
            path: display,
            object: Some(object),
            mode,
            kind,
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        }))
    }

    pub(super) async fn git_manifest(&self, tree: GitTree) -> Result<Manifest> {
        let (project, repository) = self.roots().await?;
        let output = read_git_tree(&project, tree).await?;
        let mut entries = BTreeMap::new();
        let mut excluded = 0usize;
        let mut pending = Vec::new();
        for record in output
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            if entries.len() >= MAX_FILES {
                return Err(ReviewError::Limit);
            }
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or(ReviewError::InvalidGitOutput)?;
            let header =
                std::str::from_utf8(&record[..tab]).map_err(|_| ReviewError::InvalidGitOutput)?;
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3 {
                return Err(ReviewError::InvalidGitOutput);
            }
            let (mode, oid, conflict) = match tree {
                GitTree::Head => (fields[0], fields[2], false),
                GitTree::Index => (fields[0], fields[1], fields[2] != "0"),
            };
            let path = &record[tab + 1..];
            validate_relative(&path_from_bytes(path)?)?;
            let display = String::from_utf8_lossy(path).into_owned();
            let encoded = STANDARD.encode(path);
            let entry = if conflict {
                Entry {
                    path: display,
                    object: None,
                    mode: mode.into(),
                    kind: ContentKind::Conflict,
                    size: 0,
                }
            } else if mode == "160000" {
                let object = self.save_object(oid.as_bytes()).await?;
                Entry {
                    path: display,
                    object: Some(object),
                    mode: mode.into(),
                    kind: ContentKind::Unsupported,
                    size: 0,
                }
            } else {
                if !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(ReviewError::InvalidGitOutput);
                }
                pending.push((encoded.clone(), oid.to_owned()));
                Entry {
                    path: display,
                    object: None,
                    mode: mode.into(),
                    kind: ContentKind::Unsupported,
                    size: 0,
                }
            };
            entries.insert(encoded, entry);
        }
        let objects = self
            .git_objects(
                &project,
                pending.iter().map(|(_, oid)| oid.clone()).collect(),
            )
            .await?;
        for (path, oid) in pending {
            if let Some(entry) = entries.get_mut(&path)
                && let Some(object) = objects.get(&oid)
            {
                entry.object.clone_from(&object.object);
                entry.kind = object.kind;
                entry.size = object.size;
            }
        }
        excluded += entries
            .values()
            .filter(|entry| {
                matches!(
                    entry.kind,
                    ContentKind::Unsupported | ContentKind::TooLarge | ContentKind::Conflict
                )
            })
            .count();
        Ok(Manifest {
            snapshot: Snapshot {
                id: String::new(),
                captured_at: timestamp(),
                complete: excluded == 0,
                excluded_files: excluded,
            },
            project: project.to_string_lossy().into_owned(),
            repository: repository.to_string_lossy().into_owned(),
            entries,
        })
    }

    async fn git_objects(
        &self,
        project: &Path,
        oids: BTreeSet<String>,
    ) -> Result<BTreeMap<String, Entry>> {
        let mut objects = BTreeMap::new();
        let mut missing = BTreeSet::new();
        for oid in oids {
            let cached = self.storage.join("git-objects").join(format!("{oid}.json"));
            match super::read_json::<Entry>(&cached).await {
                Ok(entry) => {
                    objects.insert(oid, entry);
                }
                Err(ReviewError::NotFound) => {
                    missing.insert(oid);
                }
                Err(error) => return Err(error),
            }
        }
        if missing.is_empty() {
            return Ok(objects);
        }
        let request = format!("{}\n", missing.into_iter().collect::<Vec<_>>().join("\n"));
        let sizes = git::run_input(
            project,
            &[
                "cat-file",
                "--batch-check=%(objectname) %(objecttype) %(objectsize)",
            ],
            "reading Git object sizes",
            false,
            git::MAX_OUTPUT,
            Some(request.as_bytes()),
        )
        .await?;
        let sizes = std::str::from_utf8(&sizes.bytes).map_err(|_| ReviewError::InvalidGitOutput)?;
        let mut fetch = Vec::new();
        let mut total = 0usize;
        for line in sizes.lines() {
            let fields: Vec<_> = line.split(' ').collect();
            let Some(oid) = fields.first() else {
                return Err(ReviewError::InvalidGitOutput);
            };
            let mut entry = unsupported_entry(String::new(), "", 0);
            if fields.get(1) == Some(&"blob") && fields.len() == 3 {
                let size: usize = fields[2]
                    .parse()
                    .map_err(|_| ReviewError::InvalidGitOutput)?;
                entry.size = u64::try_from(size).unwrap_or(u64::MAX);
                if size <= MAX_FILE_BYTES && total.saturating_add(size) <= MAX_CAPTURE_BYTES {
                    total += size;
                    fetch.push((*oid).to_owned());
                } else {
                    entry.kind = ContentKind::TooLarge;
                }
            }
            objects.insert((*oid).into(), entry);
        }
        if fetch.is_empty() {
            return Ok(objects);
        }
        let request = format!("{}\n", fetch.join("\n"));
        let output = git::run_input(
            project,
            &["cat-file", "--batch"],
            "reading Git blobs",
            false,
            MAX_CAPTURE_BYTES + git::MAX_OUTPUT,
            Some(request.as_bytes()),
        )
        .await?;
        self.save_git_objects(&mut objects, fetch, &output.bytes)
            .await?;
        Ok(objects)
    }

    async fn save_git_objects(
        &self,
        objects: &mut BTreeMap<String, Entry>,
        fetch: Vec<String>,
        content: &[u8],
    ) -> Result<()> {
        let mut bytes = content;
        for requested in fetch {
            let end = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .ok_or(ReviewError::InvalidGitOutput)?;
            let header =
                std::str::from_utf8(&bytes[..end]).map_err(|_| ReviewError::InvalidGitOutput)?;
            let fields: Vec<_> = header.split(' ').collect();
            if fields.len() != 3 || fields[0] != requested || fields[1] != "blob" {
                return Err(ReviewError::InvalidGitOutput);
            }
            let size: usize = fields[2]
                .parse()
                .map_err(|_| ReviewError::InvalidGitOutput)?;
            bytes = &bytes[end + 1..];
            let content = bytes.get(..size).ok_or(ReviewError::InvalidGitOutput)?;
            if bytes.get(size) != Some(&b'\n') {
                return Err(ReviewError::InvalidGitOutput);
            }
            let entry = Entry {
                path: String::new(),
                object: Some(self.save_object(content).await?),
                mode: String::new(),
                kind: content_kind(content),
                size: u64::try_from(size).unwrap_or(u64::MAX),
            };
            save_json(
                &self
                    .storage
                    .join("git-objects")
                    .join(format!("{requested}.json")),
                &entry,
            )
            .await?;
            objects.insert(requested, entry);
            bytes = &bytes[size + 1..];
        }
        if !bytes.is_empty() {
            return Err(ReviewError::InvalidGitOutput);
        }
        Ok(())
    }

    pub(super) async fn save_object(&self, bytes: &[u8]) -> Result<String> {
        let directory = self.storage.join("objects");
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(ReviewError::Storage)?;
        let prefix = digest(bytes);
        for suffix in 0..1000 {
            let id = format!("{prefix}-{suffix}");
            let path = directory.join(&id);
            match super::read_bytes_limit(&path, MAX_FILE_BYTES).await {
                Ok(existing) if existing == bytes => return Ok(id),
                Ok(_) => continue,
                Err(ReviewError::NotFound) => {}
                Err(error) => return Err(error),
            }
            let temporary =
                tempfile::NamedTempFile::new_in(&directory).map_err(ReviewError::Storage)?;
            tokio::fs::write(temporary.path(), bytes)
                .await
                .map_err(ReviewError::Storage)?;
            match temporary.persist_noclobber(&path) {
                Ok(_) => return Ok(id),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if super::read_bytes_limit(&path, MAX_FILE_BYTES).await? == bytes {
                        return Ok(id);
                    }
                }
                Err(error) => return Err(ReviewError::Storage(error.error)),
            }
        }
        Err(ReviewError::Limit)
    }

    pub(super) async fn object(&self, entry: Option<&Entry>) -> Result<Vec<u8>> {
        let Some(object) = entry.and_then(|entry| entry.object.as_ref()) else {
            return Ok(Vec::new());
        };
        super::validate_id(object)?;
        super::read_bytes_limit(&self.storage.join("objects").join(object), MAX_FILE_BYTES).await
    }
}

async fn read_git_tree(project: &Path, tree: GitTree) -> Result<Vec<u8>> {
    let output = match tree {
        GitTree::Index => {
            git::run(
                project,
                &["ls-files", "--stage", "-z", "--", "."],
                "reading the index",
                false,
                git::MAX_OUTPUT,
            )
            .await?
            .bytes
        }
        GitTree::Head => {
            let head = git::run(
                project,
                &["rev-parse", "--verify", "--quiet", "HEAD"],
                "reading HEAD",
                true,
                128,
            )
            .await?;
            if head.code == Some(1) {
                Vec::new()
            } else {
                let oid = std::str::from_utf8(&head.bytes)
                    .map_err(|_| ReviewError::InvalidGitOutput)?
                    .trim();
                git::run(
                    project,
                    &["ls-tree", "-r", "-z", oid, "--", "."],
                    "reading the HEAD tree",
                    false,
                    git::MAX_OUTPUT,
                )
                .await?
                .bytes
            }
        }
    };
    Ok(output)
}

pub(super) fn digest(bytes: &[u8]) -> String {
    // Hashes are locators, never proof of equality: object and review reuse compares bytes.
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}-{}", hasher.finish(), bytes.len())
}

async fn unique_id(storage: &Path, prefix: &str) -> Result<String> {
    tokio::fs::create_dir_all(storage)
        .await
        .map_err(ReviewError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(storage, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(ReviewError::Storage)?;
    }
    let file = tempfile::Builder::new()
        .prefix(prefix)
        .rand_bytes(16)
        .tempfile_in(storage)
        .map_err(ReviewError::Storage)?;
    file.path()
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .ok_or(ReviewError::InvalidInput)
}

pub(super) fn validate_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ReviewError::InvalidInput);
    }
    Ok(())
}

fn unsupported_entry(path: String, mode: &str, size: u64) -> Entry {
    Entry {
        path,
        object: None,
        mode: mode.into(),
        kind: ContentKind::Unsupported,
        size,
    }
}

pub(super) fn content_kind(bytes: &[u8]) -> ContentKind {
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        ContentKind::Binary
    } else {
        ContentKind::Text
    }
}

fn file_mode(metadata: &std::fs::Metadata) -> String {
    if metadata.file_type().is_symlink() {
        return "120000".into();
    }
    if metadata.is_dir() {
        return "160000".into();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            return "100755".into();
        }
    }
    "100644".into()
}

pub(super) fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf> {
    if bytes.contains(&0) {
        return Err(ReviewError::InvalidInput);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(OsString::from(
            std::str::from_utf8(bytes).map_err(|_| ReviewError::InvalidInput)?,
        )))
    }
}

pub(super) fn os_bytes(value: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        value.to_string_lossy().as_bytes().to_vec()
    }
}
