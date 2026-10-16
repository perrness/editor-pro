//! File discovery and fuzzy matching run off the UI thread. Nucleo owns ranking,
//! query cancellation, and result snapshots; the editor only chooses a result.
use crate::{
    editor::{Editor, View},
    input::Key,
    text::pop_grapheme,
};
use ignore::WalkBuilder;
use nucleo::{
    Config, Nucleo,
    pattern::{CaseMatching, Normalization},
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

const FILE_LIMIT: usize = 100_000;
#[derive(Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub display: String,
}
#[derive(Default)]
struct ScanResult {
    limited: bool,
    errors: usize,
}
pub struct FilePicker {
    pub root: PathBuf,
    pub query: String,
    pub selected: u32,
    pub scanning: bool,
    pub limited: bool,
    pub errors: usize,
    matcher: Nucleo<Candidate>,
    matching: bool,
    cancel: Arc<AtomicBool>,
    scan_result: mpsc::Receiver<ScanResult>,
}
impl FilePicker {
    pub fn new(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::other("File picker root must be a directory"));
        }
        let matcher = Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(|| {}), Some(2), 1);
        let injector = matcher.injector();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_root = root.clone();
        let (sender, scan_result) = mpsc::channel();
        thread::Builder::new()
            .name("file-discovery".into())
            .spawn(move || {
                let mut result = ScanResult::default();
                let mut count = 0;
                let walker = WalkBuilder::new(&worker_root)
                    .hidden(false)
                    .follow_links(false)
                    .require_git(false)
                    .filter_entry(|entry| {
                        !matches!(entry.file_name().to_str(), Some(".git" | ".hg" | ".svn"))
                    })
                    .sort_by_file_path(|a, b| a.cmp(b))
                    .build();
                for entry in walker {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => {
                            result.errors += 1;
                            continue;
                        }
                    };
                    if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                        continue;
                    }
                    if count == FILE_LIMIT {
                        result.limited = true;
                        break;
                    }
                    let Ok(relative) = entry.path().strip_prefix(&worker_root) else {
                        continue;
                    };
                    let candidate = Candidate {
                        path: relative.to_path_buf(),
                        display: relative.to_string_lossy().into_owned(),
                    };
                    injector.push(candidate, |candidate, columns| {
                        columns[0] = candidate.display.as_str().into()
                    });
                    count += 1;
                }
                let _ = sender.send(result);
            })?;
        Ok(Self {
            root,
            query: String::new(),
            selected: 0,
            scanning: true,
            limited: false,
            errors: 0,
            matcher,
            matching: true,
            cancel,
            scan_result,
        })
    }
    pub fn set_query(&mut self, query: String) {
        let append = query.starts_with(&self.query);
        self.matcher
            .pattern
            .reparse(0, &query, CaseMatching::Smart, Normalization::Smart, append);
        self.query = query;
        self.selected = 0;
        self.matching = true;
    }
    fn ready(&self) -> bool {
        self.matcher.snapshot().pattern().column_pattern(0).atoms
            == self.matcher.pattern.column_pattern(0).atoms
    }
    pub fn tick(&mut self) -> bool {
        let was_working = self.working();
        if self.scanning {
            match self.scan_result.try_recv() {
                Ok(result) => {
                    self.scanning = false;
                    self.limited = result.limited;
                    self.errors = result.errors;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scanning = false;
                    self.errors += 1;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let status = self.matcher.tick(0);
        self.matching = status.running;
        self.selected = self.selected.min(self.match_count().saturating_sub(1));
        status.changed || was_working != self.working()
    }
    pub fn working(&self) -> bool {
        self.scanning || self.matching || !self.ready()
    }
    pub fn item_count(&self) -> u32 {
        self.matcher.snapshot().item_count()
    }
    pub fn match_count(&self) -> u32 {
        if self.ready() {
            self.matcher.snapshot().matched_item_count()
        } else {
            0
        }
    }
    pub fn candidate(&self, index: u32) -> Option<&Candidate> {
        if !self.ready() {
            return None;
        }
        self.matcher
            .snapshot()
            .get_matched_item(index)
            .map(|item| item.data)
    }
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.candidate(self.selected)
            .map(|c| self.root.join(&c.path))
    }
}
impl Drop for FilePicker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Editor {
    pub fn open_picker(&mut self, root: &Path) -> io::Result<()> {
        let picker = FilePicker::new(root)?;
        self.document.buffer.commit();
        self.reset_context();
        self.picker = Some(picker);
        self.view = View::Files;
        self.info("Type to find files; Enter opens, Escape returns");
        Ok(())
    }
    pub fn tick(&mut self) -> bool {
        // Workers keep running, but applying a result can jump to a definition,
        // finish a note, or replace a contextual list. Keep the help origin intact.
        if self.help.is_some() {
            return false;
        }
        let picker = self.picker.as_mut().is_some_and(FilePicker::tick);
        let tree = self.tick_tree();
        let git = self.tick_git();
        let lsp = self.tick_lsp();
        let project_search = self.tick_project_search();
        let ai = self.tick_ai();
        self.tick_review() || tree || git || picker || lsp || project_search || ai
    }
    pub(crate) fn picker_key(&mut self, key: Key) {
        if matches!(key, Key::Escape | Key::Ctrl('c' | '[')) {
            self.picker = None;
            self.view = View::Editing;
            return;
        }
        let Some(picker) = &mut self.picker else {
            return;
        };
        match key {
            Key::Char(ch) if !ch.is_control() => {
                let mut query = picker.query.clone();
                query.push(ch);
                picker.set_query(query);
            }
            Key::Backspace => {
                let mut query = picker.query.clone();
                pop_grapheme(&mut query);
                picker.set_query(query);
            }
            Key::Down | Key::Ctrl('n' | 'j') | Key::Tab => {
                picker.selected = (picker.selected + 1).min(picker.match_count().saturating_sub(1))
            }
            Key::Up | Key::Ctrl('p' | 'k') => picker.selected = picker.selected.saturating_sub(1),
            Key::Enter => {
                if let Some(path) = picker.selected_path()
                    && let Err(error) = self.open_buffer(&path)
                {
                    self.error(&error.to_string());
                }
            }
            _ => {}
        }
    }
}
