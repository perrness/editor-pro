//! Content identities and annotations. Coordinates are display hints only;
//! freshness never uses navigation anchors, timestamps, or status letters.
use crate::{
    diff::{Diff, Kind},
    git::{Comparison, Repository},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub const MAX_RECORDS: usize = 4096;
pub const MAX_NOTE: usize = 16 * 1024;
pub const MAX_CONTEXT: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentId {
    pub present: bool,
    pub bytes: usize,
    pub sha256: String,
}
impl ContentId {
    pub fn new(bytes: &[u8], present: bool) -> Self {
        Self {
            present,
            bytes: bytes.len(),
            sha256: fingerprint(bytes),
        }
    }
}
pub fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub repository: Repository,
    pub comparison: Comparison,
    /// Full commit ID; None for index and unborn HEAD. Blob IDs remain separate.
    pub resolved_base: Option<String>,
    #[serde(with = "path_bytes")]
    pub old_path: PathBuf,
    #[serde(with = "path_bytes")]
    pub new_path: PathBuf,
    pub old_blob: Option<String>,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    pub new_blob: Option<String>,
    pub old: ContentId,
    pub new: ContentId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Side {
    Old,
    New,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Attachment {
    File,
    Hunk { fingerprint: String },
    Line { side: Side, fingerprint: String },
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub excerpt: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Unreviewed,
    Reviewed,
    Attention,
}
impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unreviewed => "unreviewed",
            Self::Reviewed => "reviewed",
            Self::Attention => "needs-attention",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unreviewed" => Some(Self::Unreviewed),
            "reviewed" => Some(Self::Reviewed),
            "attention" => Some(Self::Attention),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: u64,
    pub revision: u64,
    pub identity: Identity,
    pub attachment: Attachment,
    pub context: Context,
    pub state: Option<State>,
    pub note: Option<String>,
    pub unsaved: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Unavailable,
    Orphaned,
}
impl Freshness {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "STALE",
            Self::Unavailable => "unavailable",
            Self::Orphaned => "orphaned/ambiguous",
        }
    }
}
impl Record {
    pub fn freshness(&self, current: Option<&Diff>) -> Freshness {
        let Some(diff) = current else {
            return Freshness::Unavailable;
        };
        let Some(identity) = &diff.identity else {
            return Freshness::Unavailable;
        };
        if self.identity != *identity {
            return Freshness::Stale;
        }
        if diff.has_attachment(&self.attachment) {
            Freshness::Fresh
        } else {
            Freshness::Orphaned
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    pub version: u32,
    pub repository: Repository,
    pub next_id: u64,
    pub records: Vec<Record>,
}
impl Store {
    pub fn empty(repository: Repository) -> Self {
        Self {
            version: 1,
            repository,
            next_id: 1,
            records: Vec::new(),
        }
    }
    pub fn validate(&self, repo: &Repository) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported review store version; original preserved".into());
        }
        if self.repository != *repo {
            return Err("Review store belongs to another repository/worktree".into());
        }
        if self.next_id == 0 || self.records.len() > MAX_RECORDS {
            return Err("Review store exceeds 4096 records".into());
        }
        let mut ids = BTreeSet::new();
        let mut marks = BTreeSet::new();
        for record in &self.records {
            if record.state.is_some()
                && !marks.insert(
                    serde_json::to_string(&(&record.identity, &record.attachment))
                        .map_err(|e| e.to_string())?,
                )
            {
                return Err("Conflicting duplicate mark; clear the existing target mark before revalidating".into());
            }
            if record.id == 0
                || record.id >= self.next_id
                || !ids.insert(record.id)
                || record.revision == 0
                || record.identity.repository != *repo
                || record.note.as_ref().is_some_and(|s| s.len() > MAX_NOTE)
                || record.context.excerpt.len() > MAX_CONTEXT
                || record
                    .context
                    .old_line
                    .is_some_and(|n| n > crate::git::CONTENT_LIMIT)
                || record
                    .context
                    .new_line
                    .is_some_and(|n| n > crate::git::CONTENT_LIMIT)
                || record.state.is_some() == record.note.is_some()
                || matches!(record.attachment, Attachment::Line { .. }) && record.state.is_some()
                || !valid_identity(&record.identity)
                || !valid_attachment(&record.attachment)
            {
                return Err("Invalid review record; original preserved".into());
            }
        }
        Ok(())
    }
}
fn valid_identity(id: &Identity) -> bool {
    [id.old_path.as_path(), id.new_path.as_path()]
        .into_iter()
        .all(|p| {
            !p.as_os_str().is_empty()
                && !p.is_absolute()
                && p.as_os_str().len() <= 4096
                && !p.as_os_str().as_encoded_bytes().contains(&0)
                && p.components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
        })
        && [&id.old, &id.new].into_iter().all(|c| {
            c.bytes <= crate::git::CONTENT_LIMIT
                && c.sha256.len() == 64
                && c.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                && (c.present || (c.bytes == 0 && c.sha256 == fingerprint(b"")))
        })
        && [&id.resolved_base, &id.old_blob, &id.new_blob]
            .into_iter()
            .all(|oid| {
                oid.as_ref().is_none_or(|s| {
                    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
                })
            })
        && id.old.present == id.old_blob.is_some()
        && [
            (&id.old_mode, id.old.present),
            (&id.new_mode, id.new.present),
        ]
        .into_iter()
        .all(|(mode, present)| {
            mode.as_ref()
                .is_some_and(|s| matches!(s.as_str(), "100644" | "100755"))
                == present
                && (present || mode.is_none())
        })
}
fn valid_attachment(attachment: &Attachment) -> bool {
    let hex = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
    match attachment {
        Attachment::File => true,
        Attachment::Hunk { fingerprint } => hex(fingerprint),
        Attachment::Line { fingerprint, .. } => {
            let fields: Vec<_> = fingerprint.split(':').collect();
            fields.len() == 3
                && fields[0].parse::<usize>().is_ok_and(|n| n <= 2)
                && fields[1]
                    .parse::<usize>()
                    .is_ok_and(|n| (1..=3).contains(&n))
                && hex(fields[2])
        }
    }
}

/// Only unique content signatures are actionable. Similar/repeated locations are
/// never disambiguated using an ordinal, row number or approximate source jump.
#[derive(Clone, Debug, Default)]
pub struct Locations {
    pub hunks: BTreeMap<usize, String>,
    pub lines: BTreeMap<(usize, Side), String>,
    pub by_key: BTreeMap<String, usize>,
}
fn hash_rows(diff: &Diff, start: usize, end: usize, side: Option<Side>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"editor-pro-location-v1");
    for row in &diff.rows[start..end] {
        if row.kind == Kind::Header {
            continue;
        }
        hash.update([match row.kind {
            Kind::Add => 1,
            Kind::Delete => 2,
            _ => 3,
        }]);
        hash.update((row.text.len() as u64).to_le_bytes());
        hash.update(row.text.as_bytes());
        hash.update([u8::from(row.no_newline)]);
    }
    hash.update([match side {
        None => 0,
        Some(Side::Old) => 1,
        Some(Side::New) => 2,
    }]);
    format!("{hash:x}", hash = hash.finalize())
}
impl Diff {
    /// Called in the Git worker, once per accepted candidate (never in render).
    pub fn identify_locations(&mut self) {
        let mut locations = Locations::default();
        for (n, start) in self.hunks.iter().copied().enumerate() {
            let end = self.hunks.get(n + 1).copied().unwrap_or(self.rows.len());
            locations
                .hunks
                .insert(start, hash_rows(self, start + 1, end, None));
            for index in start + 1..end {
                for (side, exists) in [
                    (Side::Old, self.rows[index].old.is_some()),
                    (Side::New, self.rows[index].new.is_some()),
                ] {
                    if exists {
                        // Include position of the target within the content window,
                        // not its absolute coordinate. This distinguishes boundaries.
                        let a = index.saturating_sub(2).max(start + 1);
                        let b = (index + 3).min(end);
                        let key = format!(
                            "{}:{}:{}",
                            index - a,
                            b - index,
                            hash_rows(self, a, b, Some(side))
                        );
                        locations.lines.insert((index, side), key);
                    }
                }
            }
        }
        fn unique<K: Ord>(values: &mut BTreeMap<K, String>) {
            let mut counts = BTreeMap::new();
            for value in values.values() {
                *counts.entry(value.clone()).or_insert(0) += 1;
            }
            values.retain(|_, value| counts[value] == 1);
        }
        unique(&mut locations.hunks);
        unique(&mut locations.lines);
        for (i, key) in &locations.hunks {
            locations.by_key.insert(key.clone(), *i);
        }
        for ((i, _), key) in &locations.lines {
            locations.by_key.insert(key.clone(), *i);
        }
        self.locations = locations;
    }
    pub fn has_attachment(&self, attachment: &Attachment) -> bool {
        self.attachment_row(attachment).is_some()
    }
    pub fn attachment_row(&self, attachment: &Attachment) -> Option<usize> {
        match attachment {
            Attachment::File => Some(0),
            Attachment::Hunk { fingerprint } => self
                .locations
                .by_key
                .get(fingerprint)
                .copied()
                .filter(|row| self.locations.hunks.get(row) == Some(fingerprint)),
            Attachment::Line { side, fingerprint } => self
                .locations
                .by_key
                .get(fingerprint)
                .copied()
                .filter(|row| self.locations.lines.get(&(*row, *side)) == Some(fingerprint)),
        }
    }
    pub fn attachment(&self, row: usize, kind: &str) -> Result<(Attachment, Context), String> {
        if kind == "file" {
            return Ok((Attachment::File, Context::default()));
        }
        let index = if kind == "hunk" {
            self.hunks
                .iter()
                .copied()
                .take_while(|i| *i <= row)
                .last()
                .ok_or("No hunk selected")?
        } else {
            row
        };
        let r = self.rows.get(index).ok_or("No line selected")?;
        let attachment = match kind {
            "hunk" => Attachment::Hunk {
                fingerprint: self
                    .locations
                    .hunks
                    .get(&index)
                    .ok_or("Ambiguous repeated hunk; annotate the file instead")?
                    .clone(),
            },
            "old" | "new" => {
                let side = if kind == "old" { Side::Old } else { Side::New };
                Attachment::Line { side, fingerprint: self.locations.lines.get(&(index, side)).ok_or("Missing or ambiguous line on that side; choose another line or annotate the file")?.clone() }
            }
            _ => return Err("Attachment: file, hunk, old or new".into()),
        };
        let context = if kind == "hunk" {
            let end = self
                .hunks
                .iter()
                .copied()
                .find(|i| *i > index)
                .unwrap_or(self.rows.len());
            let mut excerpt = String::new();
            for r in self.rows[index..end].iter().take(8) {
                excerpt.push_str(&bounded(&r.text, 100));
                excerpt.push('\n');
            }
            Context {
                old_line: self.rows[index + 1..end].iter().find_map(|r| r.old),
                new_line: self.rows[index + 1..end].iter().find_map(|r| r.new),
                excerpt: bounded(&excerpt, MAX_CONTEXT),
            }
        } else {
            Context {
                old_line: r.old,
                new_line: r.new,
                excerpt: bounded(&r.text, MAX_CONTEXT),
            }
        };
        Ok((attachment, context))
    }
}
pub fn bounded(value: &str, bytes: usize) -> String {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

/// JSON arrays preserve all Unix filename bytes, including non-UTF-8 names.
/// No lossy display string is ever used for identity or filesystem access.
pub mod path_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::path::{Path, PathBuf};
    pub fn serialize<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
        path.as_os_str().as_encoded_bytes().serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        let bytes = Vec::<u8>::deserialize(d)?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_vec(bytes).into())
        }
        #[cfg(not(unix))]
        {
            String::from_utf8(bytes)
                .map(PathBuf::from)
                .map_err(serde::de::Error::custom)
        }
    }
}
