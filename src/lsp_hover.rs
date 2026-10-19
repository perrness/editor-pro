//! Debounced pointer documentation uses the existing bounded LSP worker. It
//! never moves the editing cursor or opens the keyboard's contextual LSP view.
use super::*;
use crate::mouse::Target;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph},
};
use std::{ops::Range, time::Duration};
use unicode_segmentation::UnicodeSegmentation;

struct HoverTarget {
    id: u64,
    stamp: Stamp,
    cursor: usize,
    mode: Mode,
    viewport: (usize, usize),
    position: usize,
    symbol: Range<usize>,
    pointer: (u16, u16),
    since: Instant,
    requested: bool,
}

#[derive(Default)]
pub struct HoverState {
    target: Option<HoverTarget>,
    next_id: u64,
    text: Option<String>,
}

impl HoverState {
    pub fn visible(&self) -> bool {
        self.text.is_some()
    }

    pub fn scheduled(&self) -> bool {
        self.target.as_ref().is_some_and(|target| !target.requested)
    }

    pub(super) fn clear(&mut self) {
        self.target = None;
        self.text = None;
    }
}

impl Editor {
    pub(crate) fn dismiss_mouse_hover(&mut self) -> bool {
        let visible = self.lsp.mouse_hover.visible();
        self.lsp.mouse_hover.clear();
        if self
            .lsp
            .context
            .as_ref()
            .is_some_and(|context| context.mouse_hover.is_some())
        {
            self.lsp.cancel_request();
        }
        visible
    }

    pub(super) fn mouse_hover_current(&self, id: u64) -> bool {
        self.mouse.settings.enabled
            && self.mouse.settings.hover_enabled
            && self.help.is_none()
            && self.disk.view.is_none()
            && self.view == View::Editing
            && self.note_edit.is_none()
            && self.lsp.mouse_hover.target.as_ref().is_some_and(|target| {
                target.id == id
                    && target.stamp.buffer_id == self.active_buffer_id
                    && target.stamp.instance == self.document.buffer.instance()
                    && target.stamp.generation == self.document.buffer.generation()
                    && target.cursor == self.document.buffer.cursor()
                    && target.mode == self.mode
                    && target.viewport == (self.viewport.top, self.viewport.left)
            })
    }

    pub(crate) fn mouse_hover_allowed(&self) -> bool {
        self.mouse.settings.hover_enabled
            && matches!(self.mode, Mode::Normal | Mode::Insert)
            && self.view == View::Editing
            && self.note_edit.is_none()
            && self.help.is_none()
            && self.disk.view.is_none()
            && self.lsp.completion.is_none()
            && !self.lsp.completion_scheduled()
            && self.document.buffer.text().len_bytes() <= lsp::MAX_DOCUMENT_BYTES
            && (self.document.virtual_uri().is_some()
                || self.document.path().and_then(Language::for_path).is_some())
    }

    pub(crate) fn move_mouse_hover(
        &mut self,
        position: Option<usize>,
        pointer: (u16, u16),
    ) -> bool {
        if !self.mouse_hover_allowed() {
            return self.dismiss_mouse_hover();
        }
        let Some(position) = position else {
            return self.dismiss_mouse_hover();
        };
        let buffer = &self.document.buffer;
        let line_index = buffer.text().char_to_line(position);
        let start = buffer.text().line_to_char(line_index);
        let line = buffer.line(line_index);
        let byte = line
            .char_indices()
            .nth(position - start)
            .map(|(byte, _)| byte);
        let word = byte.and_then(|byte| {
            line.split_word_bound_indices()
                .find(|(start, word)| *start <= byte && byte < start + word.len())
        });
        let Some((byte, word)) = word.filter(|(_, word)| !word.chars().all(char::is_whitespace))
        else {
            return self.dismiss_mouse_hover();
        };
        let symbol_start = start + line[..byte].chars().count();
        let symbol = symbol_start..symbol_start + word.chars().count();
        if self
            .lsp
            .mouse_hover
            .target
            .as_ref()
            .is_some_and(|target| target.symbol == symbol && self.mouse_hover_current(target.id))
        {
            let target = self.lsp.mouse_hover.target.as_mut().unwrap();
            if target.pointer != pointer && !target.requested {
                target.since = Instant::now();
                target.position = position;
                target.pointer = pointer;
            }
            return false;
        }
        let changed = self.dismiss_mouse_hover();
        self.lsp.mouse_hover.next_id = self.lsp.mouse_hover.next_id.wrapping_add(1);
        self.lsp.mouse_hover.target = Some(HoverTarget {
            id: self.lsp.mouse_hover.next_id,
            stamp: Stamp {
                buffer_id: self.active_buffer_id,
                instance: self.document.buffer.instance(),
                generation: self.document.buffer.generation(),
            },
            cursor: self.document.buffer.cursor(),
            mode: self.mode,
            viewport: (self.viewport.top, self.viewport.left),
            position,
            symbol,
            pointer,
            since: Instant::now(),
            requested: false,
        });
        changed
    }

    pub(super) fn tick_mouse_hover(&mut self) -> bool {
        let Some(target) = &self.lsp.mouse_hover.target else {
            return false;
        };
        if !self.mouse_hover_current(target.id) {
            return self.dismiss_mouse_hover();
        }
        if target.requested
            || target.since.elapsed() < Duration::from_millis(self.mouse.settings.hover_delay_ms)
            || self.lsp.pending
            || self.lsp.completion.is_some()
            || self.lsp.completion_scheduled()
        {
            return false;
        }
        let (id, position) = (target.id, target.position);
        let Ok(mut context) = self.lsp_request_context() else {
            if self
                .document
                .path()
                .is_some_and(|path| self.lsp.unavailable.contains_key(path))
            {
                self.lsp.mouse_hover.target.as_mut().unwrap().requested = true;
            }
            return false;
        };
        let server = &self.lsp.servers[&context.key];
        if !server.capabilities.is_object() {
            if server.status.phase == lsp::Phase::Failed {
                self.lsp.mouse_hover.target.as_mut().unwrap().requested = true;
            }
            return false;
        }
        // One attempt per resting symbol, including unsupported or failed servers.
        // Passive movement must never replace messages with startup/errors.
        self.lsp.mouse_hover.target.as_mut().unwrap().requested = true;
        let _ = (|| -> io::Result<()> {
            context.mouse_hover = Some(id);
            let uri = if let Some(uri) = self.document.virtual_uri() {
                uri.to_owned()
            } else {
                lsp::file_uri(&context.path)?.to_string()
            };
            let params = json!({"textDocument":{"uri":uri}, "position":lsp::position(self.document.buffer.text(), position)?});
            self.send_lsp("textDocument/hover", params, Some(context))
        })();
        false
    }

    pub(super) fn show_mouse_hover(&mut self, result: &Value) {
        let text: String = markup(&result["contents"])
            .chars()
            .take(32 * 1024)
            .collect();
        self.lsp.mouse_hover.text = (!text.trim().is_empty()).then_some(text);
    }

    /// A row click selects a preview, but only the existing Enter key approves
    /// a workspace edit. Location and completion rows reuse their normal action.
    pub(crate) fn click_lsp(&mut self, index: usize) {
        if let Some(panel) = &mut self.lsp.panel {
            panel.selected = index.min(panel.rows.len().saturating_sub(1));
            if panel.plan.is_none() {
                self.lsp_key(Key::Enter);
            }
        }
    }
}

pub(crate) fn render(frame: &mut Frame<'_>, editor: &mut Editor, theme: &crate::theme::Theme) {
    let Some(target) = &editor.lsp.mouse_hover.target else {
        return;
    };
    let Some(text) = &editor.lsp.mouse_hover.text else {
        return;
    };
    if !editor.mouse_hover_current(target.id) {
        return;
    }
    let area = frame.area();
    let width = area.width.min(72);
    let height = area.height.saturating_sub(2).min(12);
    if width < 4 || height < 3 {
        return;
    }
    // Existing text rendering supports literal markup, code fences and lines;
    // preserve these, wrap by graphemes, and sanitize terminal controls.
    let rows = wrapped(text, usize::from(width - 2), usize::from(height - 2));
    let height = (rows.len() as u16 + 2).min(height);
    let y = if target.pointer.1.saturating_add(height + 1) <= area.bottom().saturating_sub(2) {
        target.pointer.1 + 1
    } else {
        target.pointer.1.saturating_sub(height).max(area.y)
    };
    let rect = Rect::new(
        target.pointer.0.min(area.right() - width).max(area.x),
        y,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(rows.join("\n"))
            .style(Style::default().fg(theme.foreground).bg(theme.panel))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Hover · right-click/Esc closes ")
                    .border_style(Style::default().fg(theme.border)),
            ),
        rect,
    );
    editor.mouse.map.add(rect, Target::Popup);
}

fn wrapped(text: &str, width: usize, height: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut row = String::new();
        let mut cells = 0;
        for grapheme in line.graphemes(true) {
            let mut size = crate::text::cell_width(grapheme, cells);
            if cells + size > width && cells > 0 {
                rows.push(std::mem::take(&mut row));
                if rows.len() == height {
                    return rows;
                }
                cells = 0;
                size = crate::text::cell_width(grapheme, cells);
            }
            if size > width {
                row.push('�');
                cells += 1;
            } else {
                row.push_str(&crate::text::display_grapheme(grapheme, cells));
                cells += size;
            }
        }
        rows.push(row);
        if rows.len() == height {
            break;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::wrapped;

    #[test]
    fn hover_wrap_preserves_indent_graphemes_and_neutralizes_control_sequences() {
        assert_eq!(
            wrapped("123456789界e\u{301}Z", 10, 4),
            ["123456789", "界e\u{301}Z"]
        );
        assert_eq!(
            wrapped("\tfoo\r\n\x1b[31mbar", 10, 4),
            ["    foo", "�[31mbar"]
        );
        assert_eq!(wrapped("one\ntwo\nthree", 10, 2), ["one", "two"]);
    }
}
