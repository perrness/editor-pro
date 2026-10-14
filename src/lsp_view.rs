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
    collections::{BTreeMap, VecDeque},
    io,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Default)]
pub struct LspState {
    pub config: Config,
    worker: Option<Worker>,
    snapshots: Vec<Snapshot>,
    pub routes: BTreeMap<PathBuf, ServerKey>,
    pub unavailable: BTreeMap<PathBuf, String>,
    pub servers: BTreeMap<ServerKey, ServerInfo>,
    pub diagnostics: BTreeMap<PathBuf, DiagnosticSet>,
    pub panel: Option<Panel>,
    context: Option<Context>,
    serial: u64,
    pub pending: bool,
    started: Option<Instant>,
}
#[derive(Default)]
pub struct ServerInfo {
    pub epoch: u64,
    pub status: String,
    pub capabilities: Value,
    pub logs: VecDeque<String>,
}
pub struct DiagnosticSet {
    pub stamp: Stamp,
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
    Jump {
        path: PathBuf,
        position: Position,
        stamp: Option<Stamp>,
    },
    Completion {
        value: Value,
        resolved: bool,
    },
    Action(Value),
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
    pub fn cancel(&mut self) {
        if let Some(context) = &self.context
            && let Some(worker) = &self.worker
        {
            let _ = worker.submit(Control::Cancel(context.path.clone()));
        }
        self.serial += 1;
        self.context = None;
        self.pending = false;
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
    fn lsp_snapshots(&self) -> Vec<Snapshot> {
        let active = self
            .note_edit
            .as_ref()
            .map_or(&self.document, |note| &note.source);
        std::iter::once((self.active_buffer_id, active))
            .chain(self.buffers.iter().map(|(id, slot)| (*id, &slot.document)))
            .filter_map(|(id, doc)| {
                let path = doc.path()?;
                let language = Language::for_path(path)?;
                if doc.buffer.text().len_bytes() > lsp::MAX_DOCUMENT_BYTES {
                    return None;
                }
                Some(Snapshot {
                    stamp: Stamp {
                        buffer_id: id,
                        instance: doc.buffer.instance(),
                        generation: doc.buffer.generation(),
                    },
                    path: path.into(),
                    text: doc.buffer.text().clone(),
                    saved: doc.save_generation(),
                    language,
                })
            })
            .collect()
    }
    fn lsp_context_current(&self, context: &Context) -> bool {
        self.note_edit.is_none()
            && self.active_buffer_id == context.stamp.buffer_id
            && self.document.buffer.instance() == context.stamp.instance
            && self.document.buffer.generation() == context.stamp.generation
            && self.document.path() == Some(context.path.as_path())
            && self.document.buffer.cursor() == context.cursor
            && self.lsp_edit_guards() == context.guards
            && self
                .lsp
                .servers
                .get(&context.key)
                .is_some_and(|s| s.epoch == context.epoch && s.capabilities.is_object())
            && matches!(self.view, View::Editing | View::Lsp)
    }
    pub(crate) fn tick_lsp(&mut self) -> bool {
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
                self.lsp.servers.clear();
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
        self.lsp
            .diagnostics
            .retain(|path, d| stamps.get(path) == Some(&d.stamp));
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
        if self.lsp.servers.len() > 16 {
            self.lsp
                .servers
                .retain(|key, _| self.lsp.routes.values().any(|r| r == key));
        }
        if self.lsp.pending
            && self.lsp.started.is_some_and(|t| {
                t.elapsed() > self.lsp.config.request_timeout + std::time::Duration::from_secs(3)
            })
        {
            self.lsp.pending = false;
            self.lsp.context = None;
            self.error("LSP request expired; :LspInfo or :LspRestart");
            redraw = true;
        }
        redraw
    }
    fn lsp_event(&mut self, event: Event) -> io::Result<()> {
        match event {
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
                message,
                capabilities,
            } => {
                if !self.lsp.routes.values().any(|k| k == &key) {
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
                info.status = message;
                if let Some(capabilities) = capabilities {
                    info.capabilities = capabilities;
                }
            }
            Event::Log { key, message } => {
                if !self.lsp.routes.values().any(|k| k == &key) {
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
                versioned,
                items,
            } => {
                if self.lsp.servers.get(&key).is_some_and(|s| s.epoch == epoch)
                    && self.lsp.snapshots.iter().any(|s| {
                        s.path == document.snapshot.path && s.stamp == document.snapshot.stamp
                    })
                {
                    self.lsp.diagnostics.insert(
                        document.snapshot.path,
                        DiagnosticSet {
                            stamp: document.snapshot.stamp,
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

    pub fn lsp_command(&mut self, name: &str, argument: &str) -> io::Result<()> {
        if self.note_edit.is_some() {
            return Err(error("Language commands are unavailable in a review note"));
        }
        if !matches!(self.view, View::Editing | View::Lsp) {
            return Err(error("Return to source before using language commands"));
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
                let path = self
                    .document
                    .path()
                    .ok_or_else(|| error("Name a source file first"))?
                    .to_path_buf();
                self.close_lsp_panel();
                self.lsp.diagnostics.clear();
                if let Some(key) = self.lsp.routes.get(&path)
                    && let Some(info) = self.lsp.servers.get_mut(key)
                {
                    info.capabilities = Value::Null;
                    info.status = "Restarting".into();
                }
                self.lsp
                    .worker
                    .as_ref()
                    .ok_or_else(|| error("No supported language server"))?
                    .submit(Control::Restart(path))?;
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
            "Symbols" => "textDocument/documentSymbol",
            _ => return Err(error("Unknown language command")),
        };
        if self.document.buffer.text().len_bytes() > lsp::MAX_DOCUMENT_BYTES {
            return Err(error("LSP documents are limited to 2 MiB"));
        }
        let uri = lsp::file_uri(
            self.document
                .path()
                .ok_or_else(|| error("Name a source file first"))?,
        )?;
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
            "Symbols" => {
                params.as_object_mut().unwrap().remove("position");
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
            let path = self
                .document
                .path()
                .ok_or_else(|| error("Name a source file first"))?
                .to_path_buf();
            if let Some(message) = self.lsp.unavailable.get(&path) {
                return Err(error(message));
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
            Context {
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
            }
        };
        if !self.lsp_context_current(&context) {
            return Err(error(
                "LSP context is stale or server is not ready; :LspInfo",
            ));
        }
        let info = &self.lsp.servers[&context.key];
        if !lsp::supports(&info.capabilities, method) {
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
        self.lsp.context = Some(context);
        self.lsp.pending = true;
        self.lsp.started = Some(Instant::now());
        self.info(&format!("LSP {method}…"));
        Ok(())
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
                    match lsp::uri_path(uri) {
                        Ok(path) => panel.rows.push(location_row(path, position, "", None)),
                        Err(_) => panel.rows.push(Row {
                            label: format!("Unsupported location: {uri}"),
                            detail: "Virtual/decompiled documents are not supported".into(),
                            action: Item::Text,
                        }),
                    }
                }
            }
            "textDocument/documentSymbol" => symbols(&result, &context.path, &mut panel.rows, 0)?,
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
            self.info("No language-server results");
            return Ok(());
        }
        panel.return_mode = context.mode;
        panel.context = Some(context);
        panel.versions = versions;
        self.lsp.panel = Some(panel);
        self.view = View::Lsp;
        self.mode = Mode::Normal;
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
        if value.get("command").is_some_and(|v| !v.is_null()) {
            return Err(error(
                "Completion commands are unsupported; choose a plain completion",
            ));
        }
        let main = if let Some(edit) = value.get("textEdit") {
            let range = edit
                .get("range")
                .or_else(|| edit.get("replace"))
                .ok_or_else(|| error("Invalid completion edit"))?;
            json!({"range":range,"newText":edit["newText"]})
        } else {
            let mut start = context.cursor;
            let text = self.document.buffer.text();
            while start > 0 {
                let previous = self.document.buffer.grapheme_start_before(start);
                if !(text.char(previous).is_alphanumeric() || text.char(previous) == '_') {
                    break;
                }
                start = previous;
            }
            json!({"range":lsp_types::Range::new(lsp::position(text,start)?,lsp::position(text,context.cursor)?),
                "newText":value["insertText"].as_str().or_else(||value["label"].as_str()).ok_or_else(||error("Completion has no text"))?})
        };
        let mut edits = vec![main];
        if let Some(extra) = value["additionalTextEdits"].as_array() {
            edits.extend(extra.iter().cloned());
        }
        let edit = json!({"changes":{lsp::file_uri(&context.path)?.as_str():edits}});
        self.stage_lsp_edit(&edit, context, versions, None)
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
            Key::Enter | Key::Char('d') => {
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
            Item::Jump {
                path,
                position,
                stamp,
            } => {
                if let Some(stamp) = stamp
                    && self.lsp_edit_guards().get(&path) != Some(&stamp)
                {
                    self.lsp.panel = Some(panel);
                    return Err(error("Diagnostic location is stale"));
                }
                // Validate before activating a target, so malformed ranges cannot move source.
                let current = self.lsp_edit_guards().get(&path).cloned();
                let target = if current.is_some() {
                    None
                } else {
                    Some(crate::file::Document::open(&path)?)
                };
                let document = if let Some(stamp) = &current {
                    self.lsp_document(stamp.buffer_id).unwrap()
                } else {
                    target.as_ref().unwrap()
                };
                let scalar = lsp::scalar(document.buffer.text(), position)?;
                self.open_buffer(&path)?;
                self.document.buffer.set_cursor(scalar, false);
                self.info("Language-server location");
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
    fn lsp_information(&mut self, kind: &str) {
        let path = self.document.path();
        let key = path.and_then(|p| self.lsp.routes.get(p));
        let info = key.and_then(|k| self.lsp.servers.get(k));
        let mut text = format!(
            "Source: {}\nLSP scope: Kotlin, Java, C#; UTF-16, 2 MiB/document\n",
            self.document.display_name()
        );
        if let Some(key) = key {
            text += &format!(
                "Language: {}\nProject root: {}\nCommand override: {} (JSON argv, no shell)\n",
                key.language.id(),
                key.root.display(),
                key.language.environment()
            );
        }
        if let Some(info) = info {
            text += &format!("Status: {}\nSession: {}\n", info.status, info.epoch);
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
        if let Some(message) = path.and_then(|p| self.lsp.unavailable.get(p)) {
            text += message;
            text.push('\n');
        }
        if kind == "Doctor" {
            text += "\nInstall the standalone official Kotlin LSP, JDT LS jdtls launcher, or Roslyn-based csharp-ls.\n";
            text += "Runtime and project SDKs are separate. JAVA_HOME/PATH select Java; dotnet/global.json select .NET.\n";
            text += "Server initialization/import is asynchronous (180s default). Requests time out after 15s.\n";
            text += "Gradle settings / nearest Maven pom / containing solution or project choose roots.\n";
            text += "JDT LS uses private temporary workspace data. Wrapper argv can select a runtime/solution.\n";
            text += "Use :LspRestart after changing SDK/server setup. Build imports may download dependencies.\n";
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
        let Some(path) = self.document.path() else {
            return String::new();
        };
        if Language::for_path(path).is_none() || self.note_edit.is_some() {
            return String::new();
        }
        if self.document.buffer.text().len_bytes() > lsp::MAX_DOCUMENT_BYTES {
            return "LSP >2MiB".into();
        }
        if self.lsp.pending {
            return "LSP waiting".into();
        }
        if let Some(d) = self.current_diagnostics()
            && !d.items.is_empty()
        {
            return format!(
                "LSP {}diag{}",
                d.items.len(),
                if d.versioned { "" } else { "?" }
            );
        }
        if self.lsp.unavailable.contains_key(path) {
            return "LSP unavailable".into();
        }
        self.lsp
            .routes
            .get(path)
            .and_then(|k| self.lsp.servers.get(k))
            .map_or_else(
                || "LSP starting".into(),
                |s| {
                    if s.capabilities.is_object() {
                        format!("LSP {}", s.status)
                    } else {
                        "LSP unavailable (:LspInfo)".into()
                    }
                },
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
        self.document.buffer.set_cursor(target, false);
        Ok(())
    }
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
        },
    }
}
fn symbols(value: &Value, path: &Path, rows: &mut Vec<Row>, depth: usize) -> io::Result<()> {
    if depth > 32 {
        return Ok(());
    }
    if let Some(items) = value.as_array() {
        for item in items {
            if rows.len() >= lsp::MAX_ITEMS {
                break;
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
            rows.push(location_row(
                target,
                position,
                &format!(
                    "{}{}",
                    "  ".repeat(depth),
                    item["name"].as_str().unwrap_or("")
                ),
                None,
            ));
            symbols(&item["children"], path, rows, depth + 1)?;
        }
    }
    Ok(())
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
