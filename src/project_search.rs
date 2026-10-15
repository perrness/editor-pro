//! Bounded, cancellable ripgrep searches. All discovery/process work stays off
//! the editing thread; results describe disk text, never unsaved buffers.
use crate::{
    editor::{Editor, Mode, View},
    file::Document,
    input::Key,
};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
const RESULT_LIMIT: usize = 4096;

#[derive(Clone)]
pub struct Config {
    pub executable: PathBuf,
    pub root: Option<PathBuf>,
    pub timeout: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            executable: "rg".into(),
            root: None,
            timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub byte_column: usize,
    pub text: String,
}
pub struct Results {
    pub root: PathBuf,
    pub hits: Vec<SearchHit>,
    pub limited: bool,
    pub skipped: usize,
}
struct Request {
    query: String,
    source: PathBuf,
    root: Option<PathBuf>,
}
struct Reply {
    serial: u64,
    result: io::Result<Results>,
}
#[derive(Default)]
struct Queue {
    pending: Option<(u64, Request)>,
    result: Option<Reply>,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    serial: AtomicU64,
}
struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn new(config: Config) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        let thread = thread::Builder::new()
            .name("project-search".into())
            .spawn(move || {
                loop {
                    let mut queue = state.queue.lock().unwrap();
                    while !queue.closed && queue.pending.is_none() {
                        queue = state.wake.wait(queue).unwrap();
                    }
                    if queue.closed {
                        break;
                    }
                    let (serial, request) = queue.pending.take().unwrap();
                    drop(queue);
                    let result = search(&config, request, &state.serial, serial);
                    let mut queue = state.queue.lock().unwrap();
                    if !queue.closed && state.serial.load(Ordering::Acquire) == serial {
                        queue.result = Some(Reply { serial, result });
                    }
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
    fn submit(&self, request: Request) -> u64 {
        let mut queue = self.shared.queue.lock().unwrap();
        let serial = self.shared.serial.fetch_add(1, Ordering::AcqRel) + 1;
        queue.pending = Some((serial, request));
        queue.result = None;
        self.shared.wake.notify_one();
        serial
    }
    fn cancel(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        self.shared.serial.fetch_add(1, Ordering::AcqRel);
        queue.pending = None;
        queue.result = None;
    }
    fn poll(&self) -> Option<Reply> {
        self.shared.queue.lock().unwrap().result.take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.wake.notify_one();
        // The persistent worker is dropped only at editor shutdown, after terminal restoration.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
pub struct ProjectSearch {
    pub config: Config,
    worker: Option<Worker>,
    serial: u64,
    pub query: String,
    pub pending: bool,
    pub results: Option<Results>,
    pub error: Option<String>,
    pub selected: usize,
    top: usize,
    left: usize,
}
impl ProjectSearch {
    pub(crate) fn cancel(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancel();
        }
        self.pending = false;
        self.results = None;
    }
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
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
fn read_output(file: &mut File) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(OUTPUT_LIMIT).read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn root_for(request: &Request) -> io::Result<PathBuf> {
    if let Some(root) = &request.root {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::other("Search root must be a directory"));
        }
        return Ok(root);
    }
    if let Some(language) = crate::lsp::Language::for_path(&request.source) {
        return crate::lsp::discover_root(&request.source, language);
    }
    let parent = if request.source.is_dir() {
        request.source.as_path()
    } else {
        request
            .source
            .parent()
            .ok_or_else(|| io::Error::other("No search directory"))?
    };
    for directory in parent.ancestors() {
        if directory.join(".git").exists() || directory.join("Cargo.toml").is_file() {
            return Ok(directory.into());
        }
    }
    Ok(parent.into())
}
fn search(
    config: &Config,
    request: Request,
    current: &AtomicU64,
    serial: u64,
) -> io::Result<Results> {
    let deadline = Instant::now() + config.timeout;
    let check = || {
        if current.load(Ordering::Acquire) != serial {
            Err(io::Error::other("Search cancelled"))
        } else if Instant::now() >= deadline {
            Err(io::Error::other("Project search timed out"))
        } else {
            Ok(())
        }
    };
    check()?;
    let root = root_for(&request)?;
    check()?;
    let mut out = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut command = Command::new(&config.executable);
    // NUL terminates raw filenames. After that, each record is line:byte-column:text\n.
    // vimgrep emits one record per match, including multiple matches on a line.
    command
        .current_dir(&root)
        .args([
            "--no-config",
            "--vimgrep",
            "--null",
            "--fixed-strings",
            "--case-sensitive",
            "--hidden",
            "--no-follow",
            "--color",
            "never",
            "--encoding",
            "none",
            "--max-filesize",
            "2M",
            "--glob",
            "!.git",
            "--glob",
            "!.hg",
            "--glob",
            "!.svn",
            "--",
            &request.query,
            ".",
        ])
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut process = Process(
        command
            .spawn()
            .map_err(|e| io::Error::other(format!("Cannot run ripgrep (rg): {e}")))?,
    );
    let mut limited = false;
    let status = loop {
        check()?;
        if out.metadata()?.len() >= OUTPUT_LIMIT {
            limited = true;
            break None;
        }
        if stderr.metadata()?.len() >= OUTPUT_LIMIT {
            return Err(io::Error::other("ripgrep error output exceeds 16 MiB"));
        }
        if let Some(status) = process.0.try_wait()? {
            break Some(status);
        }
        thread::sleep(Duration::from_millis(2));
    };
    drop(process);
    check()?;
    limited |= out.metadata()?.len() > OUTPUT_LIMIT;
    let errors = read_output(&mut stderr)?;
    if status.is_some_and(|s| !s.success() && s.code() != Some(1)) || !errors.is_empty() {
        return Err(io::Error::other(format!(
            "ripgrep failed: {}",
            String::from_utf8_lossy(&errors)
                .chars()
                .take(4096)
                .collect::<String>()
        )));
    }
    let bytes = read_output(&mut out)?;
    let mut results = Results {
        root,
        hits: Vec::new(),
        limited,
        skipped: 0,
    };
    let mut remaining = bytes.as_slice();
    while !remaining.is_empty() {
        check()?;
        if results.hits.len() == RESULT_LIMIT {
            results.limited = true;
            break;
        }
        let Some(zero) = remaining.iter().position(|b| *b == 0) else {
            if limited {
                break;
            }
            return Err(io::Error::other("Malformed ripgrep filename record"));
        };
        let raw_path = &remaining[..zero];
        remaining = &remaining[zero + 1..];
        let Some(end) = remaining.iter().position(|b| *b == b'\n') else {
            if limited {
                break;
            }
            return Err(io::Error::other("Incomplete ripgrep result"));
        };
        let row = &remaining[..end];
        remaining = &remaining[end + 1..];
        let mut fields = row.splitn(3, |b| *b == b':');
        let number = |bytes: Option<&[u8]>| -> io::Result<usize> {
            bytes
                .and_then(|b| std::str::from_utf8(b).ok())
                .and_then(|s| s.parse::<usize>().ok())
                .and_then(|n| n.checked_sub(1))
                .ok_or_else(|| io::Error::other("Invalid ripgrep coordinate"))
        };
        let line = number(fields.next())?;
        let byte_column = number(fields.next())?;
        let Some(text) = fields
            .next()
            .and_then(|b| std::str::from_utf8(b).ok())
            .filter(|s| !s.contains('\0'))
        else {
            results.skipped += 1;
            continue;
        };
        if !text
            .get(byte_column..)
            .is_some_and(|s| s.starts_with(&request.query))
        {
            return Err(io::Error::other("Invalid ripgrep match offset"));
        }
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(raw_path.to_vec()))
        };
        #[cfg(not(unix))]
        let path = PathBuf::from(std::str::from_utf8(raw_path).map_err(io::Error::other)?);
        if path
            .components()
            .any(|c| !matches!(c, Component::CurDir | Component::Normal(_)))
        {
            return Err(io::Error::other(
                "ripgrep returned a path outside the search root",
            ));
        }
        let path = results.root.join(path.strip_prefix(".").unwrap_or(&path));
        results.hits.push(SearchHit {
            path,
            line,
            byte_column,
            text: text.into(),
        });
    }
    results
        .hits
        .sort_by(|a, b| (&a.path, a.line, a.byte_column).cmp(&(&b.path, b.line, b.byte_column)));
    Ok(results)
}

impl Editor {
    pub fn search_project(&mut self, query: &str) -> io::Result<()> {
        if self.note_edit.is_some() {
            return Err(io::Error::other("Return to source before project search"));
        }
        if query.is_empty() || query.len() > 4096 || query.contains(['\0', '\r', '\n']) {
            return Err(io::Error::other(
                "Use :grep LITERAL_TEXT (one line, at most 4096 bytes)",
            ));
        }
        let source = self
            .document
            .path()
            .map(Path::to_path_buf)
            .map_or_else(std::env::current_dir, Ok)?;
        let root = self
            .project_search
            .config
            .root
            .clone()
            .or_else(|| self.lsp.routes.get(&source).map(|k| k.root.clone()));
        if self.project_search.worker.is_none() {
            self.project_search.worker = Some(Worker::new(self.project_search.config.clone())?);
        }
        self.document.buffer.commit();
        self.reset_context();
        let state = &mut self.project_search;
        state.serial = state.worker.as_ref().unwrap().submit(Request {
            query: query.into(),
            source,
            root,
        });
        state.query = query.into();
        state.pending = true;
        state.error = None;
        state.selected = 0;
        state.top = 0;
        state.left = 0;
        self.view = View::ProjectSearch;
        self.info("Searching disk text; Escape cancels and returns");
        Ok(())
    }
    pub(crate) fn tick_project_search(&mut self) -> bool {
        if self.view != View::ProjectSearch && self.project_search.pending {
            self.project_search.cancel();
            return true;
        }
        let Some(reply) = self.project_search.worker.as_ref().and_then(Worker::poll) else {
            return false;
        };
        if reply.serial != self.project_search.serial || self.view != View::ProjectSearch {
            return false;
        }
        self.project_search.pending = false;
        match reply.result {
            Ok(results) => self.project_search.results = Some(results),
            Err(e) => {
                self.project_search.error = Some(e.to_string());
                self.error(&e.to_string());
            }
        }
        true
    }
    pub(crate) fn project_search_key(&mut self, key: Key) {
        let state = &mut self.project_search;
        let count = state.results.as_ref().map_or(0, |r| r.hits.len());
        match key {
            Key::Escape | Key::Ctrl('c' | '[') => {
                state.cancel();
                self.view = View::Editing;
            }
            Key::Char(':') => {
                self.mode = Mode::Command;
                self.command.clear();
            }
            Key::Down | Key::Char('j') | Key::Ctrl('n') => {
                state.selected = (state.selected + 1).min(count.saturating_sub(1))
            }
            Key::Up | Key::Char('k') | Key::Ctrl('p') => {
                state.selected = state.selected.saturating_sub(1)
            }
            Key::PageDown => state.selected = (state.selected + 10).min(count.saturating_sub(1)),
            Key::PageUp => state.selected = state.selected.saturating_sub(10),
            Key::Char('g') => state.selected = 0,
            Key::Char('G') => state.selected = count.saturating_sub(1),
            Key::Left | Key::Char('h') => state.left = state.left.saturating_sub(4),
            Key::Right | Key::Char('l') => state.left = state.left.saturating_add(4),
            Key::Char('r') => {
                let query = state.query.clone();
                if let Err(e) = self.search_project(&query) {
                    self.error(&e.to_string());
                }
            }
            Key::Enter => {
                if let Some(hit) = state
                    .results
                    .as_ref()
                    .and_then(|r| r.hits.get(state.selected))
                    .cloned()
                    && let Err(e) = self.open_search_hit(&hit)
                {
                    self.error(&e.to_string());
                }
            }
            _ => {}
        }
    }
    fn open_search_hit(&mut self, hit: &SearchHit) -> io::Result<()> {
        let id = if self.document.path() == Some(&hit.path) {
            Some(self.active_buffer_id)
        } else {
            self.buffers
                .iter()
                .find_map(|(id, s)| (s.document.path() == Some(&hit.path)).then_some(*id))
        };
        let target = if id.is_none() {
            Some(Document::open(&hit.path)?)
        } else {
            None
        };
        let document = if id == Some(self.active_buffer_id) {
            &self.document
        } else if let Some(id) = id {
            &self.buffers[&id].document
        } else {
            target.as_ref().unwrap()
        };
        let text = document.buffer.text();
        let line = text
            .get_line(hit.line)
            .ok_or_else(|| io::Error::other("Search location is stale; r searches again"))?
            .to_string();
        if line.strip_suffix('\n').unwrap_or(&line) != hit.text {
            return Err(io::Error::other(
                "Disk match differs from current text; return to source or r searches again",
            ));
        }
        let prefix = hit
            .text
            .get(..hit.byte_column)
            .ok_or_else(|| io::Error::other("Invalid search byte column"))?;
        let scalar = text.line_to_char(hit.line) + prefix.chars().count();
        let origin = self.source_location();
        if let Some(document) = target {
            self.open_prepared_buffer(document);
        } else {
            self.switch_buffer_unrecorded(id.unwrap())?;
        }
        self.document.buffer.set_cursor(scalar, false);
        self.record_navigation(origin);
        self.info("Project search location; Ctrl-o returns");
        Ok(())
    }
}

pub fn render(frame: &mut ratatui::Frame<'_>, editor: &mut Editor, theme: &crate::theme::Theme) {
    use ratatui::{layout::Rect, style::Style, widgets::Paragraph};
    let area = frame.area();
    let state = &mut editor.project_search;
    let title = if state.pending {
        format!("Searching disk: {}…", state.query)
    } else if let Some(results) = &state.results {
        format!(
            "Disk search: {} · {} matches{} · {} skipped · {}",
            state.query,
            results.hits.len(),
            if results.limited { "+ (limit)" } else { "" },
            results.skipped,
            crate::git::display_path(&results.root)
        )
    } else {
        state
            .error
            .clone()
            .unwrap_or_else(|| "Project search".into())
    };
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let visible = |s: &str, left| crate::text::visible_text(s, left, area.width as usize);
    frame.render_widget(
        Paragraph::new(visible(&title, 0)).style(style.fg(theme.command)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let height = area.height.saturating_sub(2) as usize;
    state.top = state.top.min(state.selected);
    if height > 0 && state.selected >= state.top + height {
        state.top = state.selected + 1 - height;
    }
    if let Some(results) = &state.results {
        for (row, hit) in results.hits.iter().skip(state.top).take(height).enumerate() {
            let label = format!(
                "{}:{}:{} {}",
                crate::git::display_path(hit.path.strip_prefix(&results.root).unwrap_or(&hit.path)),
                hit.line + 1,
                hit.byte_column + 1,
                hit.text.chars().take(512).collect::<String>()
            );
            frame.render_widget(
                Paragraph::new(visible(&label, state.left)).style(
                    if row + state.top == state.selected {
                        style.bg(theme.selection)
                    } else {
                        style
                    },
                ),
                Rect::new(area.x, area.y + row as u16 + 1, area.width, 1),
            );
        }
    }
    if area.height > 1 {
        let footer = if editor.mode == Mode::Command {
            format!(":{}", editor.command)
        } else if editor.message_is_error {
            editor.message.clone()
        } else {
            "Enter open · Esc cancel/return · r refresh · :grep TEXT · disk only; files ≤2 MiB"
                .into()
        };
        frame.render_widget(
            Paragraph::new(visible(
                &footer,
                if editor.mode == Mode::Command {
                    crate::text::display_column(&footer, footer.chars().count())
                        .saturating_sub(area.width.saturating_sub(1) as usize)
                } else {
                    0
                },
            ))
            .style(style.fg(if editor.message_is_error {
                theme.error
            } else {
                theme.muted
            })),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
        if editor.mode == Mode::Command {
            let column = crate::text::display_column(&footer, footer.chars().count())
                .min(area.width.saturating_sub(1) as usize);
            frame.set_cursor_position((area.x + column as u16, area.bottom() - 1));
        }
    }
}
