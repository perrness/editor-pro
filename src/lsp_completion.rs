//! Debounced Insert-mode completion, using the ordinary LSP freshness/edit guards.
use super::*;
use crate::{text::visible_text, theme::Theme};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, Clear, Paragraph},
};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

const COMPLETION_DELAY: Duration = Duration::from_millis(120);

pub struct CompletionMenu {
    pub items: Vec<Value>,
    pub selected: usize,
    pub incomplete: bool,
    context: Context,
    versions: Vec<Versioned>,
}

pub(super) struct Scheduled {
    stamp: Stamp,
    path: PathBuf,
    cursor: usize,
    since: Instant,
    typed: Option<char>,
    incomplete: bool,
}

impl LspState {
    pub fn completion_scheduled(&self) -> bool {
        self.completion_due.is_some()
    }

    pub(crate) fn completion_incomplete(&self) -> bool {
        self.completion.as_ref().is_some_and(|menu| menu.incomplete)
            || self
                .completion_due
                .as_ref()
                .is_some_and(|due| due.incomplete)
    }
}

impl Editor {
    pub(crate) fn dismiss_completion(&mut self) {
        self.lsp.completion_due = None;
        self.lsp.completion = None;
        if self
            .lsp
            .context
            .as_ref()
            .is_some_and(|c| c.inline_completion || c.completion_command.is_some())
        {
            self.lsp.cancel_request();
        }
    }

    pub(crate) fn schedule_completion(&mut self, key: Key, incomplete: bool) {
        let Some(path) = self.document.path() else {
            return;
        };
        if self.view != View::Editing
            || self.mode != Mode::Insert
            || self.note_edit.is_some()
            || Language::for_path(path).is_none()
            || self.document.buffer.text().len_bytes() > lsp::MAX_DOCUMENT_BYTES
        {
            return;
        }
        let due = Scheduled {
            stamp: Stamp {
                buffer_id: self.active_buffer_id,
                instance: self.document.buffer.instance(),
                generation: self.document.buffer.generation(),
            },
            path: path.into(),
            cursor: self.document.buffer.cursor(),
            since: Instant::now(),
            typed: match key {
                Key::Char(ch) => Some(ch),
                _ => None,
            },
            incomplete,
        };
        // A source edit also invalidates any explicit request still in flight.
        self.lsp.cancel();
        self.lsp.completion_due = Some(due);
    }

    pub(super) fn tick_completion(&mut self) {
        let Some(due) = &self.lsp.completion_due else {
            return;
        };
        if self.view != View::Editing
            || self.mode != Mode::Insert
            || self.note_edit.is_some()
            || self.active_buffer_id != due.stamp.buffer_id
            || self.document.buffer.instance() != due.stamp.instance
            || self.document.buffer.generation() != due.stamp.generation
            || self.document.path() != Some(due.path.as_path())
            || self.document.buffer.cursor() != due.cursor
            || self.lsp.unavailable.contains_key(&due.path)
        {
            self.dismiss_completion();
            return;
        }
        if due.since.elapsed() < COMPLETION_DELAY || self.lsp.pending {
            return;
        }
        // Keep the latest typing intent while discovery/initialization finishes.
        let Ok(mut context) = self.lsp_request_context() else {
            return;
        };
        let capabilities = &self.lsp.servers[&context.key].capabilities;
        if !capabilities.is_object() {
            return;
        }
        if !lsp::supports(capabilities, "textDocument/completion") {
            self.lsp.completion_due = None;
            return;
        }
        let triggers = capabilities["completionProvider"]["triggerCharacters"].as_array();
        let is_trigger = |ch: char| {
            triggers.is_some_and(|items| items.iter().any(|v| v.as_str() == Some(&ch.to_string())))
        };
        let cursor = self.document.buffer.cursor();
        let trigger = due.typed.filter(|ch| is_trigger(*ch));
        let prefix = self.completion_prefix_start(cursor);
        let after_trigger = cursor > 0 && is_trigger(self.document.buffer.text().char(cursor - 1));
        if prefix == cursor && trigger.is_none() && !after_trigger {
            self.lsp.completion_due = None;
            return;
        }
        let completion_context = if let Some(ch) = trigger {
            json!({"triggerKind":2,"triggerCharacter":ch.to_string()})
        } else {
            json!({"triggerKind":if due.incomplete { 3 } else { 1 }})
        };
        self.lsp.completion_due = None;
        context.inline_completion = true;
        let request = (|| -> io::Result<()> {
            let params = json!({
                "textDocument":{"uri":lsp::file_uri(&context.path)?},
                "position":lsp::position(self.document.buffer.text(), cursor)?,
                "context":completion_context,
            });
            self.send_lsp("textDocument/completion", params, Some(context))
        })();
        // Automatic requests must not replace typing with repeated startup errors.
        if request.is_err() {
            self.dismiss_completion();
        }
    }

    pub(super) fn completion_prefix_start(&self, cursor: usize) -> usize {
        let text = self.document.buffer.text();
        let line_start = text.line_to_char(text.char_to_line(cursor));
        let prefix = text.slice(line_start..cursor).to_string();
        cursor
            - prefix
                .graphemes(true)
                .rev()
                .take_while(|g| g.chars().next().is_some_and(identifier))
                .map(|g| g.chars().count())
                .sum::<usize>()
    }

    pub(super) fn show_completion(
        &mut self,
        result: Value,
        context: Context,
        versions: Vec<Versioned>,
    ) {
        let items = result.as_array().or_else(|| result["items"].as_array());
        let prefix = self
            .document
            .buffer
            .text()
            .slice(self.completion_prefix_start(context.cursor)..context.cursor)
            .to_string()
            .to_lowercase();
        let mut items: Vec<_> = items
            .into_iter()
            .flatten()
            .take(lsp::MAX_ITEMS)
            .filter(|item| {
                item["insertTextFormat"] != 2
                    && self.completion_matches(item, context.cursor, &prefix)
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| completion_order(a).cmp(completion_order(b)));
        if items.is_empty() {
            self.dismiss_completion();
            return;
        }
        self.lsp.completion = Some(CompletionMenu {
            items,
            selected: 0,
            incomplete: result["isIncomplete"] == true,
            context,
            versions,
        });
    }

    fn completion_matches(&self, item: &Value, cursor: usize, prefix: &str) -> bool {
        let edit_prefix;
        let prefix = if let Some(edit) = item.get("textEdit") {
            // A server edit defines its own filtering prefix; zero-width edits
            // intentionally bypass word guessing (e.g. command-driven items).
            let range = edit.get("range").or_else(|| edit.get("replace"));
            let Some(range) =
                range.and_then(|v| serde_json::from_value::<lsp_types::Range>(v.clone()).ok())
            else {
                return false;
            };
            let Ok(range) = lsp::scalar_range(self.document.buffer.text(), range) else {
                return false;
            };
            if range.start > cursor || range.end < cursor {
                return false;
            }
            edit_prefix = self
                .document
                .buffer
                .text()
                .slice(range.start..cursor)
                .to_string()
                .to_lowercase();
            &edit_prefix
        } else {
            prefix
        };
        item["filterText"]
            .as_str()
            .or_else(|| item["label"].as_str())
            .is_some_and(|label| label.to_lowercase().starts_with(prefix))
    }

    pub(crate) fn completion_key(&mut self, key: Key) -> bool {
        if self
            .lsp
            .completion
            .as_ref()
            .is_some_and(|menu| !self.lsp_context_current(&menu.context))
        {
            self.dismiss_completion();
        }
        if key == Key::Ctrl('e') {
            self.dismiss_completion();
            return true;
        }
        let Some(menu) = &mut self.lsp.completion else {
            return false;
        };
        match key {
            Key::Escape => self.dismiss_completion(),
            Key::Down | Key::Ctrl('n') => menu.selected = (menu.selected + 1) % menu.items.len(),
            Key::Up | Key::Ctrl('p') => {
                menu.selected = (menu.selected + menu.items.len() - 1) % menu.items.len()
            }
            Key::Tab | Key::Enter => {
                if let Err(e) = self.accept_completion() {
                    self.dismiss_completion();
                    self.error(&e.to_string());
                }
            }
            _ => return false,
        }
        true
    }

    fn accept_completion(&mut self) -> io::Result<()> {
        let menu = self
            .lsp
            .completion
            .take()
            .ok_or_else(|| error("No completion selected"))?;
        let mut context = menu.context;
        if !self.lsp_context_current(&context) {
            return Err(error("Completion is stale; type again to refresh"));
        }
        let value = menu.items[menu.selected].clone();
        context.apply_resolved = true;
        if lsp::supports(
            &self.lsp.servers[&context.key].capabilities,
            "completionItem/resolve",
        ) {
            self.send_lsp("completionItem/resolve", value, Some(context))
        } else {
            self.completion_edit(value, context, menu.versions)
        }
    }

    pub(super) fn validate_lsp_command(
        &self,
        command: &Value,
        context: &Context,
    ) -> io::Result<()> {
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
        Ok(())
    }

    pub(super) fn completion_edit(
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
        if let Some(command) = &command {
            self.validate_lsp_command(command, &context)?;
        }
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
        let main_edit: lsp_types::TextEdit =
            serde_json::from_value(main.clone()).map_err(io::Error::other)?;
        let range = lsp::scalar_range(self.document.buffer.text(), main_edit.range)?;
        if range.start > context.cursor || range.end < context.cursor {
            return Err(error("Completion edit does not contain the request cursor"));
        }
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
        let plan = self.prepare_lsp_edit(&edit, &context.key.root, &versions, &context.guards)?;
        self.view = View::Editing;
        self.mode = context.mode;
        self.lsp.cancel();
        let cursor =
            self.apply_completion_plan(plan, &context, context.cursor, command.is_none())?;
        if let Some(command) = command {
            // Synchronize the primary edits before running the selected command.
            // Add the space after its edit so server-provided offsets stay intact.
            self.tick_lsp();
            let mut context = self.lsp_request_context()?;
            context.completion_command = Some(cursor);
            self.send_lsp("workspace/executeCommand", command, Some(context))?;
        }
        Ok(())
    }

    fn completion_noop(
        &self,
        context: &Context,
        versions: &[Versioned],
        cursor: usize,
    ) -> io::Result<EditPlan> {
        let position = lsp::position(self.document.buffer.text(), cursor)?;
        let edit = json!({"changes":{lsp::file_uri(&context.path)?.as_str():[
            {"range":lsp_types::Range::new(position, position),"newText":""}
        ]}});
        self.prepare_lsp_edit(&edit, &context.key.root, versions, &context.guards)
    }

    pub(super) fn apply_completion_command_edit(
        &mut self,
        edit: Option<&Value>,
        context: &Context,
        versions: &[Versioned],
    ) -> io::Result<()> {
        let cursor = context.completion_command.unwrap();
        let mut plan = if let Some(edit) = edit {
            self.prepare_lsp_edit(edit, &context.key.root, versions, &context.guards)?
        } else {
            self.completion_noop(context, versions, cursor)?
        };
        if !plan.files.iter().any(|file| file.path == context.path) {
            let source = self.completion_noop(context, versions, cursor)?;
            plan.files.extend(source.files);
            plan.edit_count += source.edit_count;
        }
        self.apply_completion_plan(plan, context, cursor, true)?;
        Ok(())
    }

    fn apply_completion_plan(
        &mut self,
        mut plan: EditPlan,
        context: &Context,
        old_cursor: usize,
        finish: bool,
    ) -> io::Result<usize> {
        let source = plan
            .files
            .iter_mut()
            .find(|file| file.path == context.path)
            .ok_or_else(|| error("Completion has no source edit"))?;
        let mut cursor = source.cursor_after(old_cursor);
        if finish && (cursor == 0 || source.after.char(cursor - 1) != ' ') {
            if source.after.get_char(cursor) != Some(' ') {
                if source.after.len_bytes() >= lsp::MAX_DOCUMENT_BYTES {
                    return Err(error("LSP edit result exceeds 2 MiB"));
                }
                // Keep the space in the same transaction as the completion and
                // account for imports or other edits before the insertion point.
                if let Some((_, text)) = source
                    .edits
                    .iter_mut()
                    .find(|(range, _)| range.start <= old_cursor && old_cursor <= range.end)
                {
                    text.push(' ');
                } else {
                    source.edits.push((old_cursor..old_cursor, " ".into()));
                    source
                        .edits
                        .sort_by_key(|(range, _)| (range.start, range.end));
                    plan.edit_count += 1;
                }
                source.after.insert(cursor, " ");
            }
            cursor += 1;
        }
        if plan.files.len() > 32 || plan.edit_count > lsp::MAX_ITEMS {
            return Err(error("Completion exceeds workspace edit limits"));
        }
        self.apply_lsp_edit(plan)?;
        self.document
            .buffer
            .set_cursor(cursor, context.mode == Mode::Insert);
        if finish {
            self.info("Completion inserted");
        }
        Ok(cursor)
    }
}

fn identifier(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn completion_order(item: &Value) -> &str {
    item["sortText"]
        .as_str()
        .or_else(|| item["label"].as_str())
        .unwrap_or("")
}

pub(crate) fn render(
    frame: &mut Frame<'_>,
    editor: &mut Editor,
    theme: &Theme,
    area: Rect,
    cursor: (u16, u16),
) {
    let Some(menu) = &editor.lsp.completion else {
        return;
    };
    if !editor.lsp_context_current(&menu.context) || area.width < 2 || area.height < 2 {
        return;
    }
    let below = area.bottom().saturating_sub(cursor.1 + 1);
    let above = cursor.1.saturating_sub(area.y);
    let wanted = (menu.items.len().min(8) + 1) as u16;
    let down = below >= wanted || below >= above;
    let height = wanted.min(if down { below } else { above });
    if height < 2 {
        return;
    }
    let width = area.width.min(64);
    let rect = Rect::new(
        cursor.0.min(area.right() - width),
        if down {
            cursor.1 + 1
        } else {
            cursor.1 - height
        },
        width,
        height,
    );
    let base = Style::default().fg(theme.foreground).bg(theme.status);
    editor.mouse.map.add(rect, crate::mouse::Target::Popup);
    // Styling a block does not erase the source characters beneath the popup.
    frame.render_widget(Clear, rect);
    frame.render_widget(Block::default().style(base), rect);
    let rows = usize::from(height - 1);
    let top = menu.selected.saturating_sub(rows - 1);
    for (row, item) in menu.items.iter().enumerate().skip(top).take(rows) {
        editor.mouse.map.add(
            Rect::new(rect.x, rect.y + (row - top) as u16, width, 1),
            crate::mouse::Target::Completion(row),
        );
        let label = format!(" {}", item["label"].as_str().unwrap_or(""));
        let style = if row == menu.selected {
            base.bg(theme.selection)
        } else {
            base
        };
        frame.render_widget(
            Paragraph::new(visible_text(&label, 0, width as usize)).style(style),
            Rect::new(rect.x, rect.y + (row - top) as u16, width, 1),
        );
    }
    let hint = format!(
        " {}/{} Tab/Enter accept  ↑↓ select  Esc/Ctrl-e dismiss",
        menu.selected + 1,
        menu.items.len()
    );
    editor.mouse.map.labels(
        Rect::new(rect.x, rect.bottom() - 1, width, 1),
        &hint,
        &[
            ("Tab/Enter accept", Key::Enter),
            ("Esc/Ctrl-e dismiss", Key::Ctrl('e')),
        ],
    );
    frame.render_widget(
        Paragraph::new(visible_text(&hint, 0, width as usize)).style(base.fg(theme.muted)),
        Rect::new(rect.x, rect.bottom() - 1, width, 1),
    );
}
