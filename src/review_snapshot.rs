//! Immutable review trees, stored with annotations in the atomic review store.
//! A baseline is captured once; later checkpoints only append current trees.
use crate::{git::Comparison, review::fingerprint};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

pub const MAX_FILES: usize = 512;
pub const TREE_LIMIT: usize = 8 * 1024 * 1024;
pub const MAX_CHECKPOINTS: usize = 32;
pub const HISTORY_LIMIT: usize = 24 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    #[serde(with = "crate::review::path_bytes")]
    pub path: PathBuf,
    pub text: String,
    pub sha256: String,
    pub mode: String,
    pub blob: Option<String>,
    pub unsaved: bool,
}
impl File {
    pub fn same_content(&self, other: &Self) -> bool {
        self.sha256 == other.sha256 && self.mode == other.mode
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub comparison: Comparison,
    pub resolved_base: Option<String>,
    pub files: Vec<File>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub id: u64,
    pub files: Vec<File>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub baseline: Baseline,
    pub checkpoints: Vec<Checkpoint>,
}
impl History {
    pub fn latest(&self) -> u64 {
        self.checkpoints.last().map_or(0, |c| c.id)
    }
    pub fn files(&self, comparison: &Comparison) -> Result<&[File], String> {
        match comparison {
            Comparison::ReviewFull => Ok(&self.baseline.files),
            Comparison::ReviewSince(id) => self
                .checkpoints
                .iter()
                .find(|c| c.id == *id)
                .map(|c| c.files.as_slice())
                .ok_or_else(|| "Review checkpoint not found".into()),
            _ => Err("Choose :review full or :review since [ID]".into()),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(
            self.baseline.comparison,
            Comparison::Head | Comparison::Index | Comparison::Revision(_)
        ) || !valid_oid(self.baseline.resolved_base.as_deref())
            || (self.baseline.comparison == Comparison::Index
                && self.baseline.resolved_base.is_some())
            || (matches!(self.baseline.comparison, Comparison::Revision(_))
                && self.baseline.resolved_base.is_none())
            || self
                .baseline
                .files
                .iter()
                .any(|f| f.blob.is_none() || f.unsaved)
            || self.checkpoints.is_empty()
            || self.checkpoints.len() > MAX_CHECKPOINTS
        {
            return Err("Invalid review history or checkpoint limit exceeded".into());
        }
        let mut bytes = validate_files(&self.baseline.files)?;
        let mut paths: BTreeSet<_> = self.baseline.files.iter().map(|f| &f.path).collect();
        for (index, checkpoint) in self.checkpoints.iter().enumerate() {
            if checkpoint.id != index as u64 + 1
                || checkpoint.files.iter().any(|f| f.blob.is_some())
            {
                return Err("Invalid review checkpoint sequence".into());
            }
            bytes += validate_files(&checkpoint.files)?;
            paths.extend(checkpoint.files.iter().map(|f| &f.path));
        }
        if paths.len() > MAX_FILES {
            return Err("Review history exceeds 512 retained paths".into());
        }
        if bytes > HISTORY_LIMIT {
            return Err("Review history exceeds 24 MiB; previous checkpoints preserved".into());
        }
        Ok(())
    }
}
fn valid_oid(value: Option<&str>) -> bool {
    value.is_none_or(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub fn validate_files(files: &[File]) -> Result<usize, String> {
    if files.len() > MAX_FILES {
        return Err("Review tree exceeds 512 files".into());
    }
    let mut paths = BTreeSet::new();
    let mut bytes = 0;
    for file in files {
        if !valid_path(&file.path)
            || !paths.insert(&file.path)
            || file.text.len() > crate::git::CONTENT_LIMIT
            || file.text.contains('\0')
            || !matches!(file.mode.as_str(), "100644" | "100755")
            || !valid_oid(file.blob.as_deref())
            || file.sha256 != fingerprint(file.text.as_bytes())
        {
            return Err("Invalid review snapshot content; original preserved".into());
        }
        bytes += file.text.len();
    }
    if bytes > TREE_LIMIT {
        return Err("Review tree exceeds 8 MiB".into());
    }
    Ok(bytes)
}
pub fn valid_path(path: &std::path::Path) -> bool {
    !path.as_os_str().is_empty()
        && path.as_os_str().len() <= 4096
        && !path.as_os_str().as_encoded_bytes().contains(&0)
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(p) if p != ".git"))
}

/// Prepared off the input thread. The expected checkpoint prevents two sessions
/// from silently advancing the same review baseline.
pub struct Capture {
    pub baseline: Baseline,
    pub files: Vec<File>,
    pub expected_latest: u64,
}
