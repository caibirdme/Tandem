use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use crate::RemoteError;

const MAX_DEPTH: usize = 16;
const MAX_FILES: usize = 256;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 16_384;
const MAX_INCLUDE_LENGTH: usize = 4096;

/// A bounded discovery failure; discovery never launches SSH or evaluates commands.
#[derive(Debug, thiserror::Error)]
pub enum SshConfigError {
    #[error("The SSH host discovery task could not finish: {0}")]
    DiscoveryTask(#[source] tokio::task::JoinError),
    #[error("The local home directory is unavailable; check your user profile")]
    HomeUnavailable,
    #[error("Could not read SSH configuration {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("SSH configuration {} is not UTF-8: {source}", path.display())]
    Encoding {
        path: PathBuf,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("Unterminated quoted value in SSH configuration {} at line {line}", path.display())]
    Quoting { path: PathBuf, line: usize },
    #[error("SSH configuration {} must be a regular file", path.display())]
    FileType { path: PathBuf },
    #[error(
        "SSH host discovery exceeded its {0} limit; reduce the configuration or Include patterns"
    )]
    Limit(SshConfigLimit),
}

/// Resource limits keep recursive Includes and broad globs bounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SshConfigLimit {
    #[error("Include nesting")]
    Depth,
    #[error("included file count")]
    Files,
    #[error("configuration size")]
    Bytes,
    #[error("directory entry and alias count")]
    Entries,
    #[error("Include path length")]
    IncludeLength,
}

/// List concrete aliases from the current user's default OpenSSH configuration.
///
/// Static Includes are followed in lexical order, including glob patterns and ~/.
/// Host-dependent Include tokens and other users' ~user paths are left to SSH.
/// All Host/Match conditions remain unevaluated; this is an alias catalog only.
///
/// # Errors
/// Returns a typed error for unreadable, malformed, or excessively large files.
/// A missing configuration file or unmatched Include returns an empty catalog.
pub async fn list_ssh_hosts() -> Result<Vec<String>, RemoteError> {
    let hosts = tokio::task::spawn_blocking(|| {
        let home = std::env::home_dir()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(SshConfigError::HomeUnavailable)?;
        Discovery::new(&home).list()
    })
    .await
    .map_err(SshConfigError::DiscoveryTask)??;
    Ok(hosts)
}

struct Discovery {
    ssh_directory: PathBuf,
    home: PathBuf,
    visited: BTreeSet<PathBuf>,
    aliases: BTreeSet<String>,
    bytes: usize,
    entries: usize,
}

impl Discovery {
    fn new(home: &Path) -> Self {
        Self {
            ssh_directory: home.join(".ssh"),
            home: home.to_owned(),
            visited: BTreeSet::new(),
            aliases: BTreeSet::new(),
            bytes: 0,
            entries: 0,
        }
    }

    fn list(mut self) -> Result<Vec<String>, SshConfigError> {
        self.read_file(&self.ssh_directory.join("config"), 0)?;
        Ok(self.aliases.into_iter().collect())
    }

    fn count_entry(&mut self) -> Result<(), SshConfigError> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(SshConfigError::Limit(SshConfigLimit::Entries));
        }
        Ok(())
    }

    fn read_file(&mut self, path: &Path, depth: usize) -> Result<(), SshConfigError> {
        let canonical = match fs::canonicalize(path) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(read_error(path, source)),
        };
        if self.visited.contains(&canonical) {
            return Ok(());
        }
        if !fs::metadata(&canonical)
            .map_err(|source| read_error(path, source))?
            .is_file()
        {
            return Err(SshConfigError::FileType {
                path: path.to_owned(),
            });
        }
        if depth > MAX_DEPTH {
            return Err(SshConfigError::Limit(SshConfigLimit::Depth));
        }
        if self.visited.len() >= MAX_FILES {
            return Err(SshConfigError::Limit(SshConfigLimit::Files));
        }
        self.visited.insert(canonical);
        let file = fs::File::open(path).map_err(|source| read_error(path, source))?;
        let remaining = MAX_BYTES - self.bytes;
        let mut data = Vec::new();
        file.take(u64::try_from(remaining).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut data)
            .map_err(|source| read_error(path, source))?;
        if data.len() > remaining {
            return Err(SshConfigError::Limit(SshConfigLimit::Bytes));
        }
        self.bytes += data.len();
        let content = std::str::from_utf8(&data).map_err(|source| SshConfigError::Encoding {
            path: path.to_owned(),
            source,
        })?;
        for (line_index, line) in content.lines().enumerate() {
            let Some((keyword, values)) = directive(line) else {
                continue;
            };
            if !keyword.eq_ignore_ascii_case("host") && !keyword.eq_ignore_ascii_case("include") {
                continue;
            }
            let values = words(values).ok_or_else(|| SshConfigError::Quoting {
                path: path.to_owned(),
                line: line_index + 1,
            })?;
            for value in values {
                if keyword.eq_ignore_ascii_case("host") {
                    if concrete_alias(&value) {
                        self.count_entry()?;
                        self.aliases.insert(value);
                    }
                } else if value.len() > MAX_INCLUDE_LENGTH {
                    return Err(SshConfigError::Limit(SshConfigLimit::IncludeLength));
                } else if let Some(pattern) = self.include_path(&value) {
                    if pattern.as_os_str().len() > MAX_INCLUDE_LENGTH {
                        return Err(SshConfigError::Limit(SshConfigLimit::IncludeLength));
                    }
                    for included in self.expand_glob(&pattern)? {
                        self.read_file(&included, depth + 1)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn include_path(&self, value: &str) -> Option<PathBuf> {
        // Dynamic paths cannot be enumerated without a destination or command execution.
        let mut expanded = String::new();
        let mut characters = value.chars();
        while let Some(character) = characters.next() {
            if character == '%' {
                match characters.next()? {
                    '%' => expanded.push('%'),
                    'd' => expanded.push_str(self.home.to_str()?),
                    _ => return None,
                }
            } else if character == '$' && characters.as_str().starts_with('{') {
                characters.next();
                let mut variable = String::new();
                loop {
                    match characters.next()? {
                        '}' => break,
                        character => variable.push(character),
                    }
                }
                expanded.push_str(&std::env::var(variable).ok()?);
            } else {
                expanded.push(character);
            }
        }
        if expanded == "~" {
            return Some(self.home.clone());
        }
        if let Some(relative) = expanded.strip_prefix("~/") {
            return Some(self.home.join(relative));
        }
        #[cfg(windows)]
        if let Some(relative) = expanded.strip_prefix("~\\") {
            return Some(self.home.join(relative));
        }
        if expanded.starts_with('~') || expanded.is_empty() {
            return None;
        }
        let path = PathBuf::from(expanded);
        Some(if path.is_absolute() {
            path
        } else {
            self.ssh_directory.join(path)
        })
    }

    fn expand_glob(&mut self, pattern: &Path) -> Result<Vec<PathBuf>, SshConfigError> {
        let mut paths = vec![PathBuf::new()];
        for component in pattern.components() {
            let Component::Normal(name) = component else {
                for path in &mut paths {
                    path.push(component.as_os_str());
                }
                continue;
            };
            let name = name.to_string_lossy();
            if !name.contains(['*', '?', '[']) {
                for path in &mut paths {
                    path.push(component.as_os_str());
                }
                continue;
            }
            let mut matches = Vec::new();
            for directory in paths {
                let entries = match fs::read_dir(&directory) {
                    Ok(entries) => entries,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) =>
                    {
                        continue;
                    }
                    Err(source) => return Err(read_error(&directory, source)),
                };
                for entry in entries {
                    self.count_entry()?;
                    let entry = entry.map_err(|source| read_error(&directory, source))?;
                    let filename = entry.file_name();
                    let filename = filename.to_string_lossy();
                    if (!filename.starts_with('.') || name.starts_with('.'))
                        && glob_matches(&name, &filename)
                    {
                        matches.push(entry.path());
                    }
                }
            }
            matches.sort();
            paths = matches;
        }
        Ok(paths)
    }
}

fn read_error(path: &Path, source: std::io::Error) -> SshConfigError {
    SshConfigError::Read {
        path: path.to_owned(),
        source,
    }
}

fn directive(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|character: char| character.is_whitespace() || character == '=')?;
    let values = line[end..].trim_start();
    Some((
        &line[..end],
        values.strip_prefix('=').unwrap_or(values).trim_start(),
    ))
}

fn words(value: &str) -> Option<Vec<String>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' if characters
                .peek()
                .is_some_and(|next| matches!(next, '\\' | '"' | '\'')) =>
            {
                word.push(characters.next()?);
            }
            '"' | '\'' if quote == Some(character) => quote = None,
            '"' | '\'' if quote.is_none() => quote = Some(character),
            '#' if quote.is_none() => break,
            character if character.is_whitespace() && quote.is_none() => {
                if !word.is_empty() {
                    result.push(std::mem::take(&mut word));
                }
            }
            character => word.push(character),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        result.push(word);
    }
    Some(result)
}

fn concrete_alias(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

// Segment matching uses dynamic programming so adversarial patterns cannot backtrack exponentially.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<_> = pattern.chars().collect();
    let name: Vec<_> = name.chars().collect();
    let mut matched = vec![false; name.len() + 1];
    matched[0] = true;
    let mut index = 0;
    while index < pattern.len() {
        let character = pattern[index];
        if character == '*' {
            for offset in 1..matched.len() {
                matched[offset] |= matched[offset - 1];
            }
        } else {
            let class_end = (character == '[')
                .then(|| {
                    pattern[index + 1..]
                        .iter()
                        .position(|character| *character == ']')
                        .map(|offset| index + 1 + offset)
                })
                .flatten();
            for offset in (1..matched.len()).rev() {
                let accepted = if let Some(end) = class_end {
                    class_matches(&pattern[index + 1..end], name[offset - 1])
                } else {
                    character == '?' || character == name[offset - 1]
                };
                matched[offset] = matched[offset - 1] && accepted;
            }
            matched[0] = false;
            if let Some(end) = class_end {
                index = end;
            }
        }
        index += 1;
    }
    matched[name.len()]
}

fn class_matches(class: &[char], character: char) -> bool {
    let negated = class
        .first()
        .is_some_and(|first| matches!(first, '!' | '^'));
    let mut index = usize::from(negated);
    let mut found = false;
    while index < class.len() {
        if index + 2 < class.len() && class[index + 1] == '-' {
            found |= (class[index]..=class[index + 2]).contains(&character);
            index += 3;
        } else {
            found |= class[index] == character;
            index += 1;
        }
    }
    found != negated
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tandem-ssh-config-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join(".ssh"))
                .unwrap_or_else(|error| panic!("create fixture failed: {error}"));
            Self(path)
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.0.join(".ssh").join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .unwrap_or_else(|error| panic!("create fixture failed: {error}"));
            }
            fs::write(path, content)
                .unwrap_or_else(|error| panic!("write fixture failed: {error}"));
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_static_aliases_without_evaluating_conditions() {
        let fixture = Fixture::new();
        fixture.write("config", "# ignored\nHost = alpha beta *.example !excluded unsafe;host -option\n  HostName not-an-alias\nhOsT \"quoted\" alpha # comment\nMatch exec \"never execute this\"\n  Include \"parts with spaces/[ab]?.conf\"\nInclude ~/.ssh/config missing*.conf\n");
        fixture.write("parts with spaces/a1.conf", "Host nested\nInclude config\n");
        fixture.write("parts with spaces/b2.conf", "Host second\n");
        fixture.write("parts with spaces/c3.conf", "Host excluded-file\n");
        assert_eq!(
            Discovery::new(&fixture.0)
                .list()
                .unwrap_or_else(|error| panic!("discovery failed: {error}")),
            vec!["alpha", "beta", "nested", "quoted", "second"]
        );
    }

    #[test]
    fn missing_config_and_unmatched_include_are_empty() {
        let fixture = Fixture::new();
        assert!(
            Discovery::new(&fixture.0)
                .list()
                .is_ok_and(|hosts| hosts.is_empty())
        );
        fixture.write("config", "Include absent/*.conf\n");
        assert!(
            Discovery::new(&fixture.0)
                .list()
                .is_ok_and(|hosts| hosts.is_empty())
        );
    }

    #[test]
    fn rejects_malformed_quotes_and_non_utf8() {
        let fixture = Fixture::new();
        fixture.write("config", "Host \"unfinished\n");
        assert!(matches!(
            Discovery::new(&fixture.0).list(),
            Err(SshConfigError::Quoting { line: 1, .. })
        ));
        fs::write(fixture.0.join(".ssh/config"), [0xff])
            .unwrap_or_else(|error| panic!("write failed: {error}"));
        assert!(matches!(
            Discovery::new(&fixture.0).list(),
            Err(SshConfigError::Encoding { .. })
        ));
    }

    #[test]
    fn rejects_non_files_and_excessive_include_paths() {
        let fixture = Fixture::new();
        fixture.write("config", "Include directory\n");
        fs::create_dir(fixture.0.join(".ssh/directory"))
            .unwrap_or_else(|error| panic!("create directory failed: {error}"));
        assert!(matches!(
            Discovery::new(&fixture.0).list(),
            Err(SshConfigError::FileType { .. })
        ));
        fixture.write(
            "config",
            &format!("Include {}\n", "a".repeat(MAX_INCLUDE_LENGTH + 1)),
        );
        let error = Discovery::new(&fixture.0)
            .list()
            .err()
            .unwrap_or_else(|| panic!("unbounded Include was accepted"));
        assert!(matches!(
            error,
            SshConfigError::Limit(SshConfigLimit::IncludeLength)
        ));
        assert_eq!(
            RemoteError::from(error).code(),
            crate::RpcErrorCode::RuntimeUnavailable
        );
    }

    #[test]
    fn expands_home_tokens_and_skips_destination_dependent_includes() {
        let fixture = Fixture::new();
        fixture.write(
            "config",
            "Include %d/.ssh/host.conf %h.conf ~another-user/config\n",
        );
        fixture.write("host.conf", "Host home-token\n");
        assert_eq!(
            Discovery::new(&fixture.0)
                .list()
                .unwrap_or_else(|error| panic!("discovery failed: {error}")),
            vec!["home-token"]
        );
    }

    #[test]
    fn bounds_configuration_bytes_and_include_depth() {
        let fixture = Fixture::new();
        fixture.write("config", &"#".repeat(MAX_BYTES + 1));
        assert!(matches!(
            Discovery::new(&fixture.0).list(),
            Err(SshConfigError::Limit(SshConfigLimit::Bytes))
        ));
        fixture.write("config", "Include level-0\n");
        for depth in 0..=MAX_DEPTH {
            fixture.write(
                &format!("level-{depth}"),
                &format!("Include level-{}\n", depth + 1),
            );
        }
        assert!(matches!(
            Discovery::new(&fixture.0).list(),
            Err(SshConfigError::Limit(SshConfigLimit::Depth))
        ));
    }

    #[test]
    fn bounds_directory_entries_and_file_count() {
        let fixture = Fixture::new();
        fixture.write("config", "Include parts/*\n");
        fixture.write("parts/one", "Host one\n");
        let mut discovery = Discovery::new(&fixture.0);
        discovery.entries = MAX_ENTRIES;
        assert!(matches!(
            discovery.list(),
            Err(SshConfigError::Limit(SshConfigLimit::Entries))
        ));
        fixture.write("config", "Include parts/*\n");
        for index in 0..MAX_FILES {
            fixture.write(&format!("parts/host-{index}"), "Host alias\n");
        }
        assert!(matches!(
            Discovery::new(&fixture.0).list(),
            Err(SshConfigError::Limit(SshConfigLimit::Files))
        ));
    }

    #[test]
    fn matches_glob_segments_and_preserves_windows_backslashes() {
        for (pattern, value, expected) in [
            ("a*?", "abc", true),
            ("a?", "a", false),
            ("[a-c].conf", "b.conf", true),
            ("[!a-c]*", "a.conf", false),
            ("[!a-c]*", "z.conf", true),
            ("plain", "plain-more", false),
        ] {
            assert_eq!(
                glob_matches(pattern, value),
                expected,
                "{pattern:?} {value:?}"
            );
        }
        assert_eq!(
            words("\"C:\\Users\\Test\\config\" # comment"),
            Some(vec!["C:\\Users\\Test\\config".into()])
        );
    }
}
