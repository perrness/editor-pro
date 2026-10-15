//! AI is a contextual view over captured text, never a second editing engine.
use crate::{
    ai::{self, Answer, Context, Conversation, Draft, Source, Status, Turn, error},
    ai_worker::{Job, Outcome, Worker},
    editor::{Editor, Mode, View},
    file::{Document, absolute_path},
    input::Key,
    lsp::{self, Stamp},
    lsp_edit::{EditPlan, PlannedFile},
    review::fingerprint,
};
use ropey::Rope;
use std::{
    collections::BTreeMap,
    io,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default, PartialEq, Eq)]
enum Panel {
    #[default]
    Conversation,
    Preview,
    Edits,
}
#[derive(Clone)]
struct Link {
    context: Arc<Context>,
    line: usize,
}
pub struct Row {
    pub text: String,
    link: Option<Link>,
}
#[derive(Default)]
pub struct AiState {
    pub config: ai::Config,
    pub conversation: Option<Conversation>,
    pub draft: Draft,
    pub pending: bool,
    pub status: String,
    pub dirty: bool,
    pub rows: Vec<Row>,
    pub selected: usize,
    top: usize,
    left: usize,
    panel: Panel,
    return_view: View,
    worker: Option<Worker>,
    serial: u64,
    inference: bool,
    saved: Option<(PathBuf, String)>,
    partial: Option<Answer>,
    plan: Option<EditPlan>,
}
impl AiState {
    fn idle(&self) -> io::Result<()> {
        if self.pending {
            Err(error(
                "AI work is pending; cancel the request or wait for conversation I/O",
            ))
        } else {
            Ok(())
        }
    }
    fn submit(&mut self, job: Job) -> io::Result<()> {
        self.idle()?;
        if self.worker.is_none() {
            self.worker = Some(Worker::new()?);
        }
        self.inference = matches!(job, Job::Ask { .. });
        self.serial = self.worker.as_ref().unwrap().submit(job);
        self.pending = true;
        self.partial = None;
        Ok(())
    }
    pub(crate) fn ensure_quit(&self, force: bool) -> io::Result<()> {
        if !force && (self.pending || self.dirty) {
            Err(error(
                "AI conversation pending or unsaved: :ai save PATH, :ai cancel, or :q!",
            ))
        } else {
            Ok(())
        }
    }
    fn cancel(&mut self) -> io::Result<()> {
        if !self.pending {
            return Ok(());
        }
        if !self.inference {
            return Err(error("Wait for conversation file I/O to finish"));
        }
        if let Some(worker) = &self.worker {
            worker.cancel();
        }
        self.pending = false;
        self.partial = None;
        self.status = "Codex request cancelled".into();
        if let Some(turn) = self.conversation.as_mut().and_then(|c| c.turns.last_mut()) {
            turn.status = Status::Cancelled;
            turn.detail = self.status.clone();
        }
        self.rebuild();
        Ok(())
    }
    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        match self.panel {
            Panel::Preview => {
                plain(
                    &mut rows,
                    "Context preview — :ai send transmits the question, context and prior turns",
                );
                plain(
                    &mut rows,
                    &format!(
                        "Codex: {} · model {} · read-only",
                        crate::git::display_path(&self.config.executable),
                        self.config.model.as_deref().unwrap_or("CLI default")
                    ),
                );
                plain(
                    &mut rows,
                    "Captured text is local evidence. Model observations and inferred intent remain separate.",
                );
                if let Some(c) = &self.conversation {
                    plain(
                        &mut rows,
                        &format!(
                            "{} prior turns included; :ai shows their full context and answers",
                            c.turns.len()
                        ),
                    );
                }
                draft_rows(&mut rows, &self.draft);
            }
            Panel::Conversation => {
                plain(
                    &mut rows,
                    "AI conversation · model claims require verification against source",
                );
                if let Some(conversation) = &self.conversation {
                    for (i, turn) in conversation.turns.iter().enumerate() {
                        plain(&mut rows, &format!("Turn {} · {:?}", i + 1, turn.status));
                        draft_rows(&mut rows, &turn.draft);
                        if let Some(answer) = &turn.answer {
                            answer_rows(&mut rows, answer, &turn.draft);
                        }
                        if !turn.detail.is_empty() {
                            plain(&mut rows, &turn.detail);
                        }
                    }
                    if let (Some(answer), Some(turn)) = (&self.partial, conversation.turns.last()) {
                        plain(
                            &mut rows,
                            "Receiving answer — incomplete, no edits available yet",
                        );
                        answer_rows(&mut rows, answer, &turn.draft);
                    }
                }
                if rows.len() == 1 {
                    plain(
                        &mut rows,
                        "Use :ai context file, :ai ask QUESTION, then :ai send",
                    );
                }
            }
            Panel::Edits => {
                plain(
                    &mut rows,
                    "Suggested replacements — :ai apply approves; Escape returns without applying",
                );
                if let Some(plan) = &self.plan {
                    for file in &plan.files {
                        plain(&mut rows, &crate::git::display_path(&file.path));
                        for (range, replacement) in &file.edits {
                            plain(
                                &mut rows,
                                &format!(
                                    "Original at line {}:",
                                    file.before.char_to_line(range.start) + 1
                                ),
                            );
                            for line in file.before.slice(range.clone()).to_string().split('\n') {
                                plain(&mut rows, &format!("- {line}"));
                            }
                            plain(&mut rows, "Replacement:");
                            for line in replacement.split('\n') {
                                plain(&mut rows, &format!("+ {line}"));
                            }
                        }
                    }
                }
            }
        }
        self.rows = rows;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }
}
fn plain(rows: &mut Vec<Row>, text: &str) {
    rows.extend(text.split('\n').map(|line| Row {
        text: line.into(),
        link: None,
    }));
}
fn draft_rows(rows: &mut Vec<Row>, draft: &Draft) {
    plain(rows, &format!("Question: {}", draft.question));
    for (i, context) in draft.context.iter().enumerate() {
        let context = Arc::new(context.clone());
        plain(rows, &format!("Context {} · {}", i + 1, context.label));
        for (line, text) in context.text.split('\n').enumerate() {
            rows.push(Row {
                text: format!("{:>4} {text}", line + 1),
                link: context.source.as_ref().map(|_| Link {
                    context: context.clone(),
                    line: line + 1,
                }),
            });
        }
    }
}
fn answer_rows(rows: &mut Vec<Row>, answer: &Answer, draft: &Draft) {
    plain(rows, &format!("Codex: {}", answer.summary));
    plain(rows, "Model observations (check cited evidence):");
    let contexts: Vec<_> = draft.context.iter().cloned().map(Arc::new).collect();
    for observation in &answer.observations {
        plain(rows, &observation.text);
        for citation in &observation.citations {
            if let Some(context) = citation
                .context
                .checked_sub(1)
                .and_then(|i| contexts.get(i))
            {
                rows.push(Row {
                    text: format!(
                        "  → context {} line {} · {}",
                        citation.context, citation.line, context.label
                    ),
                    link: context.source.as_ref().map(|_| Link {
                        context: context.clone(),
                        line: citation.line,
                    }),
                });
            }
        }
    }
    plain(rows, "Inferred intent / uncertainty:");
    for inference in &answer.inferences {
        plain(rows, inference);
    }
    if !answer.edits.is_empty() {
        plain(
            rows,
            &format!(
                "{} suggested replacements; :ai edits previews the latest completed turn",
                answer.edits.len()
            ),
        );
    }
}

impl Editor {
    fn ai_root(&mut self) -> io::Result<PathBuf> {
        if let Some(c) = &self.ai.conversation {
            return Ok(c.root.clone());
        }
        let parent = self
            .document
            .path()
            .and_then(Path::parent)
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?);
        let root = parent
            .ancestors()
            .find(|p| p.join(".git").exists() || p.join("Cargo.toml").is_file())
            .unwrap_or(&parent)
            .canonicalize()?;
        self.ai.conversation = Some(Conversation::new(root.clone()));
        Ok(root)
    }
    fn ai_show(&mut self, panel: Panel) {
        if self.view != View::Ai {
            self.ai.return_view = std::mem::replace(&mut self.view, View::Ai);
            if self.ai.return_view == View::Lsp {
                // Capturing a language result leaves its request context. Return
                // to the retained source, rather than an invalidated empty list.
                self.ai.return_view = View::Editing;
                self.lsp.cancel();
            }
        }
        self.document.buffer.commit();
        self.mode = Mode::Normal;
        self.ai.panel = panel;
        self.ai.selected = 0;
        self.ai.top = 0;
        self.ai.left = 0;
        self.ai.rebuild();
    }
    fn ai_open_document(&self, path: &Path) -> Option<(usize, &Document)> {
        if self.document.path() == Some(path) {
            Some((self.active_buffer_id, &self.document))
        } else {
            self.buffers
                .iter()
                .find(|(_, s)| s.document.path() == Some(path))
                .map(|(id, s)| (*id, &s.document))
        }
    }
    fn ai_capture(
        &self,
        root: &Path,
        path: &Path,
        range: Option<Range<usize>>,
        label: &str,
    ) -> io::Result<Context> {
        let path = absolute_path(path)?;
        if !path.starts_with(root) {
            return Err(error(
                "AI context is outside this conversation's project; start :ai new first",
            ));
        }
        let loaded;
        let open = self.ai_open_document(&path);
        let document = if let Some((_, doc)) = open {
            doc
        } else {
            loaded = open_small(&path)?;
            &loaded
        };
        let text = document.buffer.text();
        if text.len_bytes() > lsp::MAX_DOCUMENT_BYTES {
            return Err(error("AI source exceeds 2 MiB; use a smaller file"));
        }
        let range = range.unwrap_or(0..text.len_chars());
        if range.start > range.end || range.end > text.len_chars() {
            return Err(error("AI context range is outside the source"));
        }
        let slice = text.slice(range.clone());
        if slice.len_bytes() > ai::MAX_ATTACHMENT {
            return Err(error(
                "AI attachment exceeds 64 KiB; select a smaller range",
            ));
        }
        let first_line = text.char_to_line(range.start) + 1;
        let unsaved = document.buffer.is_modified();
        Ok(Context {
            label: format!(
                "{label} · {}:{first_line} · {}",
                crate::git::display_path(&path),
                if unsaved {
                    "unsaved buffer"
                } else if open.is_some() {
                    "loaded buffer"
                } else {
                    "disk"
                }
            ),
            text: slice.to_string(),
            source: Some(Source {
                path,
                start: range.start,
                first_line,
                fingerprint: fingerprint(text.to_string().as_bytes()),
                unsaved,
                live: true,
                stamp: open.map(|(id, doc)| Stamp {
                    buffer_id: id,
                    instance: doc.buffer.instance(),
                    generation: doc.buffer.generation(),
                }),
            }),
        })
    }
    /// Check captured text before sending, linking, and preparing a replacement.
    /// Links may use persisted evidence; edits additionally require live stamps.
    fn ai_source(&self, context: &Context, live: bool) -> io::Result<(Rope, Option<Document>)> {
        let s = context
            .source
            .as_ref()
            .ok_or_else(|| error("This context has no source destination"))?;
        if live && !s.live {
            return Err(error(
                "Restored suggestions are historical; capture fresh context and ask again",
            ));
        }
        let root = &self
            .ai
            .conversation
            .as_ref()
            .ok_or_else(|| error("No AI conversation"))?
            .root;
        if !s.path.starts_with(root) || absolute_path(&s.path)? != s.path {
            return Err(error("AI source identity changed"));
        }
        let open = self.ai_open_document(&s.path);
        let stamp = open.map(|(id, doc)| Stamp {
            buffer_id: id,
            instance: doc.buffer.instance(),
            generation: doc.buffer.generation(),
        });
        if live && stamp != s.stamp {
            return Err(error(
                "AI source buffer changed, reloaded, opened or closed; recapture context",
            ));
        }
        let loaded = if open.is_none() {
            Some(open_small(&s.path)?)
        } else {
            None
        };
        let doc = open.map(|(_, d)| d).or(loaded.as_ref()).unwrap();
        let text = doc.buffer.text();
        if text.len_bytes() > lsp::MAX_DOCUMENT_BYTES
            || fingerprint(text.to_string().as_bytes()) != s.fingerprint
        {
            return Err(error(
                "AI source content changed; recapture context before sending or navigating",
            ));
        }
        let end = s
            .start
            .checked_add(context.text.chars().count())
            .ok_or_else(|| error("Invalid AI range"))?;
        if end > text.len_chars() || text.slice(s.start..end) != context.text.as_str() {
            return Err(error("Captured AI range no longer matches source"));
        }
        Ok((text.clone(), loaded))
    }

    pub fn ai_command(&mut self, argument: &str) -> io::Result<()> {
        if self.note_edit.is_some() {
            return Err(error("Finish the review note before using AI"));
        }
        let (name, arg) = argument
            .split_once(char::is_whitespace)
            .map_or((argument, ""), |(n, a)| (n, a.trim()));
        match name {
            "" if arg.is_empty() => { self.ai_show(Panel::Conversation); },
            "help" => self.info("AI: :ai context file|selection|symbol|hunk|diff; :ai add file|tests PATH or diagnostics|references|feedback; :ai ask QUESTION; :ai send; :ai edits; :ai apply; :ai save/load PATH; :ai new[!]; :ai cancel"),
            "context" => {
                self.ai.idle()?;
                let root = self.ai_root()?;
                let context = match arg {
                    "file" | "selection" => {
                        let range = if arg == "selection" { Some(self.selection().ok_or_else(|| error("Select code with v/V then Ctrl-g to preview AI context"))?.chars) } else { None };
                        let path = self.document.path().ok_or_else(|| error("Name the source file before adding AI context"))?;
                        self.ai_capture(&root, path, range, arg)?
                    }
                    "symbol" => {
                        let (path, range) = self.ai_lsp_symbol()?;
                        let context = self.ai_capture(&root, &path, Some(0..0), "symbol")?;
                        let (text, _) = self.ai_source(&context, true)?;
                        self.ai_capture(&root, &path, Some(lsp::scalar_range(&text, range)?), "symbol")?
                    }
                    "hunk" | "diff" => self.ai_diff_context(arg == "hunk")?,
                    _ => return Err(error("Use :ai context file|selection|symbol|hunk|diff")),
                };
                let mut draft = Draft { question: self.ai.draft.question.clone(), ..Default::default() };
                draft.add(context)?;
                self.ai.draft = draft;
                self.ai.plan = None;
                self.ai_show(Panel::Preview);
                self.info("Context captured locally; :ai ask QUESTION, then :ai send");
            }
            "add" => {
                self.ai.idle()?;
                let root = self.ai_root()?;
                let (kind, path) = arg.split_once(char::is_whitespace).map_or((arg, ""), |(n, p)| (n, p.trim()));
                let mut draft = self.ai.draft.clone();
                match kind {
                    "file" | "tests" if !path.is_empty() => draft.add(self.ai_capture(&root, Path::new(path), None, kind)?)?,
                    "diagnostics" if path.is_empty() => {
                        let set = self.current_diagnostics().ok_or_else(|| error("No current diagnostics to attach"))?;
                        let text = set.items.iter().map(|d| format!("{}:{} {}", d.range.start.line + 1, d.range.start.character + 1, d.message)).collect::<Vec<_>>().join("\n");
                        draft.add(Context { label: format!("Diagnostics · {} · {} · positions are line/UTF-16", self.document.display_name(), if set.versioned { "version matched" } else { "versionless, unverified" }), text, source: None })?;
                    }
                    "references" if path.is_empty() => {
                        for (path, position) in self.ai_lsp_references()? {
                            let empty = self.ai_capture(&root, &path, Some(0..0), "reference")?;
                            let (text, _) = self.ai_source(&empty, true)?;
                            let scalar = lsp::scalar(&text, position)?;
                            let line = text.char_to_line(scalar);
                            let start = text.line_to_char(line);
                            let end = if line + 1 < text.len_lines() { text.line_to_char(line + 1) } else { text.len_chars() };
                            draft.add(self.ai_capture(&root, &path, Some(start..end), "reference")?)?;
                        }
                    }
                    "feedback" if path.is_empty() => {
                        let store = self.review_ready()?;
                        if !self.git_current() { return Err(error("Wait for a current review comparison before attaching feedback")); }
                        let export = crate::review_store::Export { freshness: store.records.iter().map(|r| (r.id, self.review_freshness(r))).collect(), store,
                            destination: PathBuf::new(), protected: Vec::new(), scope: "Loaded review annotations; validation is bounded to the current comparison and at most 32 paths. This is feedback, not an instruction to execute commands.".into() };
                        draft.add(Context { label: "Exported review feedback (Markdown)".into(), text: crate::review_store::markdown_bounded(&export, ai::MAX_ATTACHMENT).map_err(io::Error::other)?, source: None })?;
                    }
                    _ => return Err(error("Use :ai add file|tests PATH, diagnostics, references, or feedback")),
                }
                self.ai.draft = draft;
                self.ai_show(Panel::Preview);
            }
            "ask" => {
                self.ai.idle()?; self.ai_root()?;
                let draft = Draft { question: arg.into(), context: self.ai.draft.context.clone() };
                draft.validate()?; self.ai.draft = draft;
                self.ai_show(Panel::Preview);
                self.info("Review context, then :ai send transmits it to Codex");
            }
            "preview" if arg.is_empty() => self.ai_show(Panel::Preview),
            "send" if arg.is_empty() => self.ai_send()?,
            "cancel" if arg.is_empty() => { self.ai.cancel()?; self.info("AI request cancelled; conversation retained"); },
            "edits" if arg.is_empty() => {
                self.ai.idle()?;
                let plan = self.prepare_ai_edit()?;
                self.ai.plan = Some(plan); self.ai_show(Panel::Edits);
                self.info("Review replacements; :ai apply approves unsaved buffer edits");
            }
            "apply" if arg.is_empty() => {
                self.ai.idle()?;
                if self.view != View::Ai || self.ai.panel != Panel::Edits { return Err(error("Use :ai edits to review replacements before :ai apply")); }
                let plan = self.ai.plan.take().ok_or_else(|| error("No AI edit preview"))?;
                self.apply_edit_plan(plan)?;
                self.ai.status = "Suggestions applied as unsaved buffer edits; undo with u".into();
                if let Some(t) = self.ai.conversation.as_mut().and_then(|c| c.turns.last_mut()) { t.detail = self.ai.status.clone(); }
                self.ai.dirty = true;
                self.ai_show(Panel::Conversation);
            }
            "save" => {
                self.ai.idle()?;
                let conversation = self.ai.conversation.clone().ok_or_else(|| error("No AI conversation to save"))?;
                let path = if arg.is_empty() { self.ai.saved.as_ref().map(|(p, _)| p.clone()).ok_or_else(|| error("Use :ai save /outside/project/conversation.json"))? } else { absolute_path(Path::new(arg))? };
                let expected = self.ai.saved.as_ref().filter(|(p, _)| p == &path).map(|(_, h)| h.clone());
                let protected = self.lsp_edit_guards().into_keys().collect();
                self.ai.submit(Job::Save { conversation, path, expected, protected })?;
                self.ai.status = "Saving conversation".into();
                self.info("Saving AI conversation");
            }
            "load" | "load!" if !arg.is_empty() => {
                self.ai.idle()?;
                if self.ai.dirty && name != "load!" { return Err(error("Unsaved AI conversation; save first or use :ai load! PATH")); }
                let path = absolute_path(Path::new(arg))?;
                self.ai.submit(Job::Load(path))?;
                self.ai.status = "Loading conversation".into();
                self.ai_show(Panel::Conversation);
            }
            "new" | "new!" if arg.is_empty() => {
                self.ai.idle()?;
                if self.ai.dirty && name != "new!" { return Err(error("Unsaved AI conversation; save first or use :ai new!")); }
                self.ai.conversation = None; self.ai.draft = Draft::default(); self.ai.saved = None;
                self.ai.plan = None; self.ai.dirty = false; self.ai.partial = None; self.ai.status.clear();
                self.ai_show(Panel::Conversation);
            }
            _ => return Err(error("Unknown AI command; :ai help lists commands")),
        }
        Ok(())
    }

    fn ai_diff_context(&self, hunk: bool) -> io::Result<Context> {
        if !self.git_current() {
            return Err(error(
                "Request a current :diff before adding AI diff context",
            ));
        }
        let diff = self.git.diff().ok_or_else(|| error("No diff available"))?;
        let view = self
            .git
            .diff_view
            .as_ref()
            .ok_or_else(|| error("Open :diff first"))?;
        let range = if hunk {
            let start = diff
                .hunks
                .iter()
                .copied()
                .take_while(|r| *r <= view.selected)
                .last()
                .ok_or_else(|| error("Select a diff hunk first"))?;
            let end = diff
                .hunks
                .iter()
                .copied()
                .find(|r| *r > start)
                .unwrap_or(diff.rows.len());
            start..end
        } else {
            0..diff.rows.len()
        };
        let mut text = format!(
            "{}\n{} -> {}\n",
            crate::git::display_path(&view.path),
            diff.left_label,
            diff.right_label
        );
        for row in &diff.rows[range] {
            let prefix = match row.kind {
                crate::diff::Kind::Add => "+",
                crate::diff::Kind::Delete => "-",
                crate::diff::Kind::Context => " ",
                crate::diff::Kind::Header => "",
            };
            text.push_str(prefix);
            text.push_str(&row.text);
            text.push('\n');
            if row.no_newline {
                text.push_str("\\ No newline at end of file\n");
            }
            if text.len() > ai::MAX_ATTACHMENT {
                return Err(error("AI diff exceeds 64 KiB; select a hunk"));
            }
        }
        Ok(Context {
            label: if hunk {
                "Captured hunk"
            } else {
                "Captured diff"
            }
            .into(),
            text,
            source: None,
        })
    }

    fn ai_send(&mut self) -> io::Result<()> {
        self.ai.idle()?;
        if self.view != View::Ai || self.ai.panel != Panel::Preview {
            return Err(error("Open :ai preview before :ai send"));
        }
        let conversation = self
            .ai
            .conversation
            .as_ref()
            .ok_or_else(|| error("Add AI context first"))?;
        let prompt = conversation.prompt(&self.ai.draft)?;
        for c in &self.ai.draft.context {
            if c.source.is_some() {
                self.ai_source(c, true)?;
            }
        }
        let draft = self.ai.draft.clone();
        self.ai.submit(Job::Ask {
            config: self.ai.config.clone(),
            prompt,
            draft: draft.clone(),
        })?;
        self.ai.conversation.as_mut().unwrap().turns.push(Turn {
            draft,
            status: Status::Running,
            answer: None,
            detail: String::new(),
        });
        self.ai.draft.question.clear();
        self.ai.plan = None;
        self.ai.dirty = true;
        self.ai.status = "Starting Codex read-only request".into();
        self.ai_show(Panel::Conversation);
        self.info("Codex request started; Escape returns to source, :ai cancel stops it");
        Ok(())
    }

    pub(crate) fn tick_ai(&mut self) -> bool {
        let Some(worker) = &self.ai.worker else {
            return false;
        };
        let (progress, reply) = worker.poll();
        let mut changed = false;
        if let Some(progress) = progress.filter(|p| p.serial == self.ai.serial && self.ai.pending) {
            self.ai.status = progress.message;
            if progress.answer.is_some() {
                self.ai.partial = progress.answer;
            }
            changed = true;
        }
        if let Some(reply) = reply.filter(|r| r.serial == self.ai.serial && self.ai.pending) {
            self.ai.pending = false;
            self.ai.partial = None;
            changed = true;
            match reply.result {
                Ok(Outcome::Answer(answer)) => {
                    if let Some(turn) = self
                        .ai
                        .conversation
                        .as_mut()
                        .and_then(|c| c.turns.last_mut())
                    {
                        turn.answer = Some(answer);
                        turn.status = Status::Completed;
                    }
                    self.ai.status =
                        "Codex complete; model claims require source verification".into();
                    self.info("Codex answer ready in :ai; :ai edits previews suggestions");
                }
                Ok(Outcome::Saved(path, hash)) => {
                    self.ai.saved = Some((path, hash));
                    self.ai.dirty = false;
                    self.ai.status = "Conversation saved".into();
                    self.info("AI conversation saved");
                }
                Ok(Outcome::Loaded(conversation, path, hash)) => {
                    self.ai.conversation = Some(conversation);
                    self.ai.saved = Some((path, hash));
                    self.ai.dirty = false;
                    self.ai.draft = Draft::default();
                    self.ai.plan = None;
                    self.ai.status =
                        "Conversation restored; capture fresh context for follow-up edits".into();
                    self.info("AI conversation restored; capture current context before sending a follow-up");
                }
                Err(e) => {
                    self.ai.status = e.to_string();
                    if self.ai.inference
                        && let Some(turn) = self
                            .ai
                            .conversation
                            .as_mut()
                            .and_then(|c| c.turns.last_mut())
                    {
                        turn.status = Status::Failed;
                        turn.detail = self.ai.status.chars().take(4096).collect();
                    }
                    self.error(&format!("AI: {e}"));
                }
            }
        }
        if changed {
            self.ai.rebuild();
        }
        changed
    }

    pub fn prepare_ai_edit(&self) -> io::Result<EditPlan> {
        let turn = self
            .ai
            .conversation
            .as_ref()
            .and_then(|c| c.turns.last())
            .ok_or_else(|| error("No AI answer"))?;
        if turn.status != Status::Completed {
            return Err(error("Wait for a successful complete AI answer"));
        }
        let answer = turn.answer.as_ref().ok_or_else(|| error("No AI answer"))?;
        answer.validate(&turn.draft)?;
        if answer.edits.is_empty() {
            return Err(error("The latest answer has no suggested edits"));
        }
        let mut files: BTreeMap<PathBuf, PlannedFile> = BTreeMap::new();
        let mut captured = BTreeMap::new();
        for replacement in &answer.edits {
            let context = ai::context_at(&turn.draft, replacement.context)?;
            let source = context
                .source
                .as_ref()
                .ok_or_else(|| error("Cannot edit non-source context"))?;
            if let std::collections::btree_map::Entry::Vacant(entry) =
                captured.entry(replacement.context)
            {
                entry.insert(self.ai_source(context, true)?);
            }
            let (before, unopened) = captured.get_mut(&replacement.context).unwrap();
            let byte = context
                .text
                .find(&replacement.before)
                .ok_or_else(|| error("Missing original text"))?;
            let start = source.start + context.text[..byte].chars().count();
            let range = start..start + replacement.before.chars().count();
            let file = files
                .entry(source.path.clone())
                .or_insert_with(|| PlannedFile {
                    path: source.path.clone(),
                    after: before.clone(),
                    before: before.clone(),
                    edits: Vec::new(),
                    stamp: source.stamp.clone(),
                    unopened: unopened.take(),
                });
            file.edits.push((range, replacement.after.clone()));
        }
        for file in files.values_mut() {
            file.edits.sort_by_key(|(r, _)| (r.start, r.end));
            if file
                .edits
                .windows(2)
                .any(|w| w[0].0.end > w[1].0.start || w[0].0.start == w[1].0.start)
            {
                return Err(error("AI replacements overlap or repeat a target"));
            }
            // Scan each file once, independent of the number of replacements.
            // UTF-8/UTF-16 validity alone cannot protect graphemes or CRLF pairs.
            let whole = file.before.to_string();
            let mut boundaries = whole
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .chain(std::iter::once(whole.len()))
                .peekable();
            for (range, _) in &file.edits {
                for scalar in [range.start, range.end] {
                    let byte = file.before.char_to_byte(scalar);
                    while boundaries.peek().is_some_and(|i| *i < byte) {
                        boundaries.next();
                    }
                    if boundaries.peek() != Some(&byte) {
                        return Err(error("AI replacement splits a grapheme or CRLF boundary"));
                    }
                }
            }
            for (range, text) in file.edits.iter().rev() {
                file.after.remove(range.clone());
                file.after.insert(range.start, text);
            }
            if file.after.len_bytes() > lsp::MAX_DOCUMENT_BYTES {
                return Err(error("AI edit result exceeds 2 MiB"));
            }
        }
        Ok(EditPlan {
            edit_count: answer.edits.len(),
            files: files.into_values().collect(),
        })
    }

    fn ai_jump(&mut self, link: Link) -> io::Result<()> {
        let (text, loaded) = self.ai_source(&link.context, false)?;
        let source = link.context.source.as_ref().unwrap();
        let offset: usize = link
            .context
            .text
            .split_inclusive('\n')
            .take(link.line - 1)
            .map(|s| s.chars().count())
            .sum();
        let scalar = (source.start + offset).min(text.len_chars());
        let origin = self.source_location();
        if let Some(document) = loaded {
            self.open_prepared_buffer(document);
        } else {
            self.open_buffer_unrecorded(&source.path)?;
        }
        self.document.buffer.set_cursor(scalar, false);
        self.record_navigation(origin);
        self.info("Opened captured source; Ctrl-o returns, :ai reopens conversation");
        Ok(())
    }
    pub(crate) fn ai_key(&mut self, key: Key) {
        match key {
            Key::Escape | Key::Ctrl('[') => {
                self.ai.plan = None;
                self.view = std::mem::take(&mut self.ai.return_view);
                self.selection_anchor = None;
            }
            Key::Ctrl('c') => {
                if let Err(e) = self.ai.cancel() {
                    self.error(&e.to_string());
                }
            }
            Key::Char(':') => {
                self.mode = Mode::Command;
                self.command.clear();
            }
            Key::Char('j') | Key::Down => {
                self.ai.selected = (self.ai.selected + 1).min(self.ai.rows.len().saturating_sub(1))
            }
            Key::Char('k') | Key::Up => self.ai.selected = self.ai.selected.saturating_sub(1),
            Key::PageDown => {
                self.ai.selected = (self.ai.selected + 10).min(self.ai.rows.len().saturating_sub(1))
            }
            Key::PageUp => self.ai.selected = self.ai.selected.saturating_sub(10),
            Key::Char('g') => self.ai.selected = 0,
            Key::Char('G') => self.ai.selected = self.ai.rows.len().saturating_sub(1),
            Key::Char('h') | Key::Left => self.ai.left = self.ai.left.saturating_sub(4),
            Key::Char('l') | Key::Right => self.ai.left = self.ai.left.saturating_add(4),
            Key::Enter => {
                if let Some(link) = self
                    .ai
                    .rows
                    .get(self.ai.selected)
                    .and_then(|r| r.link.clone())
                    && let Err(e) = self.ai_jump(link)
                {
                    self.error(&e.to_string());
                }
            }
            _ => {}
        }
    }
}
fn open_small(path: &Path) -> io::Result<Document> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.len() > lsp::MAX_DOCUMENT_BYTES as u64 {
        return Err(error("AI source exceeds 2 MiB"));
    }
    Document::open(path)
}

pub fn render(frame: &mut ratatui::Frame<'_>, editor: &mut Editor, theme: &crate::theme::Theme) {
    use ratatui::{layout::Rect, style::Style, widgets::Paragraph};
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let ai = &mut editor.ai;
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let title = format!(
        "AI · {}{}",
        ai.status,
        if ai.dirty {
            " · conversation unsaved"
        } else {
            ""
        }
    );
    frame.render_widget(
        Paragraph::new(crate::text::visible_text(&title, 0, area.width as usize))
            .style(style.fg(theme.command)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let height = area.height.saturating_sub(2) as usize;
    ai.top = ai.top.min(ai.selected);
    if height > 0 && ai.selected >= ai.top + height {
        ai.top = ai.selected + 1 - height;
    }
    for (i, row) in ai.rows.iter().skip(ai.top).take(height).enumerate() {
        frame.render_widget(
            Paragraph::new(crate::text::visible_text(
                &row.text,
                ai.left,
                area.width as usize,
            ))
            .style(if ai.top + i == ai.selected {
                style.bg(theme.selection)
            } else if row.link.is_some() {
                style.fg(theme.command)
            } else {
                style
            }),
            Rect::new(area.x, area.y + 1 + i as u16, area.width, 1),
        );
    }
    if area.height >= 2 {
        let command = format!(":{}", editor.command);
        let text = if editor.mode == Mode::Command {
            &command
        } else if editor.message_is_error {
            &editor.message
        } else {
            "j/k select · h/l scroll · Enter source · Esc return · :ai help"
        };
        let cells = crate::text::display_column(text, text.chars().count());
        let offset = if editor.mode == Mode::Command {
            cells.saturating_sub(area.width.saturating_sub(1) as usize)
        } else {
            0
        };
        frame.render_widget(
            Paragraph::new(crate::text::visible_text(text, offset, area.width as usize)).style(
                style.fg(if editor.message_is_error {
                    theme.error
                } else {
                    theme.muted
                }),
            ),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
        if editor.mode == Mode::Command {
            frame.set_cursor_position((
                area.x + (cells - offset).min(area.width as usize - 1) as u16,
                area.y + area.height - 1,
            ));
        }
    }
}
