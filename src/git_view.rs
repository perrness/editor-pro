//! Git contextual views share the editor's documents and own only navigation and
//! derived results. They never replace source text or its undo history.
use crate::{
    diff::{Diff, Mark},
    editor::{Editor, Mode, View, Viewport},
    git::{self, BufferVersion, Comparison, Request, Snapshot, Worker},
    input::Key,
};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Origin {
    pub buffer_id: usize,
    pub cursor: usize,
    pub viewport: Viewport,
}
pub struct DiffView {
    pub origin: Origin,
    pub path: PathBuf,
    pub selected: usize,
    pub top: usize,
    pub left: usize,
    pub paired: bool,
    pub effective_paired: bool,
    pub height: usize,
    pub anchor: usize,
}
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    buffer: Option<(usize, u64, bool)>,
    comparison: Comparison,
    inspect: bool,
    review: u64,
    review_buffers: Vec<(PathBuf, usize, u64, bool)>,
}
impl Stamp {
    fn new(request: &Request) -> Self {
        Self {
            path: request.path.clone(),
            buffer: request
                .buffer
                .as_ref()
                .map(|b| (b.id, b.generation, b.dirty)),
            comparison: request.comparison.clone(),
            inspect: request.inspect,
            review: request.review_generation,
            review_buffers: request
                .review_buffers
                .iter()
                .map(|(p, b)| (p.clone(), b.id, b.generation, b.dirty))
                .collect(),
        }
    }
}
pub struct GitState {
    pub comparison: Comparison,
    pub snapshot: Option<Snapshot>,
    pub message: String,
    pub loading: bool,
    pub diff_view: Option<DiffView>,
    pub selected: usize,
    worker: Option<Worker>,
    serial: u64,
    stamp: Option<Stamp>,
    refreshed: Instant,
}
impl Default for GitState {
    fn default() -> Self {
        Self {
            comparison: Comparison::Head,
            snapshot: None,
            message: "Git not loaded".into(),
            loading: false,
            diff_view: None,
            selected: 0,
            worker: None,
            serial: 0,
            stamp: None,
            refreshed: Instant::now(),
        }
    }
}
impl GitState {
    pub fn invalidate(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancel();
        }
        self.stamp = None;
        self.loading = true;
    }
    pub fn visible(&self) -> bool {
        self.stamp.is_some() || self.loading
    }
    pub fn problem(&self) -> Option<&str> {
        if !self.loading
            && !self.message.is_empty()
            && self.visible()
            && !self.message.starts_with("Git:")
        {
            return Some(&self.message);
        }
        if !self.loading {
            self.snapshot
                .as_ref()
                .and_then(|s| s.diff.as_ref().err())
                .map(String::as_str)
        } else {
            None
        }
    }
    pub fn summary(&self) -> String {
        if let Some(snapshot) = &self.snapshot {
            format!(
                "{}{}",
                snapshot.summary(),
                if self.loading {
                    " [stale/loading]"
                } else if !self.message.is_empty() || snapshot.diff.is_err() {
                    " [error/stale]"
                } else {
                    ""
                }
            )
        } else if self.loading {
            "Git loading…".into()
        } else {
            self.message.clone()
        }
    }
    pub fn diff(&self) -> Option<&Diff> {
        self.snapshot.as_ref()?.diff.as_ref().ok()
    }
    pub fn ready(&self) -> bool {
        !self.loading && self.message.is_empty() && self.snapshot.is_some()
    }
}
#[derive(Clone)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub label: String,
    pub deleted: bool,
}
impl Editor {
    pub fn refresh_git(&mut self) {
        self.git.invalidate();
    }
    fn git_request(&self) -> Option<Request> {
        let path = self
            .git
            .diff_view
            .as_ref()
            .map(|d| d.path.clone())
            .or_else(|| self.document.path().map(PathBuf::from));
        let inspect = path.is_some();
        let path = path.or_else(|| {
            (self.view == View::Changes)
                .then(|| {
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.join(".editor-pro-context"))
                })
                .flatten()
        })?;
        let buffer = std::iter::once((self.active_buffer_id, &self.document))
            .chain(self.buffers.iter().map(|(id, s)| (*id, &s.document)))
            .find(|(_, d)| d.path() == Some(path.as_path()))
            .map(|(id, d)| BufferVersion {
                id,
                generation: d.buffer.generation(),
                dirty: d.buffer.is_modified(),
                text: d.buffer.text().clone(),
            });
        Some(Request {
            path,
            buffer,
            comparison: self.git.comparison.clone(),
            inspect,
            review: self.review.store.clone(),
            review_generation: self.review.generation,
            review_buffers: std::iter::once((self.active_buffer_id, &self.document))
                .chain(self.buffers.iter().map(|(id, s)| (*id, &s.document)))
                .filter_map(|(id, d)| {
                    d.path().map(|p| {
                        (
                            p.to_path_buf(),
                            BufferVersion {
                                id,
                                generation: d.buffer.generation(),
                                dirty: d.buffer.is_modified(),
                                text: d.buffer.text().clone(),
                            },
                        )
                    })
                })
                .collect(),
        })
    }
    pub fn git_current(&self) -> bool {
        if self.note_edit.is_some() || !self.git.ready() {
            return false;
        }
        let Some(stamp) = &self.git.stamp else {
            return false;
        };
        let path = self
            .git
            .diff_view
            .as_ref()
            .map(|v| v.path.as_path())
            .or(self.document.path());
        if path.is_some_and(|p| p != stamp.path)
            || path.is_some() != stamp.inspect
            || stamp.comparison != self.git.comparison
            || stamp.review != self.review.generation
        {
            return false;
        }
        let documents = || {
            std::iter::once((self.active_buffer_id, &self.document))
                .chain(self.buffers.iter().map(|(id, s)| (*id, &s.document)))
        };
        let buffer = documents()
            .find(|(_, d)| d.path() == Some(stamp.path.as_path()))
            .map(|(id, d)| (id, d.buffer.generation(), d.buffer.is_modified()));
        if buffer != stamp.buffer {
            return false;
        }
        let current = documents().filter_map(|(id, d)| {
            d.path()
                .map(|p| (p, id, d.buffer.generation(), d.buffer.is_modified()))
        });
        stamp
            .review_buffers
            .iter()
            .map(|(p, id, g, dirty)| (p.as_path(), *id, *g, *dirty))
            .eq(current)
    }
    pub(crate) fn git_compared_unsaved(&self) -> bool {
        self.git
            .stamp
            .as_ref()
            .and_then(|s| s.buffer)
            .is_some_and(|(_, _, dirty)| dirty)
    }
    pub(crate) fn tick_git(&mut self) -> bool {
        if self.note_edit.is_some() {
            return false;
        }
        let Some(request) = self.git_request() else {
            if self.git.stamp.is_some() || self.git.loading {
                self.git.invalidate();
                self.git.snapshot = None;
                self.git.loading = false;
                self.git.message = "Git: unnamed buffer".into();
                return true;
            }
            return false;
        };
        let stamp = Stamp::new(&request);
        let changed = self.git.stamp.as_ref() != Some(&stamp);
        let periodic = !self.git.loading && self.git.refreshed.elapsed() >= Duration::from_secs(2);
        let mut redraw = false;
        if changed || periodic {
            if self.git.worker.is_none() {
                match Worker::new(git::Config::default()) {
                    Ok(worker) => self.git.worker = Some(worker),
                    Err(e) => {
                        self.git.message = format!("Git worker: {e}");
                        self.git.loading = false;
                        self.git.refreshed = Instant::now();
                        self.git.stamp = Some(stamp);
                        return true;
                    }
                }
            }
            // Retain stale data only for the same path and comparison.
            if self
                .git
                .stamp
                .as_ref()
                .is_some_and(|s| s.path != stamp.path || s.comparison != stamp.comparison)
            {
                self.git.snapshot = None;
            }
            self.git.serial = self.git.worker.as_ref().unwrap().submit(request);
            self.git.stamp = Some(stamp);
            self.git.loading = true;
            self.git.refreshed = Instant::now();
            redraw = true;
        }
        if let Some(reply) = self.git.worker.as_ref().and_then(Worker::poll)
            && reply.serial == self.git.serial
        {
            let selected_path = self
                .changed_files()
                .get(self.git.selected)
                .map(|f| f.path.clone());
            self.git.loading = false;
            self.git.refreshed = Instant::now();
            let header_selected = self
                .git
                .diff_view
                .as_ref()
                .and_then(|v| self.git.diff().and_then(|d| d.rows.get(v.selected)))
                .is_some_and(|r| r.kind == crate::diff::Kind::Header);
            match reply.result {
                Ok(Some(snapshot)) => {
                    if let Some(view) = &mut self.git.diff_view
                        && let Ok(diff) = &snapshot.diff
                    {
                        view.selected = if header_selected {
                            diff.hunks
                                .iter()
                                .min_by_key(|i| diff.rows[**i].anchor.abs_diff(view.anchor))
                                .copied()
                                .unwrap_or(0)
                        } else {
                            diff.nearest(view.anchor)
                        };
                    }
                    self.git.snapshot = Some(snapshot);
                    self.git.message.clear();
                }
                Ok(None) => {
                    self.git.snapshot = None;
                    self.git.message = "Git: outside repository".into();
                }
                Err(error) => self.git.message = error,
            }
            let files = self.changed_files();
            self.git.selected = selected_path
                .and_then(|p| files.iter().position(|f| f.path == p))
                .unwrap_or(self.git.selected)
                .min(files.len().saturating_sub(1));
            redraw = true;
        }
        redraw
    }
    pub fn changed_files(&self) -> Vec<ChangedFile> {
        let Some(snapshot) = &self.git.snapshot else {
            return Vec::new();
        };
        let root = &snapshot.repository.root;
        let mut files: Vec<_> = snapshot
            .entries
            .iter()
            .map(|e| ChangedFile {
                path: root.join(&e.path),
                label: format!(
                    "{}{}{} {}{}",
                    e.index,
                    e.worktree,
                    if e.conflict { " !conflict" } else { "" },
                    e.original
                        .as_ref()
                        .map(|p| format!("{} → ", git::display_path(p)))
                        .unwrap_or_default(),
                    git::display_path(&e.path)
                ),
                deleted: e.index == 'D' || e.worktree == 'D',
            })
            .collect();
        for (id, d) in std::iter::once((self.active_buffer_id, &self.document))
            .chain(self.buffers.iter().map(|(id, s)| (*id, &s.document)))
        {
            if !d.buffer.is_modified() {
                continue;
            }
            if let Some(path) = d.path()
                && let Ok(relative) = path.strip_prefix(root)
            {
                if let Some(file) = files.iter_mut().find(|f| f.path == path) {
                    file.label.push_str(&format!("  B{id}+ UNSAVED"));
                    file.deleted = false;
                } else {
                    files.push(ChangedFile {
                        path: path.into(),
                        label: format!(
                            ".. {}  B{id}+ UNSAVED (buffer only)",
                            git::display_path(relative)
                        ),
                        deleted: false,
                    });
                }
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }
    pub fn open_changes(&mut self) {
        if self.view == View::Diff {
            self.close_diff();
        }
        self.document.buffer.commit();
        self.reset_context();
        self.view = View::Changes;
        self.git.selected = 0;
        self.refresh_git();
        self.info("Changes: XY = index/disk; B+ = unsaved buffer. Enter source/history; d diff");
    }
    pub fn open_diff(&mut self, path: PathBuf, comparison: Comparison) {
        if self.git.diff_view.is_none() {
            self.document.buffer.commit();
            let origin = Origin {
                buffer_id: self.active_buffer_id,
                cursor: self.document.buffer.cursor(),
                viewport: self.viewport.clone(),
            };
            self.reset_context();
            self.git.diff_view = Some(DiffView {
                origin,
                path: path.clone(),
                selected: 0,
                top: 0,
                left: 0,
                paired: false,
                effective_paired: false,
                height: 1,
                anchor: self.document.buffer.position().0,
            });
        }
        let view = self.git.diff_view.as_mut().unwrap();
        if view.path != path {
            view.path = path;
            view.selected = 0;
            view.top = 0;
            view.anchor = 0;
        }
        self.git.comparison = comparison;
        self.view = View::Diff;
        self.mode = Mode::Normal;
        self.refresh_git();
        self.info("Diff: j/k scroll  ]/[ hunks  Tab layout  Enter source  n/p files  r refresh  Esc return");
    }
    pub fn close_diff(&mut self) {
        if let Some(view) = self.git.diff_view.take() {
            if self.active_buffer_id != view.origin.buffer_id {
                let _ = self.switch_buffer(view.origin.buffer_id);
            }
            self.document.buffer.set_cursor(view.origin.cursor, false);
            self.viewport = view.origin.viewport;
        }
        self.view = View::Editing;
        self.mode = Mode::Normal;
        if self.git.comparison == Comparison::Staged {
            self.git.comparison = Comparison::Head;
        }
        self.refresh_git();
    }
    pub fn jump_diff_source(&mut self) -> io::Result<()> {
        if !self.git.ready() {
            return Err(io::Error::other("Wait for a fresh Git comparison"));
        }
        let view = self
            .git
            .diff_view
            .as_ref()
            .ok_or_else(|| io::Error::other("No diff open"))?;
        let diff = self
            .git
            .diff()
            .ok_or_else(|| io::Error::other("No supported comparison"))?;
        if !diff.source_available {
            return Err(io::Error::other(
                "Deleted file has no current source; Escape returns to the origin",
            ));
        }
        let line = diff.rows.get(view.selected).map_or(0, |r| r.anchor);
        let path = view.path.clone();
        let staged = self.git.comparison == Comparison::Staged;
        let origin = self.source_location();
        self.open_git_source(&path)?;
        if staged {
            self.git.comparison = Comparison::Head;
        }
        self.document.buffer.goto_line(line, false);
        self.record_navigation(origin);
        self.view = View::Editing;
        self.git.diff_view = None;
        self.info("Source: deleted lines anchor to the following surviving line, or EOF");
        Ok(())
    }
    fn open_git_source(&mut self, path: &Path) -> io::Result<()> {
        // A known document survives deletion of its entire parent directory.
        // Reuse its identity without asking the filesystem to resolve that path.
        let id = std::iter::once((self.active_buffer_id, &self.document))
            .chain(self.buffers.iter().map(|(id, slot)| (*id, &slot.document)))
            .find_map(|(id, document)| (document.path() == Some(path)).then_some(id));
        if let Some(id) = id {
            self.switch_buffer(id)
        } else {
            self.open_buffer(path)
        }
    }
    pub fn cycle_changed_file(&mut self, backward: bool) -> io::Result<()> {
        if !self.git.ready() {
            return Err(io::Error::other("Wait for fresh Git status"));
        }
        let files = self.changed_files();
        if files.is_empty() {
            return Err(io::Error::other("No changed files"));
        }
        let current = self
            .git
            .diff_view
            .as_ref()
            .map(|v| v.path.as_path())
            .or(self.document.path());
        let index = current.and_then(|p| files.iter().position(|f| f.path == p));
        let index = match index {
            Some(i) if backward => (i + files.len() - 1) % files.len(),
            Some(i) => (i + 1) % files.len(),
            None if backward => files.len() - 1,
            None => 0,
        };
        let file = &files[index];
        if self.view == View::Diff || file.deleted {
            self.open_diff(file.path.clone(), self.git.comparison.clone());
            Ok(())
        } else {
            self.open_git_source(&file.path)
        }
    }
    pub fn git_mark(&self, line: usize) -> Option<Mark> {
        if !self.git.ready() || self.git.comparison == Comparison::Staged {
            return None;
        }
        let stamp = self.git.stamp.as_ref()?;
        if stamp.path != self.document.path()?
            || stamp.buffer
                != Some((
                    self.active_buffer_id,
                    self.document.buffer.generation(),
                    self.document.buffer.is_modified(),
                ))
        {
            return None;
        }
        let diff = self.git.diff()?;
        diff.marks.get(&line).copied().or_else(|| {
            // A deletion at EOF still has a visible anchor when no final empty row exists.
            (line + 1 == self.document.buffer.line_count())
                .then(|| {
                    diff.marks
                        .range(line + 1..)
                        .find_map(|(_, m)| (*m == Mark::Deleted).then_some(*m))
                })
                .flatten()
        })
    }
    pub(crate) fn git_key(&mut self, key: Key) {
        if self.view == View::Diff {
            let command = match key {
                Key::Char('F') => Some("file reviewed"),
                Key::Char('R') => Some("hunk reviewed"),
                Key::Char('A') => Some("hunk attention"),
                _ => None,
            };
            if let Some(command) = command {
                if let Err(e) = self.review_command(command) {
                    self.error(&e.to_string());
                }
                return;
            }
        }
        if key == Key::Char(':') {
            self.mode = Mode::Command;
            self.command.clear();
            return;
        }
        if matches!(key, Key::Escape | Key::Ctrl('c' | '[') | Key::Char('q')) {
            if self.view == View::Diff {
                self.close_diff();
            } else {
                self.view = View::Editing;
            }
            return;
        }
        if key == Key::Char('r') {
            self.refresh_git();
            return;
        }
        if self.view == View::Changes {
            let files = self.changed_files();
            match key {
                Key::Down | Key::Char('j') => {
                    self.git.selected = (self.git.selected + 1).min(files.len().saturating_sub(1))
                }
                Key::Up | Key::Char('k') => self.git.selected = self.git.selected.saturating_sub(1),
                Key::Enter | Key::Char('d') if self.git.ready() => {
                    if let Some(file) = files.get(self.git.selected) {
                        if key == Key::Char('d') || file.deleted {
                            self.open_diff(file.path.clone(), self.git.comparison.clone());
                        } else if let Err(e) = self.open_git_source(&file.path) {
                            self.error(&e.to_string());
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        match key {
            Key::Tab => {
                if let Some(view) = &mut self.git.diff_view {
                    view.paired = !view.paired;
                }
            }
            Key::Char('n' | 'p') => {
                if let Err(e) = self.cycle_changed_file(key == Key::Char('p')) {
                    self.error(&e.to_string());
                }
            }
            Key::Enter => {
                if let Err(e) = self.jump_diff_source() {
                    self.error(&e.to_string());
                }
            }
            Key::Char('1' | '2' | '3') => {
                self.git.comparison = match key {
                    Key::Char('2') => Comparison::Index,
                    Key::Char('3') => Comparison::Staged,
                    _ => Comparison::Head,
                };
                self.refresh_git();
            }
            _ => {
                let Some(diff) = self
                    .git
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.diff.as_ref().ok())
                else {
                    return;
                };
                let Some(view) = &mut self.git.diff_view else {
                    return;
                };
                let len = if view.effective_paired {
                    diff.pairs.len()
                } else {
                    diff.rows.len()
                };
                let current = if view.effective_paired {
                    diff.pair_index(view.selected)
                } else {
                    view.selected
                };
                let target = match key {
                    Key::Char('j') | Key::Down => Some((current + 1).min(len.saturating_sub(1))),
                    Key::Char('k') | Key::Up => Some(current.saturating_sub(1)),
                    Key::PageDown | Key::Ctrl('d') => {
                        Some((current + view.height.max(1) / 2 + 1).min(len.saturating_sub(1)))
                    }
                    Key::PageUp | Key::Ctrl('u') => {
                        Some(current.saturating_sub(view.height.max(1) / 2 + 1))
                    }
                    Key::Home | Key::Char('g') => Some(0),
                    Key::End | Key::Char('G') => Some(len.saturating_sub(1)),
                    _ => None,
                };
                if let Some(target) = target {
                    view.selected = if view.effective_paired {
                        diff.pairs
                            .get(target)
                            .and_then(|p| p.right.or(p.left))
                            .unwrap_or(0)
                    } else {
                        target
                    };
                }
                match key {
                    Key::Char(']') => {
                        if let Some(i) = diff.hunks.iter().find(|i| **i > view.selected) {
                            view.selected = *i;
                        }
                    }
                    Key::Char('[') => {
                        if let Some(i) = diff.hunks.iter().rev().find(|i| **i < view.selected) {
                            view.selected = *i;
                        }
                    }
                    Key::Char('l') | Key::Right => view.left = view.left.saturating_add(4),
                    Key::Char('h') | Key::Left => view.left = view.left.saturating_sub(4),
                    _ => {}
                }
                view.anchor = diff.rows.get(view.selected).map_or(0, |r| r.anchor);
            }
        }
    }
}
