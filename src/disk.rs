//! Bounded background disk sampling and explicit, guarded refreshes. Polling
//! never replaces a document or interrupts editing with a modal prompt.
use crate::{
    diff::Diff,
    editor::Editor,
    file::{DiskObservation as Observation, Document, read_optional},
    input::Key,
};
use ropey::Rope;
use sha2::{Digest, Sha256};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(2);
const QUIET: Duration = Duration::from_millis(150);
const BATCH: usize = 16;

impl Observation {
    fn read(bytes: &Option<Vec<u8>>, baseline: Option<&[u8]>) -> Self {
        Self {
            content: Ok(bytes.as_ref().map(|b| Sha256::digest(b).into())),
            changed: bytes.as_deref() != baseline,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    id: usize,
    path: PathBuf,
    instance: u64,
    baseline: u64,
}
struct Target {
    stamp: Stamp,
    baseline: Option<Arc<[u8]>>,
}
impl Target {
    fn new(id: usize, document: &Document) -> Option<Self> {
        Some(Self {
            stamp: Stamp {
                id,
                path: document.path()?.to_path_buf(),
                instance: document.buffer.instance(),
                baseline: document.baseline_generation,
            },
            baseline: document.baseline(),
        })
    }
}
impl Stamp {
    fn matches(&self, document: &Document) -> bool {
        document.path() == Some(self.path.as_path())
            && document.buffer.instance() == self.instance
            && document.baseline_generation == self.baseline
    }
}

struct Preview {
    bytes: Option<Arc<[u8]>>,
    observation: Observation,
    diff: Result<Diff, String>,
}
enum Job {
    Scan(Vec<Target>),
    Compare(Target, u64, Rope),
    Reload(Target, u64, Arc<[u8]>),
}
enum ResultData {
    Scan(Vec<(Stamp, Observation)>),
    Compare(Stamp, u64, Result<Preview, String>),
    Reload(Stamp, u64, Result<Document, String>),
}
#[derive(Default)]
struct Queue {
    pending: Option<(u64, Job)>,
    result: Option<(u64, ResultData)>,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    serial: AtomicU64,
}
struct Worker(Arc<Shared>);
impl Worker {
    fn new() -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        thread::Builder::new()
            .name("disk-refresh".into())
            .spawn(move || {
                loop {
                    let mut queue = state.queue.lock().unwrap();
                    while !queue.closed && queue.pending.is_none() {
                        queue = state.wake.wait(queue).unwrap();
                    }
                    if queue.closed {
                        return;
                    }
                    let (serial, job) = queue.pending.take().unwrap();
                    drop(queue);
                    let result = run(job, serial, &state.serial);
                    let mut queue = state.queue.lock().unwrap();
                    if !queue.closed && state.serial.load(Ordering::Acquire) == serial {
                        queue.result = Some((serial, result));
                    }
                }
            })?;
        Ok(Self(shared))
    }
    fn submit(&self, job: Job) -> u64 {
        let mut queue = self.0.queue.lock().unwrap();
        let serial = self.0.serial.fetch_add(1, Ordering::AcqRel) + 1;
        queue.pending = Some((serial, job));
        queue.result = None;
        self.0.wake.notify_one();
        serial
    }
    fn poll(&self) -> Option<(u64, ResultData)> {
        self.0.queue.lock().unwrap().result.take()
    }
    fn cancel(&self) {
        let mut queue = self.0.queue.lock().unwrap();
        self.0.serial.fetch_add(1, Ordering::AcqRel);
        queue.pending = None;
        queue.result = None;
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.0.queue.lock().unwrap().closed = true;
        self.0.wake.notify_one();
        // Read-only work can finish independently. A stalled filesystem must
        // not hold up input or terminal shutdown; no new worker is spawned.
    }
}
fn stable_read(
    path: &std::path::Path,
    serial: u64,
    current: &AtomicU64,
) -> Result<Option<Vec<u8>>, String> {
    let first = read_optional(path).map_err(|e| e.to_string())?;
    let until = Instant::now() + QUIET;
    while Instant::now() < until {
        if current.load(Ordering::Acquire) != serial {
            return Err("Cancelled".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let second = read_optional(path).map_err(|e| e.to_string())?;
    if first != second {
        return Err("File is still changing; c compares again after writes settle".into());
    }
    Ok(second)
}
fn run(job: Job, serial: u64, current: &AtomicU64) -> ResultData {
    match job {
        Job::Scan(targets) => {
            let mut observations = Vec::new();
            for target in targets {
                if current.load(Ordering::Acquire) != serial {
                    break;
                }
                let observation = match read_optional(&target.stamp.path) {
                    Ok(bytes) => Observation::read(&bytes, target.baseline.as_deref()),
                    Err(error) => Observation {
                        content: Err(error.to_string()),
                        changed: true,
                    },
                };
                observations.push((target.stamp, observation));
            }
            ResultData::Scan(observations)
        }
        Job::Compare(target, generation, text) => {
            let result = stable_read(&target.stamp.path, serial, current).map(|bytes| {
                let observation = Observation::read(&bytes, target.baseline.as_deref());
                let diff = if text.len_bytes() > crate::git::CONTENT_LIMIT {
                    Err("Comparison is limited to 2 MiB per side; reload or save-as is still available".into())
                } else {
                    crate::git::compare_disk_bytes(text.to_string().as_bytes(), bytes.as_deref().unwrap_or_default(), serial, current)
                };
                Preview { bytes: bytes.map(Arc::from), observation, diff }
            });
            ResultData::Compare(target.stamp, generation, result)
        }
        Job::Reload(target, generation, expected) => {
            let result = stable_read(&target.stamp.path, serial, current).and_then(|bytes| {
                if bytes.as_deref() != Some(expected.as_ref()) {
                    return Err(
                        "Disk changed since comparison; c compares again before reload".into(),
                    );
                }
                let document = Document::from_disk(target.stamp.path.clone(), Some(expected))
                    .map_err(|e| e.to_string())?;
                // Buffer/syntax preparation can take time. Check once more
                // before delivering; save still checks exact bytes afterward.
                if read_optional(&target.stamp.path)
                    .map_err(|e| e.to_string())?
                    .as_deref()
                    != document.baseline().as_deref()
                {
                    return Err("Disk changed during reload; c compares again".into());
                }
                Ok(document)
            });
            ResultData::Reload(target.stamp, generation, result)
        }
    }
}

pub struct RefreshView {
    stamp: Stamp,
    generation: u64,
    preview: Option<Preview>,
    pub message: String,
    pub confirming: bool,
    pub stale: bool,
    pub top: usize,
    pub left: usize,
}
pub struct DiskState {
    pub view: Option<RefreshView>,
    pub busy: bool,
    action_pending: bool,
    recheck_reload: bool,
    worker: Option<Worker>,
    serial: u64,
    next_scan: Instant,
    after: usize,
}
impl Default for DiskState {
    fn default() -> Self {
        Self {
            view: None,
            busy: false,
            action_pending: false,
            recheck_reload: false,
            worker: None,
            serial: 0,
            next_scan: Instant::now() + INTERVAL,
            after: 0,
        }
    }
}
impl DiskState {
    fn submit(&mut self, job: Job) -> io::Result<()> {
        if self.worker.is_none() {
            self.worker = Some(Worker::new()?);
        }
        self.action_pending = !matches!(job, Job::Scan(_));
        self.recheck_reload = false;
        self.serial = self.worker.as_ref().unwrap().submit(job);
        self.busy = true;
        self.next_scan = Instant::now() + INTERVAL;
        Ok(())
    }
}
impl Editor {
    fn disk_document(&self, id: usize) -> Option<&Document> {
        if id == self.active_buffer_id {
            Some(
                self.note_edit
                    .as_ref()
                    .map_or(&self.document, |n| &n.source),
            )
        } else {
            self.buffers.get(&id).map(|s| &s.document)
        }
    }
    fn disk_document_mut(&mut self, id: usize) -> Option<&mut Document> {
        if id == self.active_buffer_id {
            Some(
                self.note_edit
                    .as_mut()
                    .map_or(&mut self.document, |n| &mut n.source),
            )
        } else {
            self.buffers.get_mut(&id).map(|s| &mut s.document)
        }
    }
    /// Request an early scan, without reading or hashing files on the caller.
    pub fn check_external(&mut self) -> bool {
        self.disk.next_scan = Instant::now();
        self.tick_disk()
    }
    fn observe_disk(&mut self, stamp: &Stamp, observation: Observation) -> bool {
        let Some(document) = self
            .disk_document_mut(stamp.id)
            .filter(|d| stamp.matches(d))
        else {
            return false;
        };
        let before = document.external_changed;
        let changed = document
            .disk_observation
            .as_ref()
            .map_or(observation.changed, |old| old != &observation)
            || before != observation.changed;
        document.external_changed = observation.changed;
        document.disk_observation = Some(observation.clone());
        if let Some(view) = &mut self.disk.view
            && view.stamp == *stamp
            && view
                .preview
                .as_ref()
                .is_some_and(|p| p.observation.content != observation.content)
        {
            view.stale = true;
            view.confirming = false;
            view.message = "Disk changed again; c compares the latest version".into();
        }
        if changed {
            self.refresh_git();
            if observation.changed && !before {
                self.error(&format!(
                    "Buffer {} changed on disk. :refresh compares/reloads; :w NEW_PATH keeps edits",
                    stamp.id
                ));
            }
        }
        changed
    }
    pub(crate) fn tick_disk(&mut self) -> bool {
        let mut redraw = false;
        if self.help.is_some() && self.disk.action_pending {
            self.disk.recheck_reload = true;
        }
        // Defer explicit refresh replies under help, but keep silent scans alive.
        if !(self.help.is_some() && self.disk.view.is_some())
            && let Some((serial, result)) = self.disk.worker.as_ref().and_then(Worker::poll)
            && serial == self.disk.serial
        {
            self.disk.busy = false;
            self.disk.action_pending = false;
            redraw = true;
            match result {
                ResultData::Scan(results) => {
                    for (stamp, observation) in results {
                        self.observe_disk(&stamp, observation);
                    }
                }
                ResultData::Compare(stamp, generation, result) => {
                    if self.refresh_matches(&stamp, generation) {
                        if let Ok(preview) = &result {
                            self.observe_disk(&stamp, preview.observation.clone());
                        }
                        let view = self.disk.view.as_mut().unwrap();
                        match result {
                            Ok(preview) => {
                                view.message = if preview.bytes.is_none() {
                                    "File deleted or renamed; buffer retained. Cancel, then :w NEW_PATH to recover".into()
                                } else {
                                    "Comparison ready; c compare again, r reload, Esc cancel".into()
                                };
                                view.preview = Some(preview);
                                view.stale = false;
                            }
                            Err(error) => view.message = error,
                        }
                    } else {
                        self.stale_refresh();
                    }
                }
                ResultData::Reload(stamp, generation, result) => {
                    if self.refresh_matches(&stamp, generation) {
                        match result {
                            Ok(document) => {
                                if self.disk.recheck_reload {
                                    // A prepared result may have sat behind help
                                    // while another writer changed the file.
                                    let target = Target::new(stamp.id, &self.document).unwrap();
                                    if let Err(error) = self.disk.submit(Job::Reload(
                                        target,
                                        generation,
                                        document.baseline().unwrap(),
                                    )) {
                                        self.disk.view.as_mut().unwrap().message =
                                            error.to_string();
                                    }
                                } else {
                                    self.disk.view = None;
                                    self.replace_reloaded_document(document);
                                    self.info("Reloaded from disk; cursor and viewport retained where possible");
                                }
                            }
                            Err(error) => {
                                let view = self.disk.view.as_mut().unwrap();
                                view.message = error;
                                view.stale = true;
                                view.confirming = false;
                            }
                        }
                    } else {
                        self.stale_refresh();
                    }
                }
            }
        }
        if !self.disk.busy && Instant::now() >= self.disk.next_scan {
            // Stable ID order and a rotating cursor bound each batch without
            // starving inactive buffers. Baselines are shared, never copied.
            let ids = || {
                self.buffers
                    .keys()
                    .copied()
                    .chain(std::iter::once(self.active_buffer_id))
            };
            let mut ids: Vec<_> = ids().collect();
            ids.sort_unstable();
            let targets: Vec<_> = ids
                .iter()
                .copied()
                .filter(|id| *id > self.disk.after)
                .chain(ids.iter().copied().filter(|id| *id <= self.disk.after))
                .filter_map(|id| Target::new(id, self.disk_document(id)?))
                .take(BATCH)
                .collect();
            self.disk.next_scan = Instant::now() + INTERVAL;
            if let Some(last) = targets.last() {
                self.disk.after = last.stamp.id;
                if let Err(error) = self.disk.submit(Job::Scan(targets)) {
                    self.error(&format!("Cannot start disk monitor: {error}"));
                    redraw = true;
                }
            }
        }
        redraw
    }
    fn refresh_matches(&self, stamp: &Stamp, generation: u64) -> bool {
        self.disk
            .view
            .as_ref()
            .is_some_and(|v| &v.stamp == stamp && v.generation == generation)
            && self.active_buffer_id == stamp.id
            && self.note_edit.is_none()
            && stamp.matches(&self.document)
            && self.document.buffer.generation() == generation
    }
    fn stale_refresh(&mut self) {
        if let Some(view) = &mut self.disk.view {
            view.stale = true;
            view.confirming = false;
            view.message = "Buffer changed; cancel and reopen :refresh".into();
        }
    }
    pub(crate) fn open_refresh(&mut self) -> io::Result<()> {
        let target = Target::new(self.active_buffer_id, &self.document)
            .ok_or_else(|| io::Error::other("No file name"))?;
        let view = RefreshView {
            stamp: target.stamp.clone(),
            generation: self.document.buffer.generation(),
            preview: None,
            message: "Comparing buffer with disk…".into(),
            confirming: false,
            stale: false,
            top: 0,
            left: 0,
        };
        self.disk.submit(Job::Compare(
            target,
            view.generation,
            self.document.buffer.text().clone(),
        ))?;
        self.disk.view = Some(view);
        Ok(())
    }
    pub(crate) fn refresh_key(&mut self, key: Key) {
        if matches!(key, Key::Escape | Key::Ctrl('c' | '[') | Key::Char('q')) {
            if let Some(worker) = &self.disk.worker {
                worker.cancel();
            }
            self.disk.busy = false;
            self.disk.action_pending = false;
            self.disk.view = None;
            self.info("Refresh cancelled; buffer retained");
            return;
        }
        if key == Key::Char('c') {
            if let Err(error) = self.open_refresh() {
                self.error(&error.to_string());
            }
            return;
        }
        let Some(view) = &self.disk.view else {
            return;
        };
        if matches!(key, Key::Char('r' | 'y')) && !self.disk.action_pending {
            if !self.refresh_matches(&view.stamp, view.generation) {
                self.stale_refresh();
                return;
            }
            if view.stale
                || view
                    .preview
                    .as_ref()
                    .and_then(|p| p.bytes.as_ref())
                    .is_none()
            {
                return;
            }
            if self.document.buffer.is_modified() && !(view.confirming && key == Key::Char('y')) {
                if key == Key::Char('r') {
                    let view = self.disk.view.as_mut().unwrap();
                    view.confirming = true;
                    view.message = "Discard unsaved buffer edits and reload? y confirms; n keeps edits; Esc cancels".into();
                }
                return;
            }
            if key == Key::Char('y') && !view.confirming {
                return;
            }
            let expected = view
                .preview
                .as_ref()
                .unwrap()
                .bytes
                .as_ref()
                .unwrap()
                .clone();
            let generation = view.generation;
            let target = Target::new(self.active_buffer_id, &self.document).unwrap();
            match self.disk.submit(Job::Reload(target, generation, expected)) {
                Ok(()) => {
                    self.disk.view.as_mut().unwrap().message = "Checking disk before reload…".into()
                }
                Err(error) => self.error(&error.to_string()),
            }
            return;
        }
        if key == Key::Char('n') {
            if let Some(worker) = &self.disk.worker {
                worker.cancel();
            }
            self.disk.busy = false;
            self.disk.action_pending = false;
        }
        let view = self.disk.view.as_mut().unwrap();
        let rows = view
            .preview
            .as_ref()
            .and_then(|p| p.diff.as_ref().ok())
            .map_or(0, |d| d.rows.len());
        match key {
            Key::Char('n') => {
                view.confirming = false;
                view.message = "Buffer retained; c compare, r reload, Esc cancel".into();
            }
            Key::Down | Key::Char('j') => view.top = (view.top + 1).min(rows.saturating_sub(1)),
            Key::Up | Key::Char('k') => view.top = view.top.saturating_sub(1),
            Key::PageDown | Key::Ctrl('d') => {
                view.top = (view.top + 10).min(rows.saturating_sub(1))
            }
            Key::PageUp | Key::Ctrl('u') => view.top = view.top.saturating_sub(10),
            Key::Right | Key::Char('l') => view.left = view.left.saturating_add(4),
            Key::Left | Key::Char('h') => view.left = view.left.saturating_sub(4),
            _ => {}
        }
    }
}

pub(crate) fn render(frame: &mut ratatui::Frame<'_>, editor: &Editor, theme: &crate::theme::Theme) {
    use crate::{diff::Kind, text::visible_text};
    use ratatui::{layout::Rect, style::Style, widgets::Paragraph};
    let Some(view) = &editor.disk.view else {
        return;
    };
    let area = frame.area();
    let base = Style::default().fg(theme.foreground).bg(theme.background);
    let width = usize::from(area.width);
    let row = |y, text: &str, style| {
        (
            Paragraph::new(visible_text(text, 0, width)).style(style),
            Rect::new(area.x, y, area.width, 1),
        )
    };
    let title = format!(
        "Disk refresh | {} | {}",
        if editor.document.buffer.is_modified() {
            "UNSAVED conflict"
        } else {
            "clean buffer"
        },
        view.stamp.path.display()
    );
    let (widget, rect) = row(area.y, &title, base.fg(theme.normal));
    frame.render_widget(widget, rect);
    if area.height < 2 {
        return;
    }
    let (widget, rect) = row(
        area.y + 1,
        "- retained buffer   + disk sample | c compare  r reload  Esc cancel",
        base.fg(theme.muted),
    );
    frame.render_widget(widget, rect);
    if let Some(preview) = &view.preview {
        let lines: Vec<_> = match &preview.diff {
            Ok(diff) if diff.rows.is_empty() => {
                vec![("No textual differences".into(), theme.muted)]
            }
            Ok(diff) => diff
                .rows
                .iter()
                .skip(view.top)
                .take(usize::from(area.height.saturating_sub(3)))
                .map(|r| {
                    let (prefix, color) = match r.kind {
                        Kind::Add => ('+', theme.diff_added),
                        Kind::Delete => ('-', theme.diff_deleted),
                        Kind::Header => (' ', theme.normal),
                        Kind::Context => (' ', theme.foreground),
                    };
                    let text = format!(
                        "{prefix} {}{}{}",
                        r.text.strip_suffix('\r').unwrap_or(&r.text),
                        if r.text.ends_with('\r') {
                            " [CRLF]"
                        } else {
                            ""
                        },
                        if r.no_newline {
                            " [no final newline]"
                        } else {
                            ""
                        }
                    );
                    (visible_text(&text, view.left, width), color)
                })
                .collect(),
            Err(error) => vec![(error.clone(), theme.error)],
        };
        for (offset, (text, color)) in lines
            .iter()
            .take(usize::from(area.height.saturating_sub(3)))
            .enumerate()
        {
            let (widget, rect) = row(area.y + 2 + offset as u16, text, base.fg(*color));
            frame.render_widget(widget, rect);
        }
    }
    if area.height >= 3 {
        let (widget, rect) = row(
            area.bottom() - 1,
            &view.message,
            base.fg(if view.confirming || view.stale {
                theme.error
            } else {
                theme.foreground
            }),
        );
        frame.render_widget(widget, rect);
    }
}
