use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::{Result, ReviewError, WorkspaceReview, git};

impl WorkspaceReview {
    pub(super) async fn config_bool(&self, key: &str, default: bool) -> Result<bool> {
        let output = git::run(
            &self.project,
            &["config", "--type=bool", "--get", key],
            "reading workspace configuration",
            true,
            256,
        )
        .await?;
        if output.code == Some(1) {
            return Ok(default);
        }
        match output.bytes.strip_suffix(b"\n").unwrap_or(&output.bytes) {
            b"true" => Ok(true),
            b"false" => Ok(false),
            _ => Err(ReviewError::InvalidGitOutput),
        }
    }

    pub(super) async fn check_content_filters(&self, tracked: &BTreeSet<String>) -> Result<()> {
        let mut paths = Vec::new();
        for path in tracked {
            paths.extend(
                STANDARD
                    .decode(path)
                    .map_err(|_| ReviewError::InvalidInput)?,
            );
            paths.push(0);
        }
        let attributes = git::run_input(
            &self.project,
            &["check-attr", "-z", "--stdin", "filter"],
            "checking content filters",
            false,
            git::MAX_OUTPUT,
            Some(&paths),
        )
        .await?;
        let fields: Vec<_> = attributes.bytes.split(|byte| *byte == 0).collect();
        for record in fields.as_chunks::<3>().0 {
            if record[2] != b"unspecified" && record[2] != b"unset" {
                return Err(ReviewError::ContentFilters);
            }
        }
        Ok(())
    }

    pub(super) async fn eol_paths<'a>(
        &self,
        paths: impl Iterator<Item = &'a String>,
    ) -> Result<BTreeMap<String, bool>> {
        let mut input = Vec::new();
        for path in paths {
            input.extend(
                STANDARD
                    .decode(path)
                    .map_err(|_| ReviewError::InvalidInput)?,
            );
            input.push(0);
        }
        let config = git::run(
            &self.project,
            &["config", "--get", "core.autocrlf"],
            "reading line ending configuration",
            true,
            256,
        )
        .await?;
        let automatic = matches!(
            std::str::from_utf8(&config.bytes)
                .map_err(|_| ReviewError::InvalidGitOutput)?
                .trim(),
            "true" | "input"
        );
        let output = git::run_input(
            &self.project,
            &["check-attr", "-z", "--stdin", "text", "eol"],
            "reading line ending attributes",
            false,
            git::MAX_OUTPUT,
            Some(&input),
        )
        .await?;
        let fields: Vec<_> = output.bytes.split(|byte| *byte == 0).collect();
        let mut values: BTreeMap<String, (bool, bool, bool, bool)> = BTreeMap::new();
        for record in fields.as_chunks::<3>().0 {
            let value = values
                .entry(STANDARD.encode(record[0]))
                .or_insert((automatic, false, true, false));
            match (record[1], record[2]) {
                (b"text", b"unset") => value.1 = true,
                (b"text", b"set") => {
                    value.0 = true;
                    value.2 = false;
                }
                (b"text", b"auto") => {
                    value.0 = true;
                    value.2 = true;
                    value.3 = true;
                }
                (b"eol", b"lf" | b"crlf") => {
                    value.0 = true;
                    if !value.3 {
                        value.2 = false;
                    }
                }
                _ => {}
            }
        }
        Ok(values
            .into_iter()
            .filter_map(|(path, (normalize, disabled, automatic, _))| {
                (normalize && !disabled).then_some((path, automatic))
            })
            .collect())
    }
}
