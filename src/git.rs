//! Read-only Git CLI inspection. One worker, one pending request and one result;
//! subprocesses never run in input dispatch or rendering.
use crate::diff::Diff;
use ropey::Rope;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const CONTENT_LIMIT: usize = 2 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Comparison {
    #[default]
    Head,
    Index,
    Revision(String),
    Staged,
}
impl Comparison {
    pub fn label(&self) -> &str {
        match self {
            Self::Head => "HEAD",
            Self::Index => "index",
            Self::Revision(s) => s,
            Self::Staged => "staged HEAD → index",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "" | "head" | "HEAD" => Self::Head,
            "index" => Self::Index,
            "staged" => Self::Staged,
            _ => Self::Revision(s.into()),
        }
    }
}
#[derive(Clone)]
pub struct BufferVersion {
    pub id: usize,
    pub generation: u64,
    pub dirty: bool,
    pub text: Rope,
}
#[derive(Clone)]
pub struct Request {
    pub path: PathBuf,
    pub buffer: Option<BufferVersion>,
    pub comparison: Comparison,
    pub inspect: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    pub root: PathBuf,
    pub git_dir: PathBuf,
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub original: Option<PathBuf>,
    pub index: char,
    pub worktree: char,
    pub conflict: bool,
}
impl Entry {
    pub fn untracked(&self) -> bool {
        self.index == '?'
    }
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub filters_disabled: bool,
    pub repository: Repository,
    pub branch: String,
    pub head: Option<String>,
    pub entries: Vec<Entry>,
    pub comparison: Comparison,
    pub diff: Result<Diff, String>,
}
impl Snapshot {
    pub fn summary(&self) -> String {
        format!(
            "{} S{} W{} ?{} !{}{}",
            self.branch,
            self.entries
                .iter()
                .filter(|e| !matches!(e.index, '.' | '?') && !e.conflict)
                .count(),
            self.entries
                .iter()
                .filter(|e| !matches!(e.worktree, '.' | '?') && !e.conflict)
                .count(),
            self.entries.iter().filter(|e| e.untracked()).count(),
            self.entries.iter().filter(|e| e.conflict).count(),
            if self.filters_disabled {
                " [filters off]"
            } else {
                ""
            }
        )
    }
}
#[derive(Clone)]
pub struct Config {
    pub executable: PathBuf,
    pub timeout: Duration,
    pub debounce: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            executable: "git".into(),
            timeout: Duration::from_secs(5),
            debounce: Duration::from_millis(75),
        }
    }
}
pub struct Reply {
    pub serial: u64,
    pub request: Request,
    pub result: Result<Option<Snapshot>, String>,
}
struct Queue {
    pending: Option<(u64, Request)>,
    result: Option<Reply>,
    closed: bool,
}
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    serial: AtomicU64,
}
pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn new(config: Config) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                pending: None,
                result: None,
                closed: false,
            }),
            wake: Condvar::new(),
            serial: AtomicU64::new(0),
        });
        let state = shared.clone();
        let thread = thread::Builder::new()
            .name("git-inspection".into())
            .spawn(move || {
                loop {
                    let mut queue = state.queue.lock().unwrap();
                    while !queue.closed && queue.pending.is_none() {
                        queue = state.wake.wait(queue).unwrap();
                    }
                    if queue.closed {
                        break;
                    }
                    // Restart the quiet period on every new request; storage remains bounded.
                    loop {
                        let (next, timeout) =
                            state.wake.wait_timeout(queue, config.debounce).unwrap();
                        queue = next;
                        if queue.closed {
                            return;
                        }
                        if timeout.timed_out() {
                            break;
                        }
                    }
                    let Some((serial, request)) = queue.pending.take() else {
                        continue;
                    };
                    drop(queue);
                    let runner = Runner {
                        config: &config,
                        serial,
                        current: &state.serial,
                        deadline: Instant::now() + config.timeout,
                        overrides: Vec::new(),
                    };
                    let result = inspect_with(&runner, &request);
                    let mut queue = state.queue.lock().unwrap();
                    if !queue.closed && state.serial.load(Ordering::Acquire) == serial {
                        queue.result = Some(Reply {
                            serial,
                            request,
                            result,
                        });
                    }
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
    pub fn submit(&self, request: Request) -> u64 {
        let mut queue = self.shared.queue.lock().unwrap();
        let serial = self.shared.serial.fetch_add(1, Ordering::AcqRel) + 1;
        queue.pending = Some((serial, request));
        queue.result = None;
        self.shared.wake.notify_one();
        serial
    }
    pub fn cancel(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        self.shared.serial.fetch_add(1, Ordering::AcqRel);
        queue.pending = None;
        queue.result = None;
        self.shared.wake.notify_one();
    }
    pub fn poll(&self) -> Option<Reply> {
        self.shared.queue.lock().unwrap().result.take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        {
            let mut q = self.shared.queue.lock().unwrap();
            q.closed = true;
            q.pending = None;
        }
        self.shared.serial.fetch_add(1, Ordering::AcqRel);
        self.shared.wake.notify_one();
        // Only editor shutdown drops the worker, after restoring the terminal.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
/// Synchronous entry for tests/measurements only. Product calls use Worker.
pub fn inspect(request: &Request, config: &Config) -> Result<Option<Snapshot>, String> {
    let current = AtomicU64::new(0);
    inspect_with(
        &Runner {
            config,
            serial: 0,
            current: &current,
            deadline: Instant::now() + config.timeout,
            overrides: Vec::new(),
        },
        request,
    )
}
#[derive(Clone)]
struct Runner<'a> {
    config: &'a Config,
    serial: u64,
    current: &'a AtomicU64,
    deadline: Instant,
    overrides: Vec<OsString>,
}
struct Output {
    success: bool,
    code: Option<i32>,
    bytes: Vec<u8>,
    error: String,
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // Dedicated process group covers a slow launcher and descendants.
            #[cfg(unix)]
            {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{}", self.0.id())])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
impl Runner<'_> {
    fn check(&self) -> Result<(), String> {
        if self.current.load(Ordering::Acquire) != self.serial {
            Err("Git request cancelled".into())
        } else if Instant::now() >= self.deadline {
            Err("Git inspection timed out".into())
        } else {
            Ok(())
        }
    }
    fn run(&self, cwd: &Path, args: &[&OsStr]) -> Result<Output, String> {
        self.check()?;
        let mut out = tempfile::tempfile().map_err(err)?;
        let mut stderr = tempfile::tempfile().map_err(err)?;
        let mut command = Command::new(&self.config.executable);
        command
            .current_dir(cwd)
            .args([
                "--no-optional-locks",
                "--literal-pathspecs",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "diff.renames=true",
                "-c",
                "diff.ignoreSubmodules=all",
            ])
            .args(&self.overrides)
            .args(args)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .stdin(Stdio::null())
            .stdout(out.try_clone().map_err(err)?)
            .stderr(stderr.try_clone().map_err(err)?);
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_EXTERNAL_DIFF",
            "GIT_DIFF_OPTS",
        ] {
            command.env_remove(name);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut process = Process(
            command
                .spawn()
                .map_err(|e| format!("Cannot run Git: {e}"))?,
        );
        let status = loop {
            self.check()?;
            if out.metadata().map_err(err)?.len() > OUTPUT_LIMIT
                || stderr.metadata().map_err(err)?.len() > OUTPUT_LIMIT
            {
                return Err("Git output exceeds 16 MiB limit".into());
            }
            if let Some(status) = process.0.try_wait().map_err(err)? {
                break status;
            }
            thread::sleep(Duration::from_millis(2));
        };
        let bytes = read_output(&mut out)?;
        let error = String::from_utf8_lossy(&read_output(&mut stderr)?)
            .trim()
            .to_string();
        Ok(Output {
            success: status.success(),
            code: status.code(),
            bytes,
            error,
        })
    }
    fn git(&self, cwd: &Path, args: &[&OsStr]) -> Result<Vec<u8>, String> {
        let result = self.run(cwd, args)?;
        if result.success {
            Ok(result.bytes)
        } else {
            Err(format!("Git failed: {}", result.error))
        }
    }
}
fn read_output(file: &mut File) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(0)).map_err(err)?;
    let mut bytes = Vec::new();
    file.take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err("Git output exceeds 16 MiB limit".into());
    }
    Ok(bytes)
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn refs(values: &[OsString]) -> Vec<&OsStr> {
    values.iter().map(OsString::as_os_str).collect()
}
fn line(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}
fn path(bytes: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes.to_vec()).into())
    }
    #[cfg(not(unix))]
    {
        Ok(std::str::from_utf8(bytes).map_err(err)?.into())
    }
}
pub fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .flat_map(|ch| {
            if ch.is_control() {
                ch.escape_default().collect::<Vec<_>>()
            } else {
                vec![ch]
            }
        })
        .collect()
}
fn inspect_with(runner: &Runner<'_>, request: &Request) -> Result<Option<Snapshot>, String> {
    let mut cwd = request.path.parent().ok_or("No parent directory")?;
    while !cwd.is_dir() {
        cwd = cwd.parent().ok_or("No existing parent directory")?;
    }
    let result = runner.run(cwd, &refs(&args(&["rev-parse", "--show-toplevel"])))?;
    if !result.success {
        if result.error.contains("not a git repository") {
            return Ok(None);
        }
        return Err(format!("Git discovery failed: {}", result.error));
    }
    let root = path(line(&result.bytes))?;
    let git_dir = path(line(
        &runner.git(&root, &refs(&args(&["rev-parse", "--absolute-git-dir"])))?,
    ))?;
    let repository = Repository { root, git_dir };
    // Status can invoke clean/process filters as well as fsmonitor. Suppress
    // configured helpers without changing repository configuration or attributes.
    let filters = runner.run(
        &repository.root,
        &refs(&args(&[
            "config",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|process|required)$",
        ])),
    )?;
    if !filters.success && filters.code != Some(1) {
        return Err(format!("Cannot inspect Git filters: {}", filters.error));
    }
    let mut overrides = Vec::new();
    for key in filters.bytes.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let key = std::str::from_utf8(key).map_err(err)?;
        overrides.push("-c".into());
        overrides.push(
            format!(
                "{key}={}",
                if key.ends_with(".required") {
                    "false"
                } else {
                    ""
                }
            )
            .into(),
        );
    }
    let filters_disabled = !overrides.is_empty();
    let runner = Runner {
        overrides,
        ..runner.clone()
    };
    let status_args = args(&[
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=all",
    ]);
    let raw = runner.git(&repository.root, &refs(&status_args))?;
    let (branch, head, entries) = parse_status(&raw)?;
    let mut snapshot = Snapshot {
        filters_disabled,
        repository,
        branch,
        head,
        entries,
        comparison: request.comparison.clone(),
        diff: Ok(Diff::default()),
    };
    if request.inspect {
        snapshot.diff = comparison(&runner, request, &snapshot);
    }
    // Detect an index/ref/status change during the multi-command snapshot.
    if raw != runner.git(&snapshot.repository.root, &refs(&status_args))? {
        return Err("Git changed during inspection; refresh pending (r / :gitrefresh)".into());
    }
    Ok(Some(snapshot))
}
fn parse_status(raw: &[u8]) -> Result<(String, Option<String>, Vec<Entry>), String> {
    let mut records = raw.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut branch = String::new();
    let mut head = None;
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        if let Some(value) = record.strip_prefix(b"# branch.head ") {
            branch = String::from_utf8_lossy(value).into_owned();
            continue;
        }
        if let Some(value) = record.strip_prefix(b"# branch.oid ") {
            if value != b"(initial)" {
                head = Some(String::from_utf8_lossy(value).into_owned());
            }
            continue;
        }
        if record.starts_with(b"# ") {
            continue;
        }
        let (parts, conflict, rename) = match record[0] {
            b'1' => (9, false, false),
            b'2' => (10, false, true),
            b'u' => (11, true, false),
            b'?' => (2, false, false),
            _ => return Err("Unsupported Git status record".into()),
        };
        let fields: Vec<_> = record.splitn(parts, |b| *b == b' ').collect();
        if fields.len() != parts {
            return Err("Malformed Git status record".into());
        }
        let (index, worktree) = if record[0] == b'?' {
            ('?', '?')
        } else {
            if fields[1].len() != 2 {
                return Err("Malformed Git status flags".into());
            }
            (fields[1][0] as char, fields[1][1] as char)
        };
        let original = if rename {
            Some(path(records.next().ok_or("Missing rename source")?)?)
        } else {
            None
        };
        entries.push(Entry {
            path: path(fields[parts - 1])?,
            original,
            index,
            worktree,
            conflict,
        });
        if entries.len() > 100_000 {
            return Err("Git status exceeds 100,000 files".into());
        }
    }
    if branch.is_empty() {
        return Err("Git status omitted branch identity".into());
    }
    if branch == "(detached)" {
        branch = format!(
            "detached@{}",
            head.as_deref()
                .unwrap_or("unknown")
                .chars()
                .take(12)
                .collect::<String>()
        );
    } else if head.is_none() {
        branch.push_str(" (unborn)");
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((branch, head, entries))
}
struct Content {
    bytes: Vec<u8>,
    identity: String,
}
fn blob(
    runner: &Runner<'_>,
    repo: &Repository,
    revision: Option<&str>,
    path: &Path,
) -> Result<Content, String> {
    let mut command = if let Some(revision) = revision {
        args(&["ls-tree", "-z", revision, "--"])
    } else {
        args(&["ls-files", "--stage", "-z", "--"])
    };
    command.push(path.as_os_str().to_owned());
    let bytes = runner.git(&repo.root, &refs(&command))?;
    let mut object = None;
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("Malformed Git object entry")?;
        if self::path(&record[tab + 1..])? != path {
            continue;
        }
        let fields: Vec<_> = record[..tab].split(|b| *b == b' ').collect();
        if fields.len() != 3 {
            return Err("Malformed Git object entry".into());
        }
        if fields[0] != b"100644" && fields[0] != b"100755" {
            return Err("Unsupported Git content: symlink, tree or submodule".into());
        }
        if revision.is_none() && fields[2] != b"0" {
            return Err("Unresolved conflict; inspect or resolve the source file".into());
        }
        object = Some(
            std::str::from_utf8(if revision.is_some() {
                fields[2]
            } else {
                fields[1]
            })
            .map_err(err)?
            .to_string(),
        );
    }
    let Some(oid) = object else {
        return Ok(Content {
            bytes: Vec::new(),
            identity: "absent".into(),
        });
    };
    let size = runner.git(&repo.root, &refs(&args(&["cat-file", "-s", &oid])))?;
    let size: usize = std::str::from_utf8(line(&size))
        .map_err(err)?
        .parse()
        .map_err(err)?;
    if size > CONTENT_LIMIT {
        return Err("Unsupported diff: content exceeds 2 MiB".into());
    }
    let bytes = runner.git(&repo.root, &refs(&args(&["cat-file", "blob", &oid])))?;
    Ok(Content {
        bytes,
        identity: oid,
    })
}
fn working(request: &Request) -> Result<(Vec<u8>, String, bool), String> {
    if let Some(buffer) = &request.buffer
        && (buffer.dirty || request.path.try_exists().map_err(err)?)
    {
        if buffer.text.len_bytes() > CONTENT_LIMIT {
            return Err("Unsupported diff: buffer exceeds 2 MiB".into());
        }
        return Ok((
            buffer.text.to_string().into_bytes(),
            format!(
                "buffer B{}@{}{}",
                buffer.id,
                buffer.generation,
                if buffer.dirty {
                    " UNSAVED"
                } else {
                    " (loaded; may differ from disk)"
                }
            ),
            true,
        ));
    }
    match fs::symlink_metadata(&request.path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok((Vec::new(), "working disk (deleted)".into(), false))
        }
        Err(e) => Err(err(e)),
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err("Unsupported working content: not a regular file".into());
            }
            if meta.len() as usize > CONTENT_LIMIT {
                return Err("Unsupported diff: file exceeds 2 MiB".into());
            }
            let mut bytes = Vec::new();
            File::open(&request.path)
                .map_err(err)?
                .take(CONTENT_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(err)?;
            if bytes.len() > CONTENT_LIMIT {
                return Err("Unsupported diff: file exceeds 2 MiB".into());
            }
            Ok((bytes, "working disk".into(), true))
        }
    }
}
fn compare_bytes(runner: &Runner<'_>, old: &[u8], new: &[u8]) -> Result<Diff, String> {
    for bytes in [old, new] {
        if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
            return Err("Unsupported diff: binary or non-UTF-8 content".into());
        }
        if bytes.len() > CONTENT_LIMIT {
            return Err("Unsupported diff: content exceeds 2 MiB".into());
        }
    }
    let dir = tempfile::tempdir().map_err(err)?;
    fs::write(dir.path().join("old"), old).map_err(err)?;
    fs::write(dir.path().join("new"), new).map_err(err)?;
    let result = runner.run(
        dir.path(),
        &refs(&args(&[
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--text",
            "--no-renames",
            "--diff-algorithm=myers",
            "--no-indent-heuristic",
            "--unified=3",
            "--inter-hunk-context=0",
            "--",
            "old",
            "new",
        ])),
    )?;
    if !matches!(result.code, Some(0 | 1)) {
        return Err(format!("Git diff failed: {}", result.error));
    }
    Diff::parse(std::str::from_utf8(&result.bytes).map_err(err)?)
}
fn comparison(runner: &Runner<'_>, request: &Request, snapshot: &Snapshot) -> Result<Diff, String> {
    let repo = &snapshot.repository;
    let relative = request.path.strip_prefix(&repo.root).map_err(err)?;
    let entry = snapshot.entries.iter().find(|e| e.path == relative);
    if entry.is_some_and(|e| e.conflict) {
        return Err("Unresolved conflict; inspect or resolve the source file".into());
    }
    let revision = match &request.comparison {
        Comparison::Index => None,
        Comparison::Head | Comparison::Staged => snapshot.head.clone(),
        Comparison::Revision(value) => {
            let bytes = runner
                .git(
                    &repo.root,
                    &refs(&args(&[
                        "rev-parse",
                        "--verify",
                        "--end-of-options",
                        &format!("{value}^{{commit}}"),
                    ])),
                )
                .map_err(|e| format!("Unavailable revision {value}: {e}"))?;
            Some(std::str::from_utf8(line(&bytes)).map_err(err)?.into())
        }
    };
    let mut old_path = if request.comparison != Comparison::Index {
        entry
            .and_then(|e| e.original.clone())
            .unwrap_or_else(|| relative.to_path_buf())
    } else {
        relative.to_path_buf()
    };
    if let Some(oid) = &revision {
        let mut command = args(&[
            "diff",
            "--name-status",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--ignore-submodules=all",
        ]);
        if request.comparison == Comparison::Staged {
            command.push("--cached".into());
        }
        command.push(oid.into());
        command.push("--".into());
        let names = runner.git(&repo.root, &refs(&command))?;
        let mut records = names.split(|b| *b == 0).filter(|r| !r.is_empty());
        while let Some(status) = records.next() {
            let first = records.next().ok_or("Missing changed path")?;
            if matches!(status[0], b'R' | b'C') {
                let second = records.next().ok_or("Missing renamed path")?;
                if path(second)? == relative {
                    old_path = path(first)?;
                }
            }
        }
    }
    let old = if request.comparison == Comparison::Index {
        blob(runner, repo, None, relative)?
    } else if let Some(oid) = &revision {
        blob(runner, repo, Some(oid), &old_path)?
    } else {
        Content {
            bytes: Vec::new(),
            identity: "empty (unborn HEAD)".into(),
        }
    };
    let (work, work_label, available) = working(request)?;
    let (new, right) = if request.comparison == Comparison::Staged {
        let content = blob(runner, repo, None, relative)?;
        (content.bytes, format!("index@{}", short(&content.identity)))
    } else {
        (work.clone(), work_label)
    };
    let mut diff = compare_bytes(runner, &old.bytes, &new)?;
    let base = match &request.comparison {
        Comparison::Staged => "HEAD",
        other => other.label(),
    };
    diff.left_label = format!(
        "{base}@{}:{} [blob {}]",
        revision
            .as_deref()
            .map(short)
            .unwrap_or_else(|| short(&old.identity)),
        display_path(&old_path),
        short(&old.identity)
    );
    diff.right_label = format!("{right}:{}", display_path(relative));
    diff.source_available = available;
    if request.comparison == Comparison::Staged {
        let mapping = compare_bytes(runner, &new, &work)?;
        for row in &mut diff.rows {
            row.anchor = mapping.map_old(row.anchor);
        }
    }
    Ok(diff)
}
fn short(value: &str) -> String {
    value.chars().take(12).collect()
}
