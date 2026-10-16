//! Capability-gated language commands and one contextual result/preview view.
use crate::{
    editor::{Editor, Mode, View},
    input::Key,
    lsp::{self, Config, Event, Language, Request, ServerKey, Snapshot, Stamp, Versioned},
    lsp_edit::EditPlan,
    lsp_worker::{Control, Worker},
};
use lsp_types::{Diagnostic, Position};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    path::{Path, PathBuf},
    time::Instant,
};

#[path = "lsp_completion.rs"]
mod completion;
pub use completion::CompletionMenu;
pub(crate) use completion::render as render_completion;

#[derive(Default)]
pub struct LspState {
    pub config: Config,
    pub installations: BTreeMap<Language, crate::lsp_install::State>,
    worker: Option<Worker>,
    snapshots: Vec<Snapshot>,
    virtual_sources: BTreeMap<u64, lsp::VirtualSource>,
    sessions: BTreeSet<ServerKey>,
    warm_sessions: BTreeSet<ServerKey>,
    pub discovering: bool,
    pub routes: BTreeMap<PathBuf, ServerKey>,
    pub unavailable: BTreeMap<PathBuf, String>,
    pub servers: BTreeMap<ServerKey, ServerInfo>,
    pub diagnostics: BTreeMap<PathBuf, DiagnosticSet>,
    pub panel: Option<Panel>,
    pub completion: Option<CompletionMenu>,
    completion_due: Option<completion::Scheduled>,
    call_history: Vec<Panel>,
    context: Option<Context>,
    serial: u64,
    pub pending: bool,
    started: Option<Instant>,
    status_redraw: Option<Instant>,
}
#[derive(Default)]
pub struct ServerInfo {
    pub epoch: u64,
    pub status: lsp::Status,
    pub capabilities: Value,
    pub logs: VecDeque<String>,
}
pub struct DiagnosticSet {
    pub stamp: Stamp,
    dependencies: Option<BTreeMap<PathBuf, Stamp>>,
    pub versioned: bool,
    pub items: Vec<Diagnostic>,
}
#[derive(Clone)]
struct Context {
    serial: u64,
    stamp: Stamp,
    path: PathBuf,
    cursor: usize,
    mode: Mode,
    guards: BTreeMap<PathBuf, Stamp>,
    key: ServerKey,
    epoch: u64,
    apply_resolved: bool,
    inline_completion: bool,
    jump_to_single_location: bool,
    ai_question: Option<String>,
    call_item: Option<Value>,
    virtual_target: Option<(String, Position)>,
}
pub struct Panel {
    pub title: String,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub top: usize,
    pub left: usize,
    context: Option<Context>,
    versions: Vec<Versioned>,
    plan: Option<EditPlan>,
    reply: Option<(ServerKey, u64, Value)>,
    continuation: Option<Value>,
    return_mode: Mode,
}
pub struct Row {
    pub label: String,
    pub detail: String,
    action: Item,
}
enum Item {
    Text,
    VirtualJump {
        uri: String,
        position: Position,
    },
    Jump {
        path: PathBuf,
        position: Position,
        stamp: Option<Stamp>,
        symbol_range: Option<lsp_types::Range>,
    },
    Completion {
        value: Value,
        resolved: bool,
    },
    Action(Value),
    WorkspaceSymbol(Value),
    Call(Value),
}
impl Panel {
    fn text(title: &str, text: &str) -> Self {
        Self {
            title: title.into(),
            rows: text
                .lines()
                .take(lsp::MAX_ITEMS)
                .map(|line| Row {
                    label: line.into(),
                    detail: String::new(),
                    action: Item::Text,
                })
                .collect(),
            selected: 0,
            top: 0,
            left: 0,
            context: None,
            versions: Vec::new(),
            plan: None,
            reply: None,
            continuation: None,
            return_mode: Mode::Normal,
        }
    }
}
impl LspState {
    fn cancel_request(&mut self) {
        if let Some(context) = &self.context
            && let Some(worker) = &self.worker
        {
            let _ = worker.submit(Control::Cancel(context.path.clone()));
        }
        self.serial += 1;
        self.context = None;
        self.pending = false;
    }
    pub fn cancel(&mut self) {
        self.cancel_request();
        self.completion = None;
        self.completion_due = None;
        self.call_history.clear();
        if let Some(panel) = self.panel.take()
            && let Some((key, epoch, id)) = panel.reply
            && let Some(worker) = &self.worker
        {
            let _ = worker.submit(Control::ApplyReply {
                key,
                epoch,
                id,
                applied: false,
                reason: "Edit preview cancelled".into(),
            });
        }
    }
}

impl Editor {
    pub(crate) fn ai_request_symbol(&mut self, question: &str) -> io::Result<()> {
        let question = if question.is_empty() {
            crate::ai::EXPLAIN_SOURCE
        } else {
            question
        };
        crate::ai::Draft {
            question: question.into(),
            ..Default::default()
        }
        .validate()?;
        self.lsp_command("Outline", "")?;
        self.lsp.context.as_mut().unwrap().ai_question = Some(question.into());
        self.info("Finding the containing symbol for a local AI preview; Escape cancels");
        Ok(())
    }

    /// Queue directory discovery on the existing manager. No filesystem or
    /// server work occurs on the editor thread. None cancels unused preparation.
    pub fn prepare_lsp(&mut self, directory: Option<PathBuf>) -> io::Result<()> {
        if directory.is_some() && self.lsp.worker.is_none() {
            self.lsp.worker = Some(Worker::new(self.lsp.config.clone())?);
        }
        let discovering = directory.is_some();
        if let Some(worker) = &self.lsp.worker
            && worker.prepare(directory)
        {
            self.lsp.discovering = discovering;
        }
        Ok(())
    }

    pub(crate) fn ai_lsp_symbol(&self) -> io::Result<(PathBuf, lsp_types::Range)> {
        let panel = self
            .lsp
            .panel
            .as_ref()
            .ok_or_else(|| error("Use :Outline and select a symbol first"))?;
        if !panel
            .context
            .as_ref()
            .is_some_and(|c| self.lsp_context_current(c))
        {
            return Err(error("Symbol results are stale; request :Outline again"));
        }
        match &panel
            .rows
            .get(panel.selected)
            .ok_or_else(|| error("No symbol selected"))?
            .action
        {
            Item::Jump {
                path,
                symbol_range: Some(range),
                ..
            } => Ok((path.clone(), *range)),
            _ => Err(error("Select a document symbol in :Outline first")),
        }
    }

    pub(crate) fn ai_lsp_references(&self) -> io::Result<Vec<(PathBuf, Position)>> {
        let panel = self
            .lsp
            .panel
            .as_ref()
            .ok_or_else(|| error("Request :References first"))?;
        if !panel
            .context
            .as_ref()
            .is_some_and(|c| self.lsp_context_current(c))
            || panel.title != "references"
        {
            return Err(error(
                "Request fresh :References before attaching references",
            ));
        }
        let locations: Vec<_> = panel
            .rows
            .iter()
            .filter_map(|r| match &r.action {
                Item::Jump { path, position, .. } => Some((path.clone(), *position)),
                _ => None,
            })
            .collect();
        if locations.is_empty() || locations.len() > crate::ai::MAX_ATTACHMENTS {
            return Err(error(
                "References require 1–16 locations; select individual files for larger results",
            ));
        }
        Ok(locations)
    }

    fn lsp_snapshots(&self) -> Vec<Snapshot> {
        let active = self
            .note_edit
            .as_ref()
            .map_or(&self.document, |note| &note.source);
        std::iter::once((self.active_buffer_id, active))
            .chain(self.buffers.iter().map(|(id, slot)| (*id, &slot.document)))
            .filter_map(|(id, doc)| {
                let virtual_source = self
                    .lsp
                    .virtual_sources
                    .get(&doc.buffer.instance())
                    .cloned();
                let path = lsp_identity(doc)?;
                let language = virtual_source
                    .as_ref()
                    .map(|v| v.key.language)
                    .or_else(|| Language::for_path(&path))?;
                if doc.buffer.text().len_bytes() > lsp::MAX_DOCUMENT_BYTES {
                    return None;
                }
                Some(Snapshot {
                    stamp: Stamp {
                        buffer_id: id,
                        instance: doc.buffer.instance(),
                        generation: doc.buffer.generation(),
                    },
                    path,
                    text: doc.buffer.text().clone(),
                    saved: doc.save_generation(),
                    language,
                    virtual_source,
                })
            })
            .collect()
    }
    fn lsp_context_current(&self, context: &Context) -> bool {
        self.note_edit.is_none()
            && self.active_buffer_id == context.stamp.buffer_id
            && self.document.buffer.instance() == context.stamp.instance
            && self.document.buffer.generation() == context.stamp.generation
            && lsp_identity(&self.document).as_ref() == Some(&context.path)
            && self.document.buffer.cursor() == context.cursor
            && self.lsp_edit_guards() == context.guards
            && self
                .lsp
                .servers
                .get(&context.key)
                .is_some_and(|s| s.epoch == context.epoch && s.capabilities.is_object())
            && matches!(self.view, View::Editing | View::Lsp)
            && (!context.inline_completion
                || (self.view == View::Editing && self.mode == Mode::Insert))
    }
    pub(crate) fn tick_lsp(&mut self) -> bool {
        if !self.lsp.virtual_sources.is_empty() {
            let active = self
                .note_edit
                .as_ref()
                .map_or(&self.document, |note| &note.source);
            let instances: BTreeSet<_> = std::iter::once(active.buffer.instance())
                .chain(self.buffers.values().map(|s| s.document.buffer.instance()))
                .collect();
            self.lsp
                .virtual_sources
                .retain(|instance, _| instances.contains(instance));
        }
        let snapshots = self.lsp_snapshots();
        if self.lsp.worker.is_none() && !snapshots.is_empty() {
            match Worker::new(self.lsp.config.clone()) {
                Ok(worker) => self.lsp.worker = Some(worker),
                Err(e) => {
                    self.error(&format!("Cannot start LSP manager: {e}"));
                    return false;
                }
            }
        }
        let changed = self.lsp.snapshots.len() != snapshots.len()
            || !self
                .lsp
                .snapshots
                .iter()
                .zip(&snapshots)
                .all(|(a, b)| a.same_document(b));
        if changed {
            if let Some(worker) = &self.lsp.worker {
                worker.sync(snapshots.clone());
            }
            self.lsp.snapshots = snapshots;
            self.lsp
                .routes
                .retain(|path, _| self.lsp.snapshots.iter().any(|s| &s.path == path));
            self.lsp
                .unavailable
                .retain(|path, _| self.lsp.snapshots.iter().any(|s| &s.path == path));
            if self.lsp.snapshots.is_empty() {
                self.lsp
                    .sessions
                    .retain(|key| self.lsp.warm_sessions.contains(key));
                self.lsp
                    .servers
                    .retain(|key, _| self.lsp.warm_sessions.contains(key));
            }
        }
        let mut redraw = changed;
        if self.lsp.context.as_ref().is_some_and(|c| {
            !self.lsp_context_current(c)
                || (self.lsp.pending && self.view == View::Editing && self.mode != c.mode)
        }) {
            self.close_lsp_panel();
            redraw = true;
        }
        let stamps: BTreeMap<_, _> = self
            .lsp
            .snapshots
            .iter()
            .map(|s| (s.path.clone(), s.stamp.clone()))
            .collect();
        self.lsp.diagnostics.retain(|path, d| {
            stamps.get(path) == Some(&d.stamp)
                && self.lsp.routes.get(path).is_some_and(|key| {
                    diagnostic_dependencies_current(
                        &d.dependencies,
                        key,
                        &self.lsp.snapshots,
                        &self.lsp.routes,
                    )
                })
        });
        for _ in 0..128 {
            let Some(event) = self.lsp.worker.as_mut().and_then(Worker::poll) else {
                break;
            };
            redraw = true;
            if let Err(e) = self.lsp_event(event) {
                self.lsp.pending = false;
                self.error(&e.to_string());
            }
        }
        if self.lsp.servers.values().any(|s| s.status.phase.busy())
            && self
                .lsp
                .status_redraw
                .is_none_or(|last| last.elapsed().as_secs() >= 1)
        {
            self.lsp.status_redraw = Some(Instant::now());
            redraw = true;
        }
        if redraw
            && self.view == View::Lsp
            && self
                .lsp
                .panel
                .as_ref()
                .is_some_and(|panel| matches!(panel.title.as_str(), "LspInfo" | "LspLog"))
        {
            let panel = self.lsp.panel.as_ref().unwrap();
            let (selected, top, left) = (panel.selected, panel.top, panel.left);
            let title = panel.title.clone();
            self.lsp_information(&title);
            if let Some(panel) = &mut self.lsp.panel {
                panel.selected = selected.min(panel.rows.len().saturating_sub(1));
                panel.top = top.min(panel.rows.len().saturating_sub(1));
                panel.left = left;
            }
        }
        if self.lsp.pending
            && self.lsp.started.is_some_and(|t| {
                t.elapsed() > self.lsp.config.request_timeout + std::time::Duration::from_secs(3)
            })
        {
            let quiet = self
                .lsp
                .context
                .as_ref()
                .is_some_and(|c| c.inline_completion && !c.apply_resolved);
            self.lsp.cancel_request();
            if !quiet {
                self.error("LSP request expired; :LspInfo or :LspRestart");
            }
            redraw = true;
        }
        self.tick_completion();
        redraw
    }
    fn lsp_event(&mut self, event: Event) -> io::Result<()> {
        match event {
            Event::Sessions {
                keys,
                warm,
                discovering,
            } => {
                self.lsp.warm_sessions = warm.into_iter().collect();
                // Membership can have been queued before the last source was
                // closed. Do not resurrect its UI state during async teardown.
                self.lsp.sessions = keys
                    .into_iter()
                    .filter(|key| {
                        self.lsp.warm_sessions.contains(key)
                            || self.lsp.routes.values().any(|route| route == key)
                    })
                    .collect();
                self.lsp.discovering = discovering;
                self.lsp
                    .servers
                    .retain(|key, _| self.lsp.sessions.contains(key));
                for key in &self.lsp.sessions {
                    self.lsp
                        .servers
                        .entry(key.clone())
                        .or_insert_with(|| ServerInfo {
                            status: lsp::Status::new(
                                lsp::Phase::Starting,
                                format!("Preparing {}", key.language.id()),
                            ),
                            ..ServerInfo::default()
                        });
                }
            }
            Event::Installation { language, state } => {
                self.lsp.installations.insert(language, state);
            }
            Event::Doctor { path, report } => {
                if self.document.path() == Some(&path)
                    && self.view == View::Lsp
                    && let Some(panel) = &mut self.lsp.panel
                    && panel.title == "Doctor"
                {
                    panel.rows.extend(Panel::text("", &report).rows);
                }
            }
            Event::Route { path, key } => {
                if !self.lsp.snapshots.iter().any(|s| s.path == path) {
                    return Ok(());
                }
                self.lsp.unavailable.remove(&path);
                self.lsp.routes.insert(path, key);
            }
            Event::Unavailable { path, message } => {
                if !self.lsp.snapshots.iter().any(|s| s.path == path) {
                    return Ok(());
                }
                self.lsp.unavailable.insert(path, message);
            }
            Event::Status {
                key,
                epoch,
                state,
                capabilities,
            } => {
                if !self.lsp.sessions.contains(&key) {
                    return Ok(());
                }
                let info = self.lsp.servers.entry(key.clone()).or_default();
                if epoch < info.epoch {
                    return Ok(());
                }
                if epoch != info.epoch || capabilities.as_ref().is_some_and(Value::is_null) {
                    info.capabilities = Value::Null;
                    self.lsp
                        .diagnostics
                        .retain(|path, _| self.lsp.routes.get(path) != Some(&key));
                }
                info.epoch = epoch;
                info.status = state;
                if let Some(capabilities) = capabilities {
                    info.capabilities = capabilities;
                }
            }
            Event::Log { key, message } => {
                if !self.lsp.sessions.contains(&key) {
                    return Ok(());
                }
                let info = self.lsp.servers.entry(key).or_default();
                info.logs.push_back(message.chars().take(4096).collect());
                while info.logs.len() > 16 {
                    info.logs.pop_front();
                }
            }
            Event::Diagnostics {
                key,
                epoch,
                document,
                dependencies,
                versioned,
                items,
            } => {
                if self
                    .lsp
                    .servers
                    .get(&key)
                    .is_some_and(|s| s.epoch == epoch && s.capabilities.is_object())
                    && self.lsp.snapshots.iter().any(|s| {
                        s.path == document.snapshot.path && s.stamp == document.snapshot.stamp
                    })
                    && diagnostic_dependencies_current(
                        &dependencies,
                        &key,
                        &self.lsp.snapshots,
                        &self.lsp.routes,
                    )
                {
                    self.lsp.diagnostics.insert(
                        document.snapshot.path,
                        DiagnosticSet {
                            stamp: document.snapshot.stamp,
                            dependencies,
                            versioned,
                            items,
                        },
                    );
                }
            }
            Event::Response {
                key,
                epoch,
                request,
                documents,
                result,
            } => {
                let Some(context) = self.lsp.context.clone().filter(|c| {
                    c.serial == request.serial
                        && c.key == key
                        && c.epoch == epoch
                        && self.lsp_context_current(c)
                }) else {
                    return Ok(());
                };
                self.lsp.pending = false;
                if context.inline_completion
                    && !context.apply_resolved
                    && let Err(message) = &result
                {
                    self.lsp.cancel_request();
                    return self.lsp_event(Event::Log {
                        key,
                        message: message.clone(),
                    });
                }
                let result = result.map_err(io::Error::other)?;
                self.lsp_response(&request.method, result, context, documents)?;
            }
            Event::Apply {
                key,
                epoch,
                id,
                request,
                documents,
                edit,
            } => {
                let context = self.lsp.context.clone().filter(|c| {
                    c.serial == request.serial
                        && c.key == key
                        && c.epoch == epoch
                        && self.lsp_context_current(c)
                        && self.lsp.panel.as_ref().is_none_or(|p| p.plan.is_none())
                });
                if let Some(context) = context {
                    match self.stage_lsp_edit(&edit, context, documents, None) {
                        Ok(()) => {
                            self.lsp.pending = false;
                            self.lsp.panel.as_mut().unwrap().reply = Some((key, epoch, id));
                        }
                        Err(e) => {
                            self.lsp
                                .worker
                                .as_ref()
                                .unwrap()
                                .submit(Control::ApplyReply {
                                    key,
                                    epoch,
                                    id,
                                    applied: false,
                                    reason: e.to_string(),
                                })?;
                            return Err(e);
                        }
                    }
                } else if let Some(worker) = &self.lsp.worker {
                    worker.submit(Control::ApplyReply {
                        key,
                        epoch,
                        id,
                        applied: false,
                        reason: "Obsolete or concurrent edit request".into(),
                    })?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn lsp_navigation_shortcut(&mut self, name: &str) -> io::Result<()> {
        self.lsp_command(name, "")?;
        if let Some(context) = &mut self.lsp.context {
            context.jump_to_single_location = true;
        }
        Ok(())
    }

    pub fn lsp_command(&mut self, name: &str, argument: &str) -> io::Result<()> {
        if self.note_edit.is_some() {
            return Err(error("Language commands are unavailable in a review note"));
        }
        if !matches!(self.view, View::Editing | View::Lsp) {
            return Err(error("Return to source before using language commands"));
        }
        self.dismiss_completion();
        if matches!(name, "LspInstall" | "LspInstallCancel") {
            let language = match argument.trim() {
                "" => self
                    .document
                    .path()
                    .and_then(Language::for_path)
                    .ok_or_else(|| error("Specify kotlin, java or csharp: :LspInstall LANGUAGE"))?,
                "kotlin" => Language::Kotlin,
                "java" => Language::Java,
                "csharp" | "c#" => Language::CSharp,
                _ => return Err(error("Supported installers: kotlin, java, csharp")),
            };
            if self.lsp.worker.is_none() {
                self.lsp.worker = Some(Worker::new(self.lsp.config.clone())?);
            }
            let cancel = name == "LspInstallCancel";
            self.lsp.worker.as_ref().unwrap().submit(if cancel {
                Control::InstallCancel(language)
            } else {
                Control::Install(language)
            })?;
            self.lsp.installations.insert(
                language,
                crate::lsp_install::State {
                    message: if cancel {
                        "Cancelling installation"
                    } else {
                        "Installation queued"
                    }
                    .into(),
                    busy: true,
                    failed: false,
                },
            );
            self.info(if cancel {
                "Cancelling language-server installation"
            } else {
                "Installing language server in background; :LspInfo shows progress"
            });
            return Ok(());
        }
        if self.view == View::Editing
            && self.lsp.pending
            && matches!(
                name,
                "Complete"
                    | "Hover"
                    | "Signature"
                    | "Definition"
                    | "References"
                    | "Rename"
                    | "CodeAction"
                    | "Format"
                    | "Symbols"
                    | "Outline"
                    | "WorkspaceSymbols"
                    | "Calls"
            )
        {
            self.lsp.cancel();
        }
        self.tick_lsp();
        match name {
            "LspInfo" | "Doctor" | "LspLog" => {
                self.lsp_information(name);
                if name == "Doctor"
                    && let (Some(worker), Some(path)) = (&self.lsp.worker, self.document.path())
                {
                    worker.submit(Control::Doctor(path.into()))?;
                }
                return Ok(());
            }
            "LspRestart" => {
                let path = lsp_identity(&self.document)
                    .ok_or_else(|| error("Name a source file first"))?;
                self.lsp
                    .worker
                    .as_ref()
                    .ok_or_else(|| error("No supported language server"))?
                    .submit(Control::Restart(path.clone()))?;
                self.close_lsp_panel();
                self.lsp.diagnostics.clear();
                if let Some(key) = self.lsp.routes.get(&path)
                    && let Some(info) = self.lsp.servers.get_mut(key)
                {
                    info.capabilities = Value::Null;
                    // Reject already queued results from the old process immediately.
                    info.epoch = info.epoch.saturating_add(1);
                    info.status = lsp::Status::new(
                        lsp::Phase::Starting,
                        "Restarting with current unsaved buffers",
                    );
                }
                self.info("Restarting language server; current unsaved buffers will be reopened");
                return Ok(());
            }
            "Diagnostics" => {
                self.open_diagnostics();
                return Ok(());
            }
            "Dnext" | "Dprev" => {
                self.close_lsp_panel();
                return self.diagnostic_step(name == "Dprev");
            }
            _ => {}
        }
        if self.view != View::Editing {
            return Err(error(
                "Return to source before requesting language intelligence",
            ));
        }
        let method = match name {
            "Complete" => "textDocument/completion",
            "Hover" => "textDocument/hover",
            "Signature" => "textDocument/signatureHelp",
            "Definition" => "textDocument/definition",
            "References" => "textDocument/references",
            "Rename" => "textDocument/rename",
            "CodeAction" => "textDocument/codeAction",
            "Format" => "textDocument/formatting",
            "Symbols" | "Outline" => "textDocument/documentSymbol",
            "WorkspaceSymbols" => "workspace/symbol",
            "Calls" => "textDocument/prepareCallHierarchy",
            _ => return Err(error("Unknown language command")),
        };
        if self.document.buffer.text().len_bytes() > lsp::MAX_DOCUMENT_BYTES {
            return Err(error("LSP documents are limited to 2 MiB"));
        }
        if self.document.buffer.is_read_only()
            && matches!(name, "Complete" | "Rename" | "CodeAction" | "Format")
        {
            return Err(error("Generated/decompiled source is read only"));
        }
        let uri = if let Some(uri) = self.document.virtual_uri() {
            uri.to_owned()
        } else {
            lsp::file_uri(
                self.document
                    .path()
                    .ok_or_else(|| error("Name a source file first"))?,
            )?
            .to_string()
        };
        let position = lsp::position(self.document.buffer.text(), self.document.buffer.cursor())?;
        let mut params = json!({"textDocument":{"uri":uri},"position":position});
        match name {
            "References" => params["context"] = json!({"includeDeclaration":true}),
            "Rename" => {
                if argument.is_empty() || argument.chars().any(char::is_control) {
                    return Err(error("Use :Rename NEW_NAME"));
                }
                params["newName"] = json!(argument);
            }
            "CodeAction" => {
                let range = if let Some(selection) = self.selection() {
                    lsp_types::Range::new(
                        lsp::position(self.document.buffer.text(), selection.chars.start)?,
                        lsp::position(self.document.buffer.text(), selection.chars.end)?,
                    )
                } else {
                    lsp_types::Range::new(position, position)
                };
                let diagnostics = self
                    .current_diagnostics()
                    .map(|d| {
                        d.items
                            .iter()
                            .filter(|d| d.range.start <= range.end && d.range.end >= range.start)
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                params["range"] = json!(range);
                params["context"] = json!({"diagnostics":diagnostics,"triggerKind":1});
                params.as_object_mut().unwrap().remove("position");
            }
            "Format" => {
                params["options"] = json!({"tabSize":4,"insertSpaces":false});
                params.as_object_mut().unwrap().remove("position");
            }
            "Symbols" | "Outline" => {
                params.as_object_mut().unwrap().remove("position");
            }
            "WorkspaceSymbols" => {
                if argument.len() > 4096 || argument.chars().any(char::is_control) {
                    return Err(error(
                        "Workspace symbol query must be at most 4096 bytes without controls",
                    ));
                }
                params = json!({"query":argument});
            }
            _ => {}
        }
        self.send_lsp(method, params, None)
    }

    fn send_lsp(
        &mut self,
        method: &str,
        params: Value,
        previous: Option<Context>,
    ) -> io::Result<()> {
        let mut context = if let Some(context) = previous {
            context
        } else {
            self.lsp_request_context()?
        };
        if !self.lsp_context_current(&context) {
            return Err(error(
                "LSP context is stale or server is not ready; :LspInfo",
            ));
        }
        let info = &self.lsp.servers[&context.key];
        if !lsp::supports_request(&info.capabilities, context.key.language, method, &params) {
            return Err(error(&format!("Server does not support {method}")));
        }
        self.lsp.serial += 1;
        context.serial = self.lsp.serial;
        let request = Request {
            serial: context.serial,
            stamp: context.stamp.clone(),
            path: context.path.clone(),
            method: method.into(),
            params,
        };
        self.lsp
            .worker
            .as_ref()
            .ok_or_else(|| error("LSP unavailable"))?
            .submit(Control::Request(request))?;
        if !context.inline_completion {
            self.info(&format!("LSP {method}…"));
        }
        self.lsp.context = Some(context);
        self.lsp.pending = true;
        self.lsp.started = Some(Instant::now());
        Ok(())
    }

    fn lsp_request_context(&self) -> io::Result<Context> {
        let path = lsp_identity(&self.document).ok_or_else(|| error("Name a source file first"))?;
        if let Some(message) = self.lsp.unavailable.get(&path) {
            return Err(error(message));
        }
        if self.document.virtual_uri().is_none() && Language::for_path(&path).is_none() {
            return Err(error(
                "No language server for this file type; supported languages: Kotlin, Java, C#",
            ));
        }
        let key = self
            .lsp
            .routes
            .get(&path)
            .ok_or_else(|| error("LSP discovery is pending; :LspInfo"))?
            .clone();
        let info = self
            .lsp
            .servers
            .get(&key)
            .ok_or_else(|| error("LSP is starting; :LspInfo"))?;
        if self
            .lsp
            .virtual_sources
            .get(&self.document.buffer.instance())
            .is_some_and(|source| source.epoch != info.epoch)
        {
            return Err(error(
                "This virtual source belongs to a previous server session; return to source and use gd again",
            ));
        }
        Ok(Context {
            serial: 0,
            stamp: Stamp {
                buffer_id: self.active_buffer_id,
                instance: self.document.buffer.instance(),
                generation: self.document.buffer.generation(),
            },
            path,
            cursor: self.document.buffer.cursor(),
            mode: self.mode,
            guards: self.lsp_edit_guards(),
            key,
            epoch: info.epoch,
            apply_resolved: false,
            inline_completion: false,
            jump_to_single_location: false,
            ai_question: None,
            call_item: None,
            virtual_target: None,
        })
    }

    fn lsp_response(
        &mut self,
        method: &str,
        result: Value,
        context: Context,
        versions: Vec<Versioned>,
    ) -> io::Result<()> {
        let mut panel = Panel::text(method.trim_start_matches("textDocument/"), "");
        match method {
            "csharp/metadata" => {
                return self.open_virtual_result(result, context);
            }
            "textDocument/hover" => {
                panel = Panel::text("Documentation", &markup(&result["contents"]))
            }
            "textDocument/signatureHelp" => {
                let active = result["activeSignature"].as_u64().unwrap_or(0) as usize;
                if let Some(items) = result["signatures"].as_array() {
                    for (index, item) in items.iter().take(lsp::MAX_ITEMS).enumerate() {
                        panel.rows.push(Row {
                            label: format!(
                                "{} {}",
                                if index == active { ">" } else { " " },
                                item["label"].as_str().unwrap_or("")
                            ),
                            detail: format!(
                                "parameter {}: {}",
                                result["activeParameter"]
                                    .as_u64()
                                    .unwrap_or(0)
                                    .saturating_add(1),
                                markup(&item["documentation"])
                            ),
                            action: Item::Text,
                        });
                    }
                }
            }
            "textDocument/completion" => {
                if context.inline_completion {
                    self.show_completion(result, context, versions);
                    return Ok(());
                }
                let items = result.as_array().or_else(|| result["items"].as_array());
                if result["isIncomplete"] == true {
                    panel
                        .title
                        .push_str(" (incomplete; request again after typing)");
                }
                if let Some(items) = items {
                    if items.len() > lsp::MAX_ITEMS {
                        panel.title.push_str(" (4096 result limit)");
                    }
                    let mut items: Vec<_> = items.iter().take(lsp::MAX_ITEMS).collect();
                    items.sort_by_key(|item| {
                        item["sortText"]
                            .as_str()
                            .or_else(|| item["label"].as_str())
                            .unwrap_or("")
                    });
                    for item in items {
                        panel.rows.push(completion_row(item.clone(), false));
                    }
                }
            }
            "completionItem/resolve" => {
                if context.apply_resolved {
                    return self.completion_edit(result, context, versions);
                }
                panel.rows.push(completion_row(result, true));
            }
            "textDocument/definition" | "textDocument/references" => {
                let items = result.as_array().cloned().unwrap_or_else(|| {
                    if result.is_null() {
                        Vec::new()
                    } else {
                        vec![result]
                    }
                });
                for item in items.iter().take(lsp::MAX_ITEMS) {
                    let uri = item["uri"]
                        .as_str()
                        .or_else(|| item["targetUri"].as_str())
                        .ok_or_else(|| error("Invalid LSP location"))?;
                    let position = item
                        .get("targetSelectionRange")
                        .or_else(|| item.get("range"))
                        .ok_or_else(|| error("Missing LSP location range"))?["start"]
                        .clone();
                    let position: Position =
                        serde_json::from_value(position).map_err(io::Error::other)?;
                    panel
                        .rows
                        .push(uri_location_row(uri, position, "", context.key.language));
                }
                if context.jump_to_single_location
                    && panel.rows.len() == 1
                    && let Item::Jump {
                        path,
                        position,
                        stamp,
                        ..
                    } = &panel.rows[0].action
                {
                    return self.jump_lsp_location(path, *position, stamp.as_ref());
                }
                if context.jump_to_single_location
                    && panel.rows.len() == 1
                    && let Item::VirtualJump { uri, position } = &panel.rows[0].action
                {
                    return self.request_virtual_location(uri.clone(), *position, context);
                }
            }
            "textDocument/documentSymbol" => {
                panel.title = "Symbol outline".into();
                let truncated = symbols(&result, &context.path, &mut panel.rows, 0)?;
                if truncated {
                    panel.title.push_str(" (outline truncated)");
                }
                if context.ai_question.is_some() {
                    if truncated {
                        return Err(error(
                            "Symbol outline is truncated; select code with v/V and Ctrl-g",
                        ));
                    }
                    let mut containing = Vec::new();
                    for (index, row) in panel.rows.iter().enumerate() {
                        if let Item::Jump {
                            path,
                            symbol_range: Some(range),
                            ..
                        } = &row.action
                            && path == &context.path
                        {
                            let range = lsp::scalar_range(self.document.buffer.text(), *range)?;
                            if range.contains(&context.cursor) {
                                containing.push((range.len(), index));
                            }
                        }
                    }
                    panel.selected = containing
                        .into_iter()
                        .min()
                        .map(|(_, index)| index)
                        .ok_or_else(|| {
                            error("No symbol contains the cursor; select code with v/V and Ctrl-g")
                        })?;
                }
            }
            "workspace/symbol" => {
                panel.title = format!(
                    "Workspace symbols · {}",
                    crate::git::display_path(&context.key.root)
                );
                for item in result_items(&result)? {
                    panel.rows.push(workspace_symbol_row(item.clone())?);
                }
                if result.as_array().is_some_and(|a| a.len() > lsp::MAX_ITEMS) {
                    panel.title.push_str(" (4096 result limit)");
                }
            }
            "workspaceSymbol/resolve" => {
                let row = workspace_symbol_row(result)?;
                if let Item::Jump { path, position, .. } = row.action {
                    return self.jump_lsp_location(&path, position, None);
                }
                return Err(error(
                    "Server did not resolve a local workspace symbol range",
                ));
            }
            "textDocument/prepareCallHierarchy" => {
                panel.title = "Call hierarchy · i incoming · o outgoing · Enter source".into();
                for item in result_items(&result)? {
                    panel.rows.push(call_row(item.clone(), "")?);
                }
                if result.as_array().is_some_and(|a| a.len() > lsp::MAX_ITEMS) {
                    panel.title.push_str(" (4096 result limit)");
                }
            }
            "callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls" => {
                let incoming = method == "callHierarchy/incomingCalls";
                let parent = context
                    .call_item
                    .as_ref()
                    .ok_or_else(|| error("Missing call hierarchy item"))?;
                panel.title = format!(
                    "{} · {} · Backspace parent",
                    if incoming {
                        "Incoming calls"
                    } else {
                        "Outgoing calls"
                    },
                    parent["name"].as_str().unwrap_or("symbol")
                );
                for call in result_items(&result)? {
                    let item = &call[if incoming { "from" } else { "to" }];
                    let ranges: Vec<lsp_types::Range> =
                        serde_json::from_value(call["fromRanges"].clone())
                            .map_err(io::Error::other)?;
                    if ranges.iter().any(|r| r.start > r.end) {
                        return Err(error("Invalid call site range"));
                    }
                    let sites = ranges
                        .iter()
                        .take(16)
                        .map(|r| {
                            format!(
                                "{}:{}",
                                r.start.line.saturating_add(1),
                                r.start.character.saturating_add(1)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let caller = if incoming { item } else { parent };
                    let detail = format!(
                        "{} call site(s) in {}: {}{}",
                        ranges.len(),
                        caller["uri"].as_str().unwrap_or(""),
                        sites,
                        if ranges.len() > 16 { "…" } else { "" }
                    );
                    panel.rows.push(call_row(item.clone(), &detail)?);
                }
                if result.as_array().is_some_and(|a| a.len() > lsp::MAX_ITEMS) {
                    panel.title.push_str(" (4096 result limit)");
                }
                if panel.rows.is_empty() {
                    panel.rows =
                        Panel::text("", "No calls reported; Backspace returns to the parent").rows;
                }
            }
            "textDocument/codeAction" => {
                if let Some(items) = result.as_array() {
                    for item in items.iter().take(lsp::MAX_ITEMS) {
                        panel.rows.push(Row {
                            label: item["title"].as_str().unwrap_or("Code action").into(),
                            detail: item["disabled"]["reason"].as_str().unwrap_or("").into(),
                            action: Item::Action(item.clone()),
                        });
                    }
                }
            }
            "codeAction/resolve" => return self.select_action(result, context, versions, true),
            "textDocument/rename" if result.is_null() => {
                self.info("No rename edits");
                return Ok(());
            }
            "textDocument/rename" => return self.stage_lsp_edit(&result, context, versions, None),
            "textDocument/formatting" => {
                if result.is_null() || result.as_array().is_some_and(Vec::is_empty) {
                    self.info("No formatting edits");
                    return Ok(());
                }
                let edit = json!({"changes":{lsp::file_uri(&context.path)?.as_str():result}});
                return self.stage_lsp_edit(&edit, context, versions, None);
            }
            "workspace/executeCommand" => {
                self.info(
                    "Code action completed; any returned edits require applying their preview",
                );
                return Ok(());
            }
            _ => return Err(error("Unsupported LSP response")),
        }
        if panel.rows.is_empty() {
            self.info(if method == "textDocument/definition" {
                "No language-server results; check indexing and project errors in :LspInfo / :LspLog"
            } else {
                "No language-server results"
            });
            return Ok(());
        }
        let ai_question = context.ai_question.clone();
        panel.return_mode = context.mode;
        panel.context = Some(context);
        panel.versions = versions;
        self.lsp.panel = Some(panel);
        self.view = View::Lsp;
        self.mode = Mode::Normal;
        if let Some(question) = ai_question {
            self.ai_explain(&question)?;
        }
        Ok(())
    }
    fn completion_edit(
        &mut self,
        value: Value,
        context: Context,
        versions: Vec<Versioned>,
    ) -> io::Result<()> {
        if value["insertTextFormat"] == 2 {
            return Err(error(
                "Server returned a snippet despite snippetSupport=false",
            ));
        }
        let command = value.get("command").filter(|v| !v.is_null()).cloned();
        let main = if let Some(edit) = value.get("textEdit") {
            let range = edit
                .get("range")
                .or_else(|| edit.get("replace"))
                .ok_or_else(|| error("Invalid completion edit"))?;
            json!({"range":range,"newText":edit["newText"]})
        } else {
            let start = self.completion_prefix_start(context.cursor);
            let text = self.document.buffer.text();
            json!({"range":lsp_types::Range::new(lsp::position(text,start)?,lsp::position(text,context.cursor)?),
                "newText":value["insertText"].as_str().or_else(||value["label"].as_str()).ok_or_else(||error("Completion has no text"))?})
        };
        let mut edits = vec![main];
        if let Some(extra) = value.get("additionalTextEdits").filter(|v| !v.is_null()) {
            edits.extend(
                extra
                    .as_array()
                    .ok_or_else(|| error("Invalid additional completion edits"))?
                    .iter()
                    .cloned(),
            );
        }
        let edit = json!({"changes":{lsp::file_uri(&context.path)?.as_str():edits}});
        if command.is_none()
            && edits.len() == 1
            && self.local_completion_edit(&edits[0], &context)?
        {
            let plan =
                self.prepare_lsp_edit(&edit, &context.key.root, &versions, &context.guards)?;
            self.view = View::Editing;
            self.mode = context.mode;
            self.lsp.cancel();
            self.apply_lsp_edit(plan)?;
            self.info("Completion inserted");
            Ok(())
        } else {
            let mut context = context;
            context.inline_completion = false;
            self.stage_lsp_edit(&edit, context, versions, command)
        }
    }
    fn select_action(
        &mut self,
        value: Value,
        mut context: Context,
        versions: Vec<Versioned>,
        resolved: bool,
    ) -> io::Result<()> {
        if let Some(reason) = value["disabled"]["reason"].as_str() {
            return Err(error(reason));
        }
        if value["edit"].is_object() {
            return self.stage_lsp_edit(
                &value["edit"],
                context,
                versions,
                value.get("command").filter(|v| v.is_object()).cloned(),
            );
        }
        let command = if value["command"].is_string() {
            value.clone()
        } else {
            value["command"].clone()
        };
        if command.is_object() {
            return self.send_lsp("workspace/executeCommand", command, Some(context));
        }
        if !resolved
            && lsp::supports(
                &self.lsp.servers[&context.key].capabilities,
                "codeAction/resolve",
            )
        {
            context.apply_resolved = true;
            return self.send_lsp("codeAction/resolve", value, Some(context));
        }
        Err(error("Code action has no supported edit or command"))
    }
    fn stage_lsp_edit(
        &mut self,
        edit: &Value,
        context: Context,
        versions: Vec<Versioned>,
        continuation: Option<Value>,
    ) -> io::Result<()> {
        if let Some(command) = &continuation {
            let advertised =
                &self.lsp.servers[&context.key].capabilities["executeCommandProvider"]["commands"];
            if !command["command"].is_string()
                || !advertised
                    .as_array()
                    .is_some_and(|items| items.contains(&command["command"]))
            {
                return Err(error(
                    "Server did not advertise this completion/action command",
                ));
            }
        }
        let plan = self.prepare_lsp_edit(edit, &context.key.root, &versions, &context.guards)?;
        let mut panel = Panel::text(
            &format!(
                "Apply {} edits in {} files? Enter applies; Esc cancels",
                plan.edit_count,
                plan.files.len()
            ),
            "",
        );
        for file in &plan.files {
            panel.rows.push(Row {
                label: file.path.display().to_string(),
                detail: format!(
                    "{} → {} bytes; unsaved after applying",
                    file.before.len_bytes(),
                    file.after.len_bytes()
                ),
                action: Item::Text,
            });
            for (range, text) in file.edits.iter().take(128) {
                panel.rows.push(Row {
                    label: format!(
                        "- {}",
                        preview_excerpt(&file.before.slice(range.clone()).to_string())
                    ),
                    detail: String::new(),
                    action: Item::Text,
                });
                panel.rows.push(Row {
                    label: format!("+ {}", preview_excerpt(text)),
                    detail: String::new(),
                    action: Item::Text,
                });
            }
            if file.edits.len() > 128 {
                panel.rows.push(Row {
                    label: "Additional edits omitted from preview (128 per file limit)".into(),
                    detail: String::new(),
                    action: Item::Text,
                });
            }
        }
        panel.return_mode = context.mode;
        if let Some(command) = &continuation {
            panel.rows.push(Row {
                label: format!(
                    "Then run server command: {}",
                    command["command"].as_str().unwrap_or("")
                ),
                detail: "Any further text edits require another preview".into(),
                action: Item::Text,
            });
        }
        self.lsp.context = Some(context.clone());
        panel.context = Some(context);
        panel.versions = versions;
        panel.plan = Some(plan);
        panel.continuation = continuation;
        self.lsp.panel = Some(panel);
        self.view = View::Lsp;
        self.mode = Mode::Normal;
        Ok(())
    }
    fn close_lsp_panel(&mut self) {
        let mode = self.lsp.panel.as_ref().map(|p| p.return_mode);
        self.lsp.cancel();
        if self.view == View::Lsp {
            self.view = View::Editing;
            self.mode = mode.unwrap_or(Mode::Normal);
            self.document.buffer.clamp_cursor(self.mode == Mode::Insert);
        }
    }
    pub(crate) fn lsp_key(&mut self, key: Key) {
        if matches!(key, Key::Escape | Key::Ctrl('c' | '[')) {
            self.close_lsp_panel();
            return;
        }
        if key == Key::Char(':') {
            self.mode = Mode::Command;
            self.command.clear();
            return;
        }
        if key == Key::Backspace
            && let Some(panel) = self.lsp.call_history.pop()
        {
            self.lsp.cancel_request();
            self.lsp.context = panel.context.clone();
            self.lsp.panel = Some(panel);
            self.info("Call hierarchy parent");
            return;
        }
        if matches!(key, Key::Char('i' | 'o')) {
            if let Err(e) = self.expand_calls(key == Key::Char('i')) {
                self.error(&e.to_string());
            }
            return;
        }
        if self.lsp.pending {
            return;
        }
        let Some(panel) = &mut self.lsp.panel else {
            self.view = View::Editing;
            return;
        };
        match key {
            Key::Char('j') | Key::Down | Key::Ctrl('n') => {
                panel.selected = (panel.selected + 1).min(panel.rows.len().saturating_sub(1))
            }
            Key::Char('k') | Key::Up | Key::Ctrl('p') => {
                panel.selected = panel.selected.saturating_sub(1)
            }
            Key::PageDown | Key::Ctrl('d') => {
                panel.selected = (panel.selected + 10).min(panel.rows.len().saturating_sub(1))
            }
            Key::PageUp | Key::Ctrl('u') => panel.selected = panel.selected.saturating_sub(10),
            Key::Char('g') => panel.selected = 0,
            Key::Char('G') => panel.selected = panel.rows.len().saturating_sub(1),
            Key::Char('h') | Key::Left => panel.left = panel.left.saturating_sub(4),
            Key::Char('l') | Key::Right => panel.left += 4,
            Key::Enter | Key::Char('d') | Key::Tab
                if key != Key::Tab
                    || (panel.plan.is_none()
                        && panel
                            .rows
                            .get(panel.selected)
                            .is_some_and(|row| matches!(row.action, Item::Completion { .. }))) =>
            {
                if let Err(e) = self.select_lsp(key == Key::Char('d')) {
                    if self.lsp.panel.is_none() {
                        self.close_lsp_panel();
                    }
                    self.error(&e.to_string());
                }
            }
            _ => {}
        }
    }
    fn select_lsp(&mut self, documentation: bool) -> io::Result<()> {
        let mut panel = self
            .lsp
            .panel
            .take()
            .ok_or_else(|| error("No language result"))?;
        if panel
            .context
            .as_ref()
            .is_some_and(|c| !self.lsp_context_current(c))
        {
            self.close_lsp_panel();
            return Err(error("Language result is stale; request it again"));
        }
        if panel.plan.is_some() && !documentation {
            self.mode = panel.return_mode;
            let result = self.apply_lsp_edit(panel.plan.take().unwrap());
            if let Some((key, epoch, id)) = panel.reply.take() {
                self.lsp
                    .worker
                    .as_ref()
                    .unwrap()
                    .submit(Control::ApplyReply {
                        key,
                        epoch,
                        id,
                        applied: result.is_ok(),
                        reason: result
                            .as_ref()
                            .err()
                            .map_or(String::new(), ToString::to_string),
                    })?;
            }
            self.view = View::Editing;
            self.mode = panel.return_mode;
            self.lsp.cancel();
            result?;
            if let Some(command) = panel.continuation {
                self.tick_lsp();
                self.send_lsp("workspace/executeCommand", command, None)?;
            }
            return Ok(());
        }
        let Some(row) = panel.rows.get_mut(panel.selected) else {
            self.lsp.panel = Some(panel);
            return Ok(());
        };
        let action = std::mem::replace(&mut row.action, Item::Text);
        let context = panel.context.clone();
        let versions = panel.versions.clone();
        match action {
            Item::VirtualJump { uri, position } => {
                row.action = Item::VirtualJump {
                    uri: uri.clone(),
                    position,
                };
                self.lsp.panel = Some(panel);
                self.request_virtual_location(
                    uri,
                    position,
                    context.ok_or_else(|| error("Missing navigation context"))?,
                )?;
            }
            Item::Jump {
                path,
                position,
                stamp,
                symbol_range,
            } => {
                if let Err(e) = self.jump_lsp_location(&path, position, stamp.as_ref()) {
                    row.action = Item::Jump {
                        path,
                        position,
                        stamp,
                        symbol_range,
                    };
                    self.lsp.panel = Some(panel);
                    return Err(e);
                }
            }
            Item::WorkspaceSymbol(value) => {
                row.action = Item::WorkspaceSymbol(value.clone());
                self.lsp.panel = Some(panel);
                self.send_lsp("workspaceSymbol/resolve", value, context)?;
            }
            Item::Call(value) => {
                let item: lsp_types::CallHierarchyItem =
                    serde_json::from_value(value).map_err(io::Error::other)?;
                self.jump_lsp_location(
                    &lsp::uri_path(item.uri.as_str())?,
                    item.selection_range.start,
                    None,
                )?;
            }
            Item::Completion { value, resolved } => {
                let mut context = context.ok_or_else(|| error("Missing completion context"))?;
                if !resolved
                    && lsp::supports(
                        &self.lsp.servers[&context.key].capabilities,
                        "completionItem/resolve",
                    )
                {
                    context.apply_resolved = !documentation;
                    self.lsp.panel = Some(panel);
                    return self.send_lsp("completionItem/resolve", value, Some(context));
                }
                if documentation {
                    row.action = Item::Completion { value, resolved };
                    self.lsp.panel = Some(panel);
                } else {
                    self.completion_edit(value, context, versions)?;
                }
            }
            Item::Action(value) => self.select_action(
                value,
                context.ok_or_else(|| error("Missing action context"))?,
                versions,
                false,
            )?,
            Item::Text => self.lsp.panel = Some(panel),
        }
        Ok(())
    }
    fn jump_lsp_location(
        &mut self,
        path: &Path,
        position: Position,
        stamp: Option<&Stamp>,
    ) -> io::Result<()> {
        // Outline rows for the active virtual document use its opaque identity.
        if self.document.virtual_uri().is_some()
            && lsp_identity(&self.document).as_deref() == Some(path)
        {
            let scalar = lsp::scalar(self.document.buffer.text(), position)?;
            let origin = self.source_location();
            self.reset_context();
            self.document.buffer.set_cursor(scalar, false);
            self.record_navigation(origin);
            self.info("Language-server location; Ctrl-o returns");
            return Ok(());
        }
        let path = if self.has_open_path(path) {
            path.to_path_buf()
        } else {
            crate::file::absolute_path(path)?
        };
        let current = self.lsp_edit_guards().get(&path).cloned();
        if stamp.is_some() && current.as_ref() != stamp {
            return Err(error("Diagnostic location is stale"));
        }
        let target = if current.is_some() {
            None
        } else {
            if !path.is_file() {
                return Err(error("Language-server target file is unavailable"));
            }
            Some(crate::file::Document::open(&path)?)
        };
        let document = if let Some(stamp) = &current {
            self.lsp_document(stamp.buffer_id).unwrap()
        } else {
            target.as_ref().unwrap()
        };
        let scalar = lsp::scalar(document.buffer.text(), position)?;
        let origin = self.source_location();
        if let Some(document) = target {
            self.open_prepared_buffer(document);
        } else {
            self.open_buffer_unrecorded(&path)?;
        }
        self.document.buffer.set_cursor(scalar, false);
        self.record_navigation(origin);
        self.info("Language-server location; Ctrl-o returns");
        Ok(())
    }

    fn request_virtual_location(
        &mut self,
        uri: String,
        position: Position,
        mut context: Context,
    ) -> io::Result<()> {
        if context.key.language != Language::CSharp || !lsp::is_csharp_metadata_uri(&uri) {
            return Err(error("Unsupported virtual source URI"));
        }
        if self.lsp.virtual_sources.len() >= 32
            && self
                .virtual_buffer(&uri, &context.key, context.epoch)
                .is_none()
        {
            return Err(error(
                "Virtual source limit (32); close a generated/decompiled buffer and retry",
            ));
        }
        context.virtual_target = Some((uri.clone(), position));
        self.send_lsp(
            "csharp/metadata",
            json!({"textDocument":{"uri":uri}}),
            Some(context),
        )
    }

    fn virtual_buffer(&self, uri: &str, key: &ServerKey, epoch: u64) -> Option<usize> {
        std::iter::once((self.active_buffer_id, &self.document))
            .chain(self.buffers.iter().map(|(id, slot)| (*id, &slot.document)))
            .find(|(_, doc)| {
                self.lsp
                    .virtual_sources
                    .get(&doc.buffer.instance())
                    .is_some_and(|source| {
                        source.uri == uri && &source.key == key && source.epoch == epoch
                    })
            })
            .map(|(id, _)| id)
    }

    fn open_virtual_result(&mut self, result: Value, context: Context) -> io::Result<()> {
        let (uri, position) = context
            .virtual_target
            .ok_or_else(|| error("Missing virtual source target"))?;
        let text = result["source"].as_str().filter(|s| !s.is_empty())
            .ok_or_else(|| error("Language server returned no source text for this generated/decompiled location"))?;
        if text.len() > lsp::MAX_DOCUMENT_BYTES {
            return Err(error("Virtual source exceeds 2 MiB"));
        }
        let document =
            crate::file::Document::virtual_source(uri.clone(), text, Path::new("source.cs"))?;
        // Validate before changing the active buffer or its navigation history.
        let scalar = lsp::scalar(document.buffer.text(), position)?;
        let source = lsp::VirtualSource {
            uri,
            key: context.key,
            epoch: context.epoch,
        };
        let existing = self.virtual_buffer(&source.uri, &source.key, source.epoch);
        let origin = self.source_location();
        if let Some(id) = existing {
            self.switch_buffer_unrecorded(id)?;
            // Ask the server on each jump: source generators can change their
            // output within one session. Reuse the slot, refreshing its snapshot.
            if self.document.buffer.text() != document.buffer.text() {
                self.lsp
                    .virtual_sources
                    .remove(&self.document.buffer.instance());
                self.replace_reloaded_document(document);
            }
        } else {
            self.open_prepared_buffer(document);
        }
        self.lsp
            .virtual_sources
            .insert(self.document.buffer.instance(), source.clone());
        let identity = lsp_identity(&self.document).unwrap();
        self.lsp.routes.insert(identity, source.key);
        self.document.buffer.set_cursor(scalar, false);
        self.record_navigation(origin);
        self.info("Read-only generated/decompiled source; Ctrl-o returns");
        Ok(())
    }
    fn expand_calls(&mut self, incoming: bool) -> io::Result<()> {
        if self.lsp.pending {
            return Err(error(
                "Wait for the call hierarchy response, or Escape cancels",
            ));
        }
        if self.lsp.call_history.len() >= 16 {
            return Err(error("Call hierarchy depth limit (16); Backspace returns"));
        }
        let panel = self
            .lsp
            .panel
            .as_ref()
            .ok_or_else(|| error("No call hierarchy open"))?;
        let Some(Row {
            action: Item::Call(value),
            ..
        }) = panel.rows.get(panel.selected)
        else {
            return Err(error(
                "Select a call hierarchy item; :Calls starts at the source cursor",
            ));
        };
        let value = value.clone();
        let mut context = panel
            .context
            .clone()
            .ok_or_else(|| error("Missing call context"))?;
        context.call_item = Some(value.clone());
        self.send_lsp(
            if incoming {
                "callHierarchy/incomingCalls"
            } else {
                "callHierarchy/outgoingCalls"
            },
            json!({"item":value}),
            Some(context),
        )?;
        let panel = self.lsp.panel.take().unwrap();
        let mut waiting = Panel::text(
            if incoming {
                "Incoming calls"
            } else {
                "Outgoing calls"
            },
            "Waiting for language server; Backspace returns, Escape closes",
        );
        waiting.return_mode = panel.return_mode;
        waiting.context = self.lsp.context.clone();
        self.lsp.call_history.push(panel);
        self.lsp.panel = Some(waiting);
        Ok(())
    }
    fn lsp_information(&mut self, kind: &str) {
        let path = lsp_identity(&self.document);
        let key = path.as_ref().and_then(|p| self.lsp.routes.get(p));
        let info = key.and_then(|k| self.lsp.servers.get(k));
        let mut text = format!(
            ":LspInfo status/progress | :LspLog server messages\n:Doctor executable, SDK and root checks | :LspRestart retry\nSource: {}\nLSP scope: Kotlin, Java, C#; UTF-16, 2 MiB/document\n",
            self.document.display_name()
        );
        text += &format!(
            "Automatic installation: {}\nManaged directory: {:?}\n",
            self.lsp.config.installation.auto_install, self.lsp.config.installation.directory
        );
        for (language, state) in &self.lsp.installations {
            text += &format!("Install {}: {}\n", language.id(), state.message);
        }
        if self.lsp.discovering {
            text += "Preparing: detecting project languages in background\n";
        }
        for (other, server) in &self.lsp.servers {
            if Some(other) != key {
                text += &format!(
                    "Prepared {}: {}\nRoot: {}\nSession: {}\n",
                    other.language.id(),
                    format_args!("{} - {}", server.status.summary(), server.status.message),
                    other.root.display(),
                    server.epoch
                );
                if key.is_none() {
                    for log in &server.logs {
                        text += log;
                        text.push('\n');
                    }
                }
            }
        }
        if let Some(key) = key {
            text += &format!(
                "Language: {}\nProject root: {}\nCommand override: {} (JSON argv, no shell)\n",
                key.language.id(),
                key.root.display(),
                key.language.environment()
            );
        }
        if let Some(info) = info {
            text += &format!(
                "Status: {}\n{}\nSession: {}\n",
                info.status.summary(),
                info.status.message,
                info.epoch
            );
            text += if info.capabilities.is_object() {
                "Initialization: complete (does not prove indexing finished)\n"
            } else {
                "Initialization: unavailable; language requests wait for capabilities\n"
            };
            for task in &info.status.progress {
                text += &format!("Active work: {task}\n");
            }
            if info.status.phase.busy() && info.status.since.elapsed().as_secs() >= 30 {
                text += "Still waiting. Gradle/Maven imports may download dependencies or need a project JDK.\nCheck :LspLog and :Doctor; editing remains available. No indexing percentage is estimated.\n";
            }
            if kind != "LspLog" {
                for method in [
                    "textDocument/completion",
                    "textDocument/hover",
                    "textDocument/signatureHelp",
                    "textDocument/definition",
                    "textDocument/references",
                    "textDocument/rename",
                    "textDocument/codeAction",
                    "textDocument/formatting",
                    "textDocument/documentSymbol",
                    "workspace/symbol",
                    "workspaceSymbol/resolve",
                    "textDocument/prepareCallHierarchy",
                    "textDocument/diagnostic",
                ] {
                    text += &format!(
                        "{}: {}\n",
                        method,
                        if lsp::supports(&info.capabilities, method) {
                            "supported"
                        } else {
                            "unavailable"
                        }
                    );
                }
            }
            text += "\nRecent server stderr/messages (bounded, session only):\n";
            for log in &info.logs {
                text += log;
                text.push('\n');
            }
        } else {
            text += "Discovery pending or unsupported file type. Language servers are optional.\n";
        }
        if let Some(message) = path.as_ref().and_then(|p| self.lsp.unavailable.get(p)) {
            text += message;
            text.push('\n');
        }
        if kind == "Doctor" {
            text += "\n:LspInstall [kotlin|java|csharp] installs a pinned server; :LspInstallCancel cancels.\n";
            text += "Kotlin includes Java. JDT LS requires Java 21+; csharp-ls requires .NET SDK 10+.\n";
            text += "Runtime and project SDKs are separate. JAVA_HOME/PATH select Java; dotnet/global.json select .NET.\n";
            text += "Server initialization/import is asynchronous (180s default). Requests time out after 15s.\n";
            text += "Gradle settings / nearest Maven pom / containing solution or project choose roots.\n";
            text += "JDT LS uses private temporary workspace data. Wrapper argv can select a runtime/solution.\n";
            text += "Use :LspRestart after changing SDK/server setup. Build imports may download dependencies.\n";
            text += "Check project Gradle/Maven output for missing toolchains, repositories, proxy/authentication or invalid build files.\n";
            text += "Versionless push diagnostics are labelled unverified; prefer versioned or pull diagnostics.\n";
        }
        self.close_lsp_panel();
        self.lsp.panel = Some(Panel::text(kind, &text));
        self.view = View::Lsp;
        self.mode = Mode::Normal;
    }
    pub fn current_diagnostics(&self) -> Option<&DiagnosticSet> {
        if self.note_edit.is_some() {
            return None;
        }
        self.lsp.diagnostics.get(self.document.path()?).filter(|d| {
            d.stamp.buffer_id == self.active_buffer_id
                && d.stamp.instance == self.document.buffer.instance()
                && d.stamp.generation == self.document.buffer.generation()
        })
    }
    pub fn lsp_summary(&self) -> String {
        if self.note_edit.is_some() {
            return String::new();
        }
        let path = self
            .document
            .path()
            .filter(|path| Language::for_path(path).is_some());
        let Some(path) = path else {
            if self.lsp.discovering {
                return "LSP detecting projects".into();
            }
            if let Some((language, state)) =
                self.lsp.installations.iter().find(|(_, state)| state.busy)
            {
                return format!("LSP {}: {}", language.id(), state.message);
            }
            return self
                .lsp
                .servers
                .iter()
                .find(|(_, info)| info.status.phase != lsp::Phase::Ready)
                .or_else(|| self.lsp.servers.iter().next())
                .map_or_else(String::new, |(key, info)| {
                    format!(
                        "LSP {} :LspInfo ({})",
                        info.status.summary(),
                        key.language.id()
                    )
                });
        };
        if let Some(state) =
            Language::for_path(path).and_then(|language| self.lsp.installations.get(&language))
        {
            if state.busy {
                return format!("LSP {}", state.message);
            }
            if state.failed
                && !self
                    .lsp
                    .routes
                    .get(path)
                    .and_then(|key| self.lsp.servers.get(key))
                    .is_some_and(|info| info.capabilities.is_object())
            {
                return "LSP install failed (:LspInfo)".into();
            }
        }
        if self.document.buffer.text().len_bytes() > lsp::MAX_DOCUMENT_BYTES {
            return "LSP >2MiB".into();
        }
        let server = self
            .lsp
            .routes
            .get(path)
            .and_then(|key| self.lsp.servers.get(key));
        // Startup/failure remains visible even while requests or diagnostics exist.
        if let Some(server) = server
            && server.status.phase != lsp::Phase::Ready
        {
            return format!("LSP {} :LspInfo", server.status.summary());
        }
        if self.lsp.pending {
            return "LSP Ready (request pending)".into();
        }
        if let Some(d) = self.current_diagnostics()
            && !d.items.is_empty()
        {
            return format!(
                "LSP Ready {}diag{}",
                d.items.len(),
                if d.versioned { "" } else { "?" }
            );
        }
        if self.lsp.unavailable.contains_key(path) {
            return "LSP Failed :LspInfo".into();
        }
        server.map_or_else(
            || "LSP Starting :LspInfo".into(),
            |s| format!("LSP {} :LspInfo", s.status.summary()),
        )
    }
    fn open_diagnostics(&mut self) {
        self.close_lsp_panel();
        let mut panel = Panel::text("Diagnostics — ? means unversioned/unverified", "");
        for (path, set) in &self.lsp.diagnostics {
            for diagnostic in &set.items {
                panel.rows.push(location_row(
                    path.clone(),
                    diagnostic.range.start,
                    &format!(
                        "{}{}",
                        if set.versioned { "" } else { "? " },
                        diagnostic.message
                    ),
                    Some(set.stamp.clone()),
                ));
            }
        }
        if panel.rows.is_empty() {
            self.info("No current diagnostics");
            return;
        }
        panel.rows.truncate(lsp::MAX_ITEMS);
        self.lsp.panel = Some(panel);
        self.view = View::Lsp;
        self.mode = Mode::Normal;
    }
    fn diagnostic_step(&mut self, backward: bool) -> io::Result<()> {
        let set = self
            .current_diagnostics()
            .ok_or_else(|| error("No current diagnostics"))?;
        let text = self.document.buffer.text();
        let mut positions: Vec<_> = set
            .items
            .iter()
            .filter_map(|d| lsp::scalar(text, d.range.start).ok())
            .collect();
        positions.sort_unstable();
        positions.dedup();
        let cursor = self.document.buffer.cursor();
        let target = if backward {
            positions
                .iter()
                .rev()
                .find(|p| **p < cursor)
                .or(positions.last())
        } else {
            positions
                .iter()
                .find(|p| **p > cursor)
                .or(positions.first())
        };
        let target = *target.ok_or_else(|| error("No navigable diagnostics"))?;
        let origin = self.source_location();
        self.document.buffer.set_cursor(target, false);
        self.record_navigation(origin);
        Ok(())
    }
}

fn diagnostic_dependencies_current(
    dependencies: &Option<BTreeMap<PathBuf, Stamp>>,
    key: &ServerKey,
    snapshots: &[Snapshot],
    routes: &BTreeMap<PathBuf, ServerKey>,
) -> bool {
    let Some(dependencies) = dependencies else {
        return true;
    };
    let mut current = snapshots
        .iter()
        .filter(|s| s.virtual_source.is_none())
        .filter(|s| routes.get(&s.path) == Some(key));
    current.clone().count() == dependencies.len()
        && current.all(|s| dependencies.get(&s.path) == Some(&s.stamp))
}

fn error(message: &str) -> io::Error {
    io::Error::other(message)
}
fn preview_excerpt(text: &str) -> String {
    let mut chars = text.chars();
    let mut excerpt: String = chars.by_ref().take(512).collect();
    if chars.next().is_some() {
        excerpt.push_str("… [truncated]");
    }
    excerpt
}
fn markup(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.chars().take(32 * 1024).collect()
    } else if let Some(items) = value.as_array() {
        items
            .iter()
            .take(64)
            .map(markup)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        value["value"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(32 * 1024)
            .collect()
    }
}
fn completion_row(value: Value, resolved: bool) -> Row {
    Row {
        label: value["label"].as_str().unwrap_or("Completion").into(),
        detail: format!(
            "{}\n{}",
            value["detail"].as_str().unwrap_or(""),
            markup(&value["documentation"])
        ),
        action: Item::Completion { value, resolved },
    }
}
fn location_row(path: PathBuf, position: Position, label: &str, stamp: Option<Stamp>) -> Row {
    Row {
        label: format!(
            "{}:{}:{} {label}",
            path.display(),
            position.line.saturating_add(1),
            position.character.saturating_add(1)
        ),
        detail: label.into(),
        action: Item::Jump {
            path,
            position,
            stamp,
            symbol_range: None,
        },
    }
}

/// Virtual identities are relative tokens, never filesystem paths. Local source
/// identities are absolute paths; virtual sources cannot collide with them.
fn lsp_identity(document: &crate::file::Document) -> Option<PathBuf> {
    document.path().map(Path::to_path_buf).or_else(|| {
        document
            .virtual_uri()
            .map(|_| PathBuf::from(format!("lsp-virtual:{}", document.buffer.instance())))
    })
}

fn uri_location_row(uri: &str, position: Position, label: &str, language: Language) -> Row {
    if let Ok(path) = lsp::uri_path(uri) {
        return location_row(path, position, label, None);
    }
    if language == Language::CSharp && lsp::is_csharp_metadata_uri(uri) {
        return Row {
            label: format!(
                "{uri}:{}:{} {label}",
                position.line.saturating_add(1),
                position.character.saturating_add(1)
            ),
            detail: "Generated/decompiled source · Enter opens read only".into(),
            action: Item::VirtualJump {
                uri: uri.into(),
                position,
            },
        };
    }
    Row {
        label: format!("Unsupported location: {uri}"),
        detail: "This server's virtual/decompiled source format is not supported".into(),
        action: Item::Text,
    }
}
fn result_items(value: &Value) -> io::Result<impl Iterator<Item = &Value>> {
    let items = if value.is_null() {
        &[][..]
    } else {
        value
            .as_array()
            .ok_or_else(|| error("Invalid language-server result list"))?
            .as_slice()
    };
    Ok(items.iter().take(lsp::MAX_ITEMS))
}
fn workspace_symbol_row(value: Value) -> io::Result<Row> {
    let name = value["name"]
        .as_str()
        .ok_or_else(|| error("Missing workspace symbol name"))?;
    let uri = value["location"]["uri"]
        .as_str()
        .ok_or_else(|| error("Missing workspace symbol URI"))?;
    let label = format!("{} {}", name, value["containerName"].as_str().unwrap_or(""));
    let Ok(path) = lsp::uri_path(uri) else {
        return Ok(Row {
            label,
            detail: format!("Unsupported virtual/decompiled location: {uri}"),
            action: Item::Text,
        });
    };
    if let Some(range) = value["location"].get("range") {
        let range: lsp_types::Range =
            serde_json::from_value(range.clone()).map_err(io::Error::other)?;
        if range.start > range.end {
            return Err(error("Invalid symbol range"));
        }
        let mut row = location_row(path, range.start, &label, None);
        row.detail = row.label;
        row.label = label;
        Ok(row)
    } else {
        Ok(Row {
            label,
            detail: format!(
                "{} · Enter resolves location",
                crate::git::display_path(&path)
            ),
            action: Item::WorkspaceSymbol(value),
        })
    }
}
fn call_row(value: Value, sites: &str) -> io::Result<Row> {
    let item: lsp_types::CallHierarchyItem =
        serde_json::from_value(value.clone()).map_err(io::Error::other)?;
    if item.range.start > item.selection_range.start
        || item.selection_range.start > item.selection_range.end
        || item.selection_range.end > item.range.end
    {
        return Err(error("Invalid call hierarchy selection range"));
    }
    let detail = format!(
        "{}:{}:{} {}\n{}",
        item.uri,
        item.selection_range.start.line.saturating_add(1),
        item.selection_range.start.character.saturating_add(1),
        item.detail.as_deref().unwrap_or(""),
        sites
    );
    if lsp::uri_path(item.uri.as_str()).is_err() {
        return Ok(Row {
            label: item.name,
            detail: format!("Unsupported virtual/decompiled location: {}", item.uri),
            action: Item::Text,
        });
    }
    Ok(Row {
        label: item.name,
        detail,
        action: Item::Call(value),
    })
}
fn symbols(value: &Value, path: &Path, rows: &mut Vec<Row>, depth: usize) -> io::Result<bool> {
    if depth > 32 {
        return Ok(value.as_array().is_some_and(|items| !items.is_empty()));
    }
    let mut truncated = false;
    if let Some(items) = value.as_array() {
        for item in items {
            if rows.len() >= lsp::MAX_ITEMS {
                return Ok(true);
            }
            let location = item.get("location");
            let target = if let Some(location) = location {
                lsp::uri_path(
                    location["uri"]
                        .as_str()
                        .ok_or_else(|| error("Invalid symbol URI"))?,
                )?
            } else {
                path.to_path_buf()
            };
            let position = location
                .map(|l| &l["range"]["start"])
                .unwrap_or(&item["selectionRange"]["start"]);
            let position: Position =
                serde_json::from_value(position.clone()).map_err(io::Error::other)?;
            let mut row = location_row(
                target,
                position,
                &format!(
                    "{}{}",
                    "  ".repeat(depth),
                    item["name"].as_str().unwrap_or("")
                ),
                None,
            );
            row.detail = row.label;
            if let Item::Jump { symbol_range, .. } = &mut row.action {
                *symbol_range = Some(
                    serde_json::from_value(
                        location.map_or(&item["range"], |l| &l["range"]).clone(),
                    )
                    .map_err(io::Error::other)?,
                );
            }
            row.label = format!(
                "{}{}",
                "  ".repeat(depth),
                item["name"].as_str().unwrap_or("")
            );
            rows.push(row);
            truncated |= symbols(&item["children"], path, rows, depth + 1)?;
        }
    }
    Ok(truncated)
}

pub fn render(frame: &mut ratatui::Frame<'_>, editor: &mut Editor, theme: &crate::theme::Theme) {
    use ratatui::{layout::Rect, style::Style, widgets::Paragraph};
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(panel) = &mut editor.lsp.panel else {
        return;
    };
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let width = area.width as usize;
    let line = |text: &str, left| crate::text::visible_text(text, left, width);
    frame.render_widget(
        Paragraph::new(line(&panel.title, 0)).style(style.fg(theme.command)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let detail_rows = if area.height >= 8 { 3 } else { 0 };
    let height = area.height.saturating_sub(2 + detail_rows);
    panel.top = panel.top.min(panel.selected);
    if height > 0 && panel.selected >= panel.top + height as usize {
        panel.top = panel.selected + 1 - height as usize;
    }
    for row in 0..height {
        if let Some(item) = panel.rows.get(panel.top + row as usize) {
            let selected = panel.top + row as usize == panel.selected;
            frame.render_widget(
                Paragraph::new(line(&item.label, panel.left)).style(if selected {
                    style.bg(theme.selection)
                } else {
                    style
                }),
                Rect::new(area.x, area.y + 1 + row, area.width, 1),
            );
        }
    }
    if let Some(item) = panel.rows.get(panel.selected) {
        for (index, text) in item.detail.lines().take(detail_rows as usize).enumerate() {
            frame.render_widget(
                Paragraph::new(line(text, panel.left)).style(style.fg(theme.muted)),
                Rect::new(area.x, area.y + 1 + height + index as u16, area.width, 1),
            );
        }
    }
    if area.height >= 2 {
        let command = format!(":{}", editor.command);
        let text = if editor.mode == Mode::Command {
            &command
        } else if editor.message_is_error {
            &editor.message
        } else if panel.rows.iter().any(|r| matches!(r.action, Item::Call(_))) {
            "j/k select  i incoming  o outgoing  Enter source  Backspace parent  Esc return"
        } else if panel
            .rows
            .iter()
            .any(|r| matches!(r.action, Item::Completion { .. }))
        {
            "↑↓ select  Enter/Tab accept  d documentation  Esc return"
        } else {
            "j/k select  h/l scroll  Enter open/apply  d completion docs  Esc return"
        };
        let cells = crate::text::display_column(text, text.chars().count());
        let offset = if editor.mode == Mode::Command {
            cells.saturating_sub(width.saturating_sub(1))
        } else {
            0
        };
        frame.render_widget(
            Paragraph::new(line(text, offset)).style(style.fg(if editor.message_is_error {
                theme.error
            } else {
                theme.muted
            })),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
        if editor.mode == Mode::Command {
            frame.set_cursor_position((
                area.x + (cells - offset).min(width - 1) as u16,
                area.y + area.height - 1,
            ));
        }
    }
}
