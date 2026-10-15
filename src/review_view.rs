//! Review commands and a contextual record list. Notes temporarily use the same
//! Document/editing machinery; source documents and histories are retained whole.
use crate::{
    diff::Diff,
    editor::{Editor, Mode, View},
    git::{Comparison, Repository},
    input::Key,
    review::{Attachment, Freshness, Record, State, Store},
    review_store::{self, Change, Job, Outcome, Worker},
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default)]
pub struct ReviewState {
    pub store: Option<Arc<Store>>,
    pub repository: Option<Repository>,
    pub generation: u64,
    pub busy: bool,
    pub error: Option<String>,
    pub status: String,
    pub selected: usize,
    pub filter: String,
    pub return_view: View,
    pub pending_jump: Option<u64>,
    pub focused_record: Option<u64>,
    worker: Option<Worker>,
    failed: Option<Change>,
    note_saving: bool,
}
impl ReviewState {
    fn submit(&mut self, job: Job) -> io::Result<()> {
        if self.busy {
            return Err(io::Error::other("Review pending; wait for completion"));
        }
        if self.worker.is_none() {
            self.worker = Some(Worker::new()?);
        }
        self.worker
            .as_ref()
            .unwrap()
            .submit(job)
            .map_err(io::Error::other)?;
        self.busy = true;
        self.error = None;
        self.status = "Review pending".into();
        Ok(())
    }
    pub fn saving_note(&self) -> bool {
        self.busy && self.note_saving
    }
    pub fn ensure_quit(&self, force: bool) -> io::Result<()> {
        if self.busy {
            return Err(io::Error::other(
                "Review pending; wait for Review saved, then quit",
            ));
        }
        if self.failed.is_some() && !force {
            return Err(io::Error::other(
                "Review write failed; :review retry, or :q! discards the failed action",
            ));
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct FileProgress {
    pub state: Option<State>,
    pub hunks: usize,
    pub attention: usize,
    pub historical_attention: bool,
    pub notes: usize,
    pub stale: usize,
}
impl FileProgress {
    fn include(&mut self, record: &Record, fresh: Freshness) {
        self.historical_attention |= record.state == Some(State::Attention);
        if record.note.is_some() {
            self.notes += 1;
        }
        if fresh != Freshness::Fresh {
            self.stale += 1;
            return;
        }
        if record.attachment == Attachment::File && record.state.is_some() {
            self.state = record.state;
        }
        if record.state == Some(State::Attention) {
            self.attention += 1;
        }
        if matches!(record.attachment, Attachment::Hunk { .. })
            && record.state == Some(State::Reviewed)
        {
            self.hunks += 1;
        }
    }
    pub fn label(&self) -> String {
        format!(
            "{} H{} !{} N{}{}",
            self.state.unwrap_or(State::Unreviewed).label(),
            self.hunks,
            self.attention,
            self.notes,
            if self.stale > 0 {
                format!(" stale/unavailable:{}", self.stale)
            } else {
                String::new()
            }
        )
    }
}
fn error(s: impl std::fmt::Display) -> io::Error {
    io::Error::other(s.to_string())
}
impl Editor {
    pub(crate) fn tick_review(&mut self) -> bool {
        let mut changed = false;
        if let Some(reply) = self.review.worker.as_ref().and_then(Worker::poll) {
            let selected_id = self
                .review_records()
                .get(self.review.selected)
                .map(|r| r.id);
            self.review.busy = false;
            match reply {
                Ok(Outcome::Loaded(store)) => {
                    self.review.generation = self.review.generation.wrapping_add(1);
                    self.review.repository = Some(store.repository.clone());
                    self.review.status = format!("Review loaded: {} records", store.records.len());
                    self.review.store = Some(store);
                    self.review.failed = None;
                }
                Ok(Outcome::Saved(store, id)) => {
                    self.review.generation = self.review.generation.wrapping_add(1);
                    self.review.status = format!("Review saved #{id}");
                    self.review.store = Some(store);
                    self.review.failed = None;
                    if self.review.note_saving {
                        self.finish_note();
                    }
                }
                Ok(Outcome::SnapshotSaved(store, id)) => {
                    self.review.generation = self.review.generation.wrapping_add(1);
                    self.review.repository = Some(store.repository.clone());
                    self.review.store = Some(store);
                    self.review.failed = None;
                    self.review.status =
                        format!("Review snapshot #{id} saved; :review full / :review since");
                }
                Ok(Outcome::Exported(path)) => {
                    self.review.status =
                        format!("Review exported: {}", crate::git::display_path(&path));
                }
                Err(e) => {
                    self.review.error = Some(e.clone());
                    self.review.status = format!("Review error: {e}");
                }
            }
            self.review.note_saving = false;
            let visible = self.review_records();
            self.review.selected = selected_id
                .and_then(|id| visible.iter().position(|r| r.id == id))
                .unwrap_or(self.review.selected)
                .min(visible.len().saturating_sub(1));
            let msg = self.review.status.clone();
            if self.review.error.is_some() {
                self.error(&msg);
            } else {
                self.info(&msg);
            }
            changed = true;
        }
        let repo = self.git.snapshot.as_ref().map(|s| s.repository.clone());
        if !self.review.busy
            && self.review.failed.is_none()
            && !(self.review.error.is_some() && self.review.store.is_some())
            && self.note_edit.is_none()
            && repo != self.review.repository
            && let Some(repo) = repo
        {
            self.review.repository = Some(repo.clone());
            if self.review.store.take().is_some() {
                self.review.generation = self.review.generation.wrapping_add(1);
            }
            self.review.failed = None;
            if let Err(e) = self.review.submit(Job::Load(repo)) {
                self.review.error = Some(e.to_string());
            }
            changed = true;
        }
        if let Some(id) = self.review.pending_jump
            && self.git_current()
        {
            self.review.pending_jump = None;
            let record = self
                .review
                .store
                .as_ref()
                .and_then(|s| s.records.iter().find(|r| r.id == id))
                .cloned();
            if let Some(record) = record {
                let path = self.review_record_path(&record);
                if self.git.diff_view.as_ref().is_some_and(|v| v.path != path) {
                    self.review.pending_jump = Some(id);
                    self.open_diff(path, self.git.comparison.clone());
                    return true;
                }
                let fresh = self.review_freshness(&record);
                if fresh == Freshness::Fresh {
                    if let Some(row) = self
                        .git
                        .diff()
                        .and_then(|d| d.attachment_row(&record.attachment))
                    {
                        let anchor = self
                            .git
                            .diff()
                            .and_then(|d| d.rows.get(row))
                            .map_or(0, |r| r.anchor);
                        if let Some(view) = &mut self.git.diff_view {
                            view.selected = row;
                            view.anchor = anchor;
                        }
                    }
                    self.info(&format!(
                        "Review #{id}: {}",
                        record.note.as_deref().unwrap_or("mark")
                    ));
                } else {
                    self.error(&format!("Review #{id} {}: no automatic attachment; select content and use :note attach ID TARGET or :review revalidate ID TARGET", fresh.label()));
                }
            }
            changed = true;
        }
        changed
    }
    pub fn review_diff(&self) -> Option<&Diff> {
        self.git_current()
            .then(|| self.git.diff())
            .flatten()
            .filter(|d| d.identity.is_some())
    }
    pub fn review_freshness(&self, record: &Record) -> Freshness {
        self.review_freshness_with(record, self.git_current())
    }
    fn review_freshness_with(&self, record: &Record, ready: bool) -> Freshness {
        if !ready {
            return Freshness::Unavailable;
        }
        self.git
            .snapshot
            .as_ref()
            .and_then(|s| s.review_freshness.get(&record.id))
            .map(|(revision, freshness)| {
                if *revision == record.revision {
                    *freshness
                } else {
                    Freshness::Unavailable
                }
            })
            .unwrap_or_else(|| {
                let current = self.git.diff().filter(|d| record.matches_diff(d));
                record.freshness(current)
            })
    }
    fn review_ready(&self) -> io::Result<Arc<Store>> {
        if self.review.busy {
            return Err(error("Review pending; wait for completion"));
        }
        if let Some(e) = &self.review.error {
            return Err(error(format!(
                "Review unavailable: {e}; :review reload retries loading"
            )));
        }
        self.review
            .store
            .clone()
            .filter(|s| Some(&s.repository) == self.git.snapshot.as_ref().map(|s| &s.repository))
            .ok_or_else(|| error("Review store loading or unavailable"))
    }
    fn review_target(&self, target: &str) -> io::Result<Record> {
        if !matches!(self.view, View::Editing | View::Diff | View::Changes) {
            return Err(error(
                "Inspect a comparison before annotating; Enter opens the selected review record",
            ));
        }
        if target != "file" && self.view != View::Diff {
            return Err(error(
                "Open :diff and select a hunk/line before attaching there",
            ));
        }

        let diff = self
            .review_diff()
            .ok_or_else(|| error("Wait for a fresh supported Git comparison; use :diff"))?;
        let identity = diff.identity.clone().unwrap();
        let mut row = self.git.diff_view.as_ref().map_or_else(
            || diff.nearest(self.document.buffer.position().0),
            |v| v.selected,
        );
        if self
            .git
            .diff_view
            .as_ref()
            .is_some_and(|v| v.effective_paired)
            && matches!(target, "old" | "new")
        {
            let pair = &diff.pairs[diff.pair_index(row)];
            row = if target == "old" {
                pair.left
            } else {
                pair.right
            }
            .ok_or_else(|| error("No line on selected side"))?;
        }
        if self.view == View::Changes {
            let files = self.changed_files();
            if files
                .get(self.git.selected)
                .is_none_or(|f| identity.repository.root.join(&identity.new_path) != f.path)
            {
                return Err(error(
                    "Open the selected file with d before annotating its comparison",
                ));
            }
        }
        let (attachment, context) = diff.attachment(row, target).map_err(error)?;
        let unsaved = self.git.comparison != Comparison::Staged && self.git_compared_unsaved();
        Ok(Record {
            id: 0,
            revision: 0,
            identity,
            attachment,
            context,
            state: Some(State::Unreviewed),
            note: None,
            unsaved,
        })
    }
    fn save_review_change(&mut self, change: Change) -> io::Result<()> {
        let store = self.review_ready()?;
        self.review.submit(Job::Change(
            store.repository.clone(),
            Box::new(change.clone()),
        ))?;
        self.review.failed = Some(change);
        self.info("Review pending");
        Ok(())
    }
    fn review_record(&self, id: &str) -> io::Result<Record> {
        let id: u64 = id
            .parse()
            .map_err(|_| error("Use a record ID from :review list or :notes"))?;
        self.review_ready()?
            .records
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| error("Review record not found"))
    }
    pub fn review_command(&mut self, argument: &str) -> io::Result<()> {
        let words: Vec<_> = argument.split_whitespace().collect();
        match words.as_slice() {
            ["snapshot"] => {
                self.review_ready()?;
                let request = self.git_request().ok_or_else(|| error("Name a source file before capturing"))?;
                self.review.submit(Job::Snapshot(Box::new(request)))?;
                self.info("Review snapshot pending: capturing disk and unsaved buffers; source files are not saved");
            }
            ["full"] => self.open_review_comparison(None, false)?,
            ["since"] => self.open_review_comparison(None, true)?,
            ["since", id] => self.open_review_comparison(Some(id), true)?,
            ["snapshots"] => {
                let store = self.review_ready()?;
                let history = store.history.as_ref().ok_or_else(|| error("No snapshots; :review snapshot creates the first"))?;
                self.info(&format!("Review snapshots #1–#{}; latest #{} ({} files). :review since ID; full base {} stays fixed", history.latest(), history.latest(), history.checkpoints.last().unwrap().files.len(), history.baseline.comparison.label()));
            }
            [] | ["help"] => self.info("Review: :review snapshot; :review full|since [ID]; :review snapshots; :review file|hunk reviewed|attention|unreviewed|clear; :note file|hunk|old|new; :notes; :review list [FILTER]; :review revalidate ID TARGET; :review clear ID; :review next file|hunk [FILTER]; :review export /outside/worktree/new.md"),
            ["file" | "hunk", state] => {
                let mut record = self.review_target(words[0])?;
                let store = self.review_ready()?;
                let expected = store.records.iter().find(|r| r.state.is_some() && r.attachment == record.attachment && self.review_diff().is_some_and(|d| r.matches_diff(d)) && self.review_freshness(r) == Freshness::Fresh).cloned();
                if *state == "clear" {
                    let expected = expected.ok_or_else(|| error("No mark on this exact content; :review clear ID removes a stale mark"))?;
                    return self.save_review_change(Change { expected: Some(expected), replacement: None });
                }
                record.state = Some(State::parse(state).ok_or_else(|| error("State: reviewed, attention, unreviewed or clear"))?);
                self.save_review_change(Change { expected, replacement: Some(record) })?;
            }
            ["clear", id] => { let old = self.review_record(id)?; self.save_review_change(Change { expected: Some(old), replacement: None })?; }
            ["revalidate", id, target @ ("file" | "hunk")] => {
                let old = self.review_record(id)?;
                if old.state.is_none() { return Err(error("Use :note attach ID TARGET to correct a note attachment")); }
                let mut new = self.review_target(target)?; new.state = old.state;
                self.save_review_change(Change { expected: Some(old), replacement: Some(new) })?;
            }
            ["reload"] => {
                let repo = self.git.snapshot.as_ref().map(|s| s.repository.clone()).ok_or_else(|| error("No repository"))?;
                self.review.submit(Job::Load(repo))?;
            }
            ["retry"] => {
                let change = self.review.failed.clone().ok_or_else(|| error("No failed review action; use :review reload for a load error"))?;
                let repo = change.expected.as_ref().or(change.replacement.as_ref()).unwrap().identity.repository.clone();
                self.review.submit(Job::Change(repo, Box::new(change)))?;
            }
            ["list"] => self.open_review_list("all")?,
            ["list", filter] => self.open_review_list(filter)?,
            ["next" | "prev", target @ ("file" | "hunk")] => self.review_navigate(target, "unreviewed", words[0] == "prev")?,
            ["next" | "prev", target @ ("file" | "hunk"), filter] => self.review_navigate(target, filter, words[0] == "prev")?,
            _ if argument.starts_with("export ") => self.export_review(argument[7..].trim())?,
            _ => return Err(error("Use :review help for commands")),
        }
        Ok(())
    }
    fn open_review_comparison(&mut self, id: Option<&str>, since: bool) -> io::Result<()> {
        let store = self.review_ready()?;
        let history = store
            .history
            .as_ref()
            .ok_or_else(|| error("No review snapshot; use :review snapshot first"))?;
        let comparison = if since {
            let id = id
                .map(|s| {
                    s.parse::<u64>()
                        .map_err(|_| error("Use a checkpoint ID from :review snapshots"))
                })
                .transpose()?
                .unwrap_or_else(|| history.latest());
            let comparison = Comparison::ReviewSince(id);
            history.files(&comparison).map_err(error)?;
            comparison
        } else {
            Comparison::ReviewFull
        };
        let path = self
            .git
            .diff_view
            .as_ref()
            .map(|v| v.path.clone())
            .or_else(|| self.document.path().map(PathBuf::from))
            .ok_or_else(|| error("Name a file before comparing"))?;
        self.open_diff(path, comparison);
        Ok(())
    }
    pub fn note_command(&mut self, argument: &str) -> io::Result<()> {
        if self.note_edit.is_some() {
            return match argument {
                "save" | "save-new" => {
                    let store = self
                        .review
                        .store
                        .clone()
                        .ok_or_else(|| error("No loaded review store"))?;
                    let text = self.document.buffer.text().to_string();
                    if text.len() > crate::review::MAX_NOTE {
                        return Err(error("Notes are limited to 16 KiB"));
                    }
                    let edit = self.note_edit.as_ref().unwrap();
                    let mut record = edit.record.clone();
                    record.note = Some(text);
                    let expected = if argument == "save-new" {
                        None
                    } else {
                        edit.expected.clone()
                    };
                    let change = Change {
                        expected,
                        replacement: Some(record),
                    };
                    self.review.submit(Job::Change(
                        store.repository.clone(),
                        Box::new(change.clone()),
                    ))?;
                    self.review.failed = Some(change);
                    self.review.note_saving = true;
                    self.info("Review pending: saving note");
                    Ok(())
                }
                "cancel" => {
                    if self.review.saving_note() {
                        return Err(error("Note save pending"));
                    }
                    self.finish_note();
                    self.review.failed = None;
                    self.info("Note draft cancelled");
                    Ok(())
                }
                _ => Err(error(
                    "Note draft: :note save, :note save-new, or :note cancel",
                )),
            };
        }
        let words: Vec<_> = argument.split_whitespace().collect();
        match words.as_slice() {
            [target @ ("file" | "hunk" | "old" | "new")] => {
                self.review_ready()?;
                let mut record = self.review_target(target)?;
                record.state = None;
                record.note = Some(String::new());
                self.start_note(record, None);
            }
            ["edit", id] => {
                let record = self.review_record(id)?;
                if record.note.is_none() {
                    return Err(error("Record is a mark; use :notes for note IDs"));
                }
                self.start_note(record.clone(), Some(record));
            }
            ["remove", id] => {
                let record = self.review_record(id)?;
                if record.note.is_none() {
                    return Err(error("Record is not a note"));
                }
                self.save_review_change(Change {
                    expected: Some(record),
                    replacement: None,
                })?;
            }
            ["attach", id, target] => {
                let old = self.review_record(id)?;
                if old.note.is_none() {
                    return Err(error("Record is not a note"));
                }
                let mut new = self.review_target(target)?;
                new.state = None;
                new.note = old.note.clone();
                self.save_review_change(Change {
                    expected: Some(old),
                    replacement: Some(new),
                })?;
            }
            ["next" | "prev"] => {
                let notes: Vec<_> = self
                    .review_ready()?
                    .records
                    .iter()
                    .filter(|r| r.note.is_some())
                    .map(|r| r.id)
                    .collect();
                if notes.is_empty() {
                    return Err(error("No notes"));
                }
                let pos = self
                    .review
                    .focused_record
                    .or_else(|| {
                        self.review_records()
                            .get(self.review.selected)
                            .map(|r| r.id)
                    })
                    .and_then(|id| notes.iter().position(|n| *n == id));
                let n = match pos {
                    Some(i) if words[0] == "prev" => (i + notes.len() - 1) % notes.len(),
                    Some(i) => (i + 1) % notes.len(),
                    None => 0,
                };
                self.navigate_review_record(notes[n])?;
            }
            _ => {
                return Err(error(
                    "Notes: :note file|hunk|old|new; :note edit|remove ID; :note attach ID TARGET; :notes; :note next|prev",
                ));
            }
        }
        Ok(())
    }
    pub fn export_review(&mut self, destination: &str) -> io::Result<()> {
        if destination.is_empty() {
            return Err(error("Use :review export /outside/worktree/new.md"));
        }
        let store = self.review_ready()?;
        let freshness = store
            .records
            .iter()
            .map(|r| (r.id, self.review_freshness(r)))
            .collect();
        let protected = std::iter::once(&self.document)
            .chain(self.buffers.values().map(|b| &b.document))
            .filter_map(|d| d.path().map(PathBuf::from))
            .collect();
        self.review.submit(Job::Export(Box::new(review_store::Export { store, freshness, destination: destination.into(), protected,
            scope: "The current file and up to 31 other annotated paths for the selected comparison are validated per refresh; other bases, failures and excess paths are unavailable".into() })))?;
        self.info("Review export pending");
        Ok(())
    }
    pub fn open_review_list(&mut self, filter: &str) -> io::Result<()> {
        if !["all", "stale", "attention", "unreviewed", "notes"].contains(&filter) {
            return Err(error("Filters: all, stale, attention, unreviewed, notes"));
        }
        if self.view != View::Review {
            self.review.return_view = std::mem::replace(&mut self.view, View::Review);
        }
        self.review.filter = filter.into();
        self.review.selected = 0;
        self.mode = Mode::Normal;
        self.info(
            "Review records: j/k select, Enter inspect, e edit note, Esc return; :review help",
        );
        Ok(())
    }
    fn matches_review_filter(&self, record: &Record, filter: &str) -> bool {
        match filter {
            "notes" => record.note.is_some(),
            "stale" => self.review_freshness(record) != Freshness::Fresh,
            "attention" => record.state == Some(State::Attention),
            "unreviewed" => {
                record.state == Some(State::Unreviewed)
                    || self.review_freshness(record) != Freshness::Fresh
            }
            _ => true,
        }
    }
    pub fn review_records(&self) -> Vec<&Record> {
        self.review.store.as_ref().map_or_else(Vec::new, |s| {
            s.records
                .iter()
                .filter(|r| self.matches_review_filter(r, &self.review.filter))
                .collect()
        })
    }
    fn review_record_in_comparison(&self, record: &Record) -> bool {
        record.identity.comparison == self.git.comparison
            || (self.git.comparison == Comparison::ReviewFull
                && self
                    .review
                    .store
                    .as_ref()
                    .and_then(|s| s.history.as_ref())
                    .is_some_and(|h| h.baseline.comparison == record.identity.comparison))
    }
    fn review_record_path(&self, record: &Record) -> PathBuf {
        let path = self
            .git
            .snapshot
            .as_ref()
            .filter(|s| s.repository == record.identity.repository)
            .and_then(|s| {
                s.review_renames
                    .iter()
                    .find(|(_, from)| **from == record.identity.new_path)
            })
            .map_or(&record.identity.new_path, |(to, _)| to);
        record.identity.repository.root.join(path)
    }
    pub fn review_file_progress(&self, path: &Path) -> FileProgress {
        let mut progress = FileProgress::default();
        let ready = self.git_current();
        if let Some(store) = &self.review.store {
            for r in store.records.iter().filter(|r| {
                self.review_record_path(r) == path && self.review_record_in_comparison(r)
            }) {
                progress.include(r, self.review_freshness_with(r, ready));
            }
        }
        progress
    }
    pub fn review_progress(&self) -> std::collections::BTreeMap<PathBuf, FileProgress> {
        let mut progress = std::collections::BTreeMap::<PathBuf, FileProgress>::new();
        let ready = self.git_current();
        if let Some(store) = &self.review.store {
            for r in store
                .records
                .iter()
                .filter(|r| self.review_record_in_comparison(r))
            {
                progress
                    .entry(self.review_record_path(r))
                    .or_default()
                    .include(r, self.review_freshness_with(r, ready));
            }
        }
        progress
    }
    pub fn review_file_label(&self, path: &Path) -> String {
        if self.review.store.is_none() {
            "review unavailable".into()
        } else {
            self.review_file_progress(path).label()
        }
    }
    pub fn review_summary(&self) -> String {
        let Some(store) = &self.review.store else {
            return self.review.status.clone();
        };
        let ready = self.git_current();
        let mut files = 0;
        let mut hunks = 0;
        let mut attention = 0;
        let mut stale = 0;
        for r in &store.records {
            if self.review_freshness_with(r, ready) != Freshness::Fresh {
                stale += 1;
                continue;
            }
            match (&r.attachment, r.state) {
                (_, Some(State::Attention)) => attention += 1,
                (Attachment::File, Some(State::Reviewed)) => files += 1,
                (Attachment::Hunk { .. }, Some(State::Reviewed)) => hunks += 1,
                _ => {}
            }
        }
        format!(
            "Review F{files} H{hunks} attention:{attention} stale/unavailable:{stale}{} | {}",
            if self.git.snapshot.as_ref().is_some_and(|s| s.review_limited) {
                " [validation limited]"
            } else {
                ""
            },
            self.review.status
        )
    }
    fn navigate_review_record(&mut self, id: u64) -> io::Result<()> {
        let record = self.review_record(&id.to_string())?;
        self.review.pending_jump = Some(id);
        self.review.focused_record = Some(id);
        let comparison =
            if self.git.comparison.is_review() && self.review_record_in_comparison(&record) {
                self.git.comparison.clone()
            } else {
                record.identity.comparison.clone()
            };
        let path = self.review_record_path(&record);
        self.open_diff(path, comparison);
        Ok(())
    }
    pub(crate) fn review_key(&mut self, key: Key) {
        let result = match key {
            Key::Char(':') => {
                self.mode = Mode::Command;
                self.command.clear();
                Ok(())
            }
            Key::Escape | Key::Ctrl('[' | 'c') | Key::Char('q') => {
                self.view = std::mem::take(&mut self.review.return_view);
                Ok(())
            }
            Key::Char('j') | Key::Down => {
                self.review.selected =
                    (self.review.selected + 1).min(self.review_records().len().saturating_sub(1));
                Ok(())
            }
            Key::Char('k') | Key::Up => {
                self.review.selected = self.review.selected.saturating_sub(1);
                Ok(())
            }
            Key::Enter | Key::Char('e') => {
                if let Some(id) = self
                    .review_records()
                    .get(self.review.selected)
                    .map(|r| r.id)
                {
                    if key == Key::Enter {
                        self.navigate_review_record(id)
                    } else {
                        self.note_command(&format!("edit {id}"))
                    }
                } else {
                    Err(error("No matching review records"))
                }
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.error(&e.to_string());
        }
    }
    fn review_navigate(&mut self, target: &str, filter: &str, backward: bool) -> io::Result<()> {
        if !["unreviewed", "attention", "stale", "all"].contains(&filter) {
            return Err(error(
                "Navigation filters: unreviewed, attention, stale, all",
            ));
        }
        if target == "file" {
            if !self.git_current() {
                return Err(error("Wait for fresh Git status"));
            }
            let files = self.changed_files();
            let progress = self.review_progress();
            let empty = FileProgress::default();
            let current = self
                .git
                .diff_view
                .as_ref()
                .map(|v| v.path.as_path())
                .or(self.document.path());
            let start = current
                .and_then(|p| files.iter().position(|f| f.path == p))
                .unwrap_or(files.len().saturating_sub(1));
            for step in 1..=files.len() {
                let n = if backward {
                    (start + files.len() - step) % files.len()
                } else {
                    (start + step) % files.len()
                };
                let file = &files[n];
                let progress = progress.get(&file.path).unwrap_or(&empty);
                let matches = match filter {
                    "attention" => progress.historical_attention,
                    "stale" => progress.stale > 0,
                    "unreviewed" => {
                        progress.state != Some(State::Reviewed) || progress.attention > 0
                    }
                    _ => true,
                };
                if matches {
                    self.open_diff(file.path.clone(), self.git.comparison.clone());
                    return Ok(());
                }
            }
        } else {
            let diff = self
                .review_diff()
                .ok_or_else(|| error("Wait for a fresh comparison"))?;
            let current = self
                .git
                .diff_view
                .as_ref()
                .ok_or_else(|| error("Open :diff first"))?
                .selected;
            let mut states = std::collections::BTreeMap::<&str, (Option<State>, bool, bool)>::new();
            if let Some(store) = &self.review.store {
                for r in store.records.iter().filter(|r| r.matches_diff(diff)) {
                    if let Attachment::Hunk { fingerprint } = &r.attachment {
                        let value = states.entry(fingerprint.as_str()).or_default();
                        let fresh = self.review_freshness_with(r, true);
                        if fresh == Freshness::Fresh && r.state.is_some() {
                            value.0 = r.state;
                        }
                        value.1 |= r.state == Some(State::Attention);
                        value.2 |= fresh != Freshness::Fresh;
                    }
                }
            }
            let mut rows = diff.hunks.clone();
            if backward {
                rows.reverse();
            }
            let (later, earlier): (Vec<_>, Vec<_>) = rows
                .into_iter()
                .partition(|i| if backward { *i < current } else { *i > current });
            for row in later.into_iter().chain(earlier) {
                let state = diff
                    .locations
                    .hunks
                    .get(&row)
                    .and_then(|key| states.get(key.as_str()))
                    .copied()
                    .unwrap_or_default();
                let matches = match filter {
                    "attention" => state.1,
                    "stale" => state.2,
                    "unreviewed" => state.0 != Some(State::Reviewed),
                    _ => true,
                };
                if matches {
                    let anchor = diff.rows[row].anchor;
                    let view = self.git.diff_view.as_mut().unwrap();
                    view.selected = row;
                    view.anchor = anchor;
                    return Ok(());
                }
            }
        }
        Err(error(format!(
            "No {filter} {target} targets; :review list stale includes unresolved historical records"
        )))
    }
}

pub fn render(frame: &mut ratatui::Frame<'_>, editor: &Editor, theme: &crate::theme::Theme) {
    use ratatui::{layout::Rect, style::Style, widgets::Paragraph};
    let area = frame.area();
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let mut line = |row: u16, text: &str, s: Style| {
        if row < area.height {
            frame.render_widget(
                Paragraph::new(crate::text::visible_text(text, 0, area.width as usize)).style(s),
                Rect::new(area.x, area.y + row, area.width, 1),
            );
        }
    };
    line(
        0,
        &format!("REVIEW records — filter {}", editor.review.filter),
        style.fg(theme.normal),
    );
    line(
        1,
        &editor.review_summary(),
        style.fg(if editor.review.error.is_some() {
            theme.error
        } else {
            theme.muted
        }),
    );
    let records = editor.review_records();
    let height = area.height.saturating_sub(4) as usize;
    let top = editor
        .review
        .selected
        .saturating_sub(height.saturating_sub(1));
    for (i, r) in records.iter().enumerate().skip(top).take(height) {
        let freshness = editor.review_freshness(r);
        let kind = match r.attachment {
            Attachment::File => "file",
            Attachment::Hunk { .. } => "hunk",
            Attachment::Line {
                side: crate::review::Side::Old,
                ..
            } => "old line",
            Attachment::Line { .. } => "new line",
        };
        let text = format!(
            "#{} {} {} {} {} [{}] old:{} new:{} {}",
            r.id,
            freshness.label(),
            r.state.map_or("note", State::label),
            kind,
            crate::git::display_path(&r.identity.new_path),
            r.identity.comparison.description(),
            r.context
                .old_line
                .map_or("-".into(), |n| (n + 1).to_string()),
            r.context
                .new_line
                .map_or("-".into(), |n| (n + 1).to_string()),
            r.note.as_deref().unwrap_or("")
        );
        let color = if freshness != Freshness::Fresh {
            theme.modified
        } else {
            match r.state {
                Some(State::Reviewed) => theme.diff_added,
                Some(State::Attention) => theme.error,
                _ => theme.foreground,
            }
        };
        line(
            (i - top + 2) as u16,
            &text,
            style.fg(color).bg(if i == editor.review.selected {
                theme.selection
            } else {
                theme.background
            }),
        );
    }
    if records.is_empty() {
        line(
            2,
            "No matching review records. :review help lists actions.",
            style.fg(theme.muted),
        );
    }
    if area.height > 1 {
        line(
            area.height - 2,
            "j/k select  Enter inspect  e edit note  Esc return  :review help",
            style.bg(theme.status),
        );
    }
    let command = format!(":{}", editor.command);
    let command_cells = crate::text::display_column(&command, command.chars().count());
    let offset = command_cells.saturating_sub(area.width.saturating_sub(1) as usize);
    let command_display = crate::text::visible_text(&command, offset, area.width as usize);
    let text = if editor.mode == Mode::Command {
        command_display.as_str()
    } else {
        &editor.message
    };
    line(
        area.height.saturating_sub(1),
        text,
        style.fg(if editor.message_is_error {
            theme.error
        } else {
            theme.foreground
        }),
    );
    if editor.mode == Mode::Command && area.width > 0 && area.height > 0 {
        frame.set_cursor_position((
            area.x + (command_cells - offset).min(area.width as usize - 1) as u16,
            area.bottom() - 1,
        ));
    }
}
