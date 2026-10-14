//! Bounded review I/O. All product loading, serialization, fsync and export run
//! on this worker. A cooperative create-new lock plus record CAS merges unrelated
//! writers and refuses conflicting edits; corrupt stores are never replaced.
use crate::{
    git::Repository,
    review::{Freshness, Record, Store},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
};

pub const STORE_LIMIT: u64 = 4 * 1024 * 1024;
pub fn store_path(repo: &Repository) -> PathBuf {
    repo.git_dir.join("editor-pro-review.json")
}
fn fail(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(fail(e)),
        Ok(m) => {
            if !m.is_file() || m.file_type().is_symlink() {
                return Err("Review store is not a regular file".into());
            }
            if m.len() > STORE_LIMIT {
                return Err("Review store exceeds 4 MiB; original preserved".into());
            }
        }
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(fail)?
        .take(STORE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(fail)?;
    if bytes.len() as u64 > STORE_LIMIT {
        return Err("Review store exceeds 4 MiB; original preserved".into());
    }
    Ok(Some(bytes))
}
fn decode(bytes: Option<&[u8]>, repo: &Repository) -> Result<Store, String> {
    let store = match bytes {
        None => Store::empty(repo.clone()),
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|e| format!("Corrupt review store: {e}; original preserved"))?,
    };
    store.validate(repo)?;
    Ok(store)
}
pub fn load(repo: &Repository) -> Result<Store, String> {
    decode(read_bytes(&store_path(repo))?.as_deref(), repo)
}
#[derive(Clone, Debug)]
pub struct Change {
    pub expected: Option<Record>,
    pub replacement: Option<Record>,
}
struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn lock(path: &Path) -> Result<Lock, String> {
    let path = path.with_extension("json.lock");
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&path).map_err(|e| format!("Review lock unavailable: {e}. Retry; after a crashed writer, remove only its abandoned editor-pro-review.json.lock"))?;
    let guard = Lock(path);
    writeln!(file, "{}", std::process::id()).map_err(fail)?;
    Ok(guard)
}
/// Synchronous entry for tests; product calls only through Worker.
pub fn apply(repo: &Repository, change: &Change) -> Result<(Store, u64), String> {
    if fs::metadata(&repo.git_dir)
        .map_err(fail)?
        .permissions()
        .readonly()
    {
        return Err("Review metadata directory is read-only".into());
    }
    let path = store_path(repo);
    let _lock = lock(&path)?;
    let before = read_bytes(&path)?;
    let mut store = decode(before.as_deref(), repo)?;
    let id = if let Some(expected) = &change.expected {
        let pos = store
            .records
            .iter()
            .position(|r| r.id == expected.id)
            .ok_or("Review conflict: record was removed; reload before editing")?;
        if store.records[pos] != *expected {
            return Err(
                "Review conflict: record changed in another instance; reload before editing".into(),
            );
        }
        if let Some(mut replacement) = change.replacement.clone() {
            replacement.id = expected.id;
            replacement.revision = expected
                .revision
                .checked_add(1)
                .ok_or("Review revision exhausted")?;
            store.records[pos] = replacement;
        } else {
            store.records.remove(pos);
        }
        expected.id
    } else {
        let mut replacement = change.replacement.clone().ok_or("No review change")?;
        if replacement.state.is_some()
            && store.records.iter().any(|r| {
                r.state.is_some()
                    && r.identity == replacement.identity
                    && r.attachment == replacement.attachment
            })
        {
            return Err(
                "Review conflict: mark was created in another instance; reload before editing"
                    .into(),
            );
        }
        replacement.id = store.next_id;
        replacement.revision = 1;
        store.next_id = store.next_id.checked_add(1).ok_or("Review IDs exhausted")?;
        let id = replacement.id;
        store.records.push(replacement);
        id
    };
    store.validate(repo)?;
    let bytes = serde_json::to_vec_pretty(&store).map_err(fail)?;
    if bytes.len() as u64 > STORE_LIMIT {
        return Err("Review store would exceed 4 MiB; previous data preserved".into());
    }
    if let Ok(m) = fs::symlink_metadata(&path) {
        if m.permissions().readonly() {
            return Err("Review store is read-only".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if m.nlink() != 1 {
                return Err("Refusing a hard-linked review store".into());
            }
        }
    }
    let mut temporary = tempfile::NamedTempFile::new_in(&repo.git_dir).map_err(fail)?;
    temporary.write_all(&bytes).map_err(fail)?;
    temporary.as_file().sync_all().map_err(fail)?;
    if read_bytes(&path)? != before {
        return Err("Review store changed during save; original preserved".into());
    }
    if before.is_some() {
        temporary.persist(&path).map_err(|e| fail(e.error))?;
    } else {
        temporary
            .persist_noclobber(&path)
            .map_err(|e| fail(e.error))?;
    }
    #[cfg(unix)]
    File::open(&repo.git_dir).and_then(|d| d.sync_all()).map_err(|e| format!("Review replaced but directory sync failed; durability uncertain: {e}. Reload before retry"))?;
    Ok((store, id))
}

pub struct Export {
    pub store: Arc<Store>,
    pub freshness: Vec<(u64, Freshness)>,
    pub destination: PathBuf,
    pub protected: Vec<PathBuf>,
    pub scope: String,
}
fn escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\n' | '\t' => out.push(c),
            c if c.is_control()
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                out.push_str(&format!("\\u{{{:x}}}", c as u32))
            }
            c if c.is_ascii_punctuation() => out.push_str(&format!("&#{};", c as u32)),
            _ => out.push(c),
        }
    }
    out
}
pub fn path_text(path: &Path) -> String {
    if let Some(s) = path.to_str() {
        return s
            .chars()
            .flat_map(|c| {
                if c.is_control() || c == '\\' {
                    c.escape_default().collect::<Vec<_>>()
                } else {
                    vec![c]
                }
            })
            .collect();
    }
    let encoded: String = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() && *b != b'%' {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("(non-UTF-8; percent-escaped bytes) {encoded}")
}
/// Deterministic report of persisted annotations; it does not claim a complete
/// repository scan or export unannotated code. Excerpts are bounded on creation.
pub fn markdown(export: &Export) -> String {
    let mut out = String::from("# Code review feedback\n\n");
    out.push_str(&format!("Repository: {}\n\nWorktree Git directory: {}\n\nCoverage: {}. All persisted marks and notes loaded by this session at export request, including other bases and unresolved records. Concurrent additions require :review reload. Freshness is sampled, not an atomic filesystem snapshot. No source buffers were saved.\n\n",
        escape(&path_text(&export.store.repository.root)), escape(&path_text(&export.store.repository.git_dir)), escape(&export.scope)));
    let status: std::collections::BTreeMap<_, _> = export.freshness.iter().copied().collect();
    let fresh = |r: &Record| status.get(&r.id).copied().unwrap_or(Freshness::Unavailable);
    let reviewed_files = export
        .store
        .records
        .iter()
        .filter(|r| {
            matches!(r.attachment, crate::review::Attachment::File)
                && r.state == Some(crate::review::State::Reviewed)
                && fresh(r) == Freshness::Fresh
        })
        .count();
    let reviewed_hunks = export
        .store
        .records
        .iter()
        .filter(|r| {
            matches!(r.attachment, crate::review::Attachment::Hunk { .. })
                && r.state == Some(crate::review::State::Reviewed)
                && fresh(r) == Freshness::Fresh
        })
        .count();
    let attention = export
        .store
        .records
        .iter()
        .filter(|r| {
            r.state == Some(crate::review::State::Attention) && fresh(r) == Freshness::Fresh
        })
        .count();
    out.push_str(&format!("Progress: {reviewed_files} fresh reviewed file marks; {reviewed_hunks} fresh reviewed hunk marks; {attention} fresh needs-attention marks. Hunk marks do not complete files. A needs-attention hunk still requires attention even under a reviewed file mark. Stale/unavailable marks are excluded.\n\n"));
    let mut records: Vec<_> = export.store.records.iter().collect();
    records.sort_by(|a, b| {
        a.identity
            .new_path
            .cmp(&b.identity.new_path)
            .then(
                a.identity
                    .comparison
                    .label()
                    .cmp(b.identity.comparison.label()),
            )
            .then(a.identity.resolved_base.cmp(&b.identity.resolved_base))
            .then(a.id.cmp(&b.id))
    });
    if records.is_empty() {
        out.push_str("No persisted annotations.\n");
    }
    for r in records {
        let i = &r.identity;
        out.push_str(&format!(
            "## Record {} — {} — {}\n\n",
            r.id,
            fresh(r).label(),
            r.state.map_or("note", |s| s.label())
        ));
        out.push_str(&format!("Old path: {}\n\nNew path: {}\n\nComparison: {}; resolved base: {}; old blob: {}; new blob: {}\n\n",
            escape(&path_text(&i.old_path)), escape(&path_text(&i.new_path)), escape(i.comparison.label()), escape(i.resolved_base.as_deref().unwrap_or("index or unborn HEAD")), escape(i.old_blob.as_deref().unwrap_or("absent")), escape(i.new_blob.as_deref().unwrap_or("working content or absent"))));
        out.push_str(&format!("Content SHA-256: old {} ({} bytes, {}); new {} ({} bytes, {}). Annotated from unsaved content: {}.\n\nAttachment: {}. Old line: {}; new line: {}. Coordinates are historical hints, not identity.\n\n",
            i.old.sha256, i.old.bytes, if i.old.present { "present" } else { "missing" }, i.new.sha256, i.new.bytes, if i.new.present { "present" } else { "missing" }, r.unsaved,
            escape(&format!("{:?}", r.attachment)), r.context.old_line.map_or("—".into(), |n| (n+1).to_string()), r.context.new_line.map_or("—".into(), |n| (n+1).to_string())));
        out.push_str(&format!(
            "Modes: old {}; new {}.\n\n",
            escape(i.old_mode.as_deref().unwrap_or("absent")),
            escape(i.new_mode.as_deref().unwrap_or("absent"))
        ));
        if !r.context.excerpt.is_empty() {
            out.push_str(&format!(
                "Context (bounded excerpt):\n\n<pre>{}</pre>\n\n",
                escape(&r.context.excerpt)
            ));
        }
        if let Some(note) = &r.note {
            out.push_str(&format!("Note:\n\n<pre>{}</pre>\n\n", escape(note)));
        }
    }
    out
}
pub fn export(export: &Export) -> Result<PathBuf, String> {
    let destination = crate::file::absolute_path(&export.destination).map_err(fail)?;
    let repo = &export.store.repository;
    if destination.starts_with(&repo.root)
        || destination.starts_with(&repo.git_dir)
        || destination.components().any(|c| c.as_os_str() == ".git")
        || export.protected.contains(&destination)
    {
        return Err(
            "Export must be a new file outside the worktree, Git metadata and open source paths"
                .into(),
        );
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err("Export destination already exists; choose a new path".into());
    }
    let parent = destination.parent().ok_or("No export parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(fail)?;
    temporary
        .write_all(markdown(export).as_bytes())
        .map_err(fail)?;
    temporary.as_file().sync_all().map_err(fail)?;
    temporary
        .persist_noclobber(&destination)
        .map_err(|e| fail(e.error))?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("Export created but directory sync failed: {e}"))?;
    Ok(destination)
}

pub enum Job {
    Load(Repository),
    Change(Repository, Box<Change>),
    Export(Box<Export>),
}
pub enum Outcome {
    Loaded(Arc<Store>),
    Saved(Arc<Store>, u64),
    Exported(PathBuf),
}
pub struct Worker {
    tx: Option<SyncSender<Job>>,
    rx: Receiver<Result<Outcome, String>>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn new() -> io::Result<Self> {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (done, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("review-store".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = match job {
                        Job::Load(repo) => load(&repo).map(|s| Outcome::Loaded(Arc::new(s))),
                        Job::Change(repo, change) => {
                            apply(&repo, &change).map(|(s, id)| Outcome::Saved(Arc::new(s), id))
                        }
                        Job::Export(request) => export(&request).map(Outcome::Exported),
                    };
                    if done.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            tx: Some(tx),
            rx,
            thread: Some(thread),
        })
    }
    pub fn submit(&self, job: Job) -> Result<(), String> {
        self.tx.as_ref().unwrap().try_send(job).map_err(fail)
    }
    pub fn poll(&self) -> Option<Result<Outcome, String>> {
        self.rx.try_recv().ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
