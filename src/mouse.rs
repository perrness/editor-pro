//! Mouse dispatch and screen-to-document mapping. Renderers register only the
//! regions they actually display; actions reuse the ordinary editor commands.
use std::time::{Duration, Instant};

use ratatui::layout::{Position, Rect};

use crate::{
    buffer::Buffer,
    config::MouseSettings,
    editor::{Editor, Mode, View},
    input::{Key, Mouse, MouseKind},
    text::{char_at_display_column, display_column},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Key(Key),
    Command(&'static str),
    Buffer(usize),
    File(u32),
    Tree(usize),
    Change(usize),
    Diff(usize),
    Lsp(usize),
    Completion(usize),
    Recent(usize),
    Search(usize),
    Review(usize),
    Ai(usize),
    Diagnostic(usize),
    Popup,
}

#[derive(Clone, Copy, Debug)]
pub struct SourceArea {
    pub area: Rect,
    pub gutter: u16,
    pub top: usize,
    pub left: usize,
}

impl SourceArea {
    /// Returns a Unicode scalar index at the start of a grapheme. Gutter and
    /// beyond-EOL clicks clamp to the line; hover uses `symbol_only` instead.
    pub fn position(
        self,
        buffer: &Buffer,
        column: u16,
        row: u16,
        symbol_only: bool,
    ) -> Option<usize> {
        if self.area.is_empty() || !self.area.contains(Position::new(column, row)) {
            return None;
        }
        let line = self.top + usize::from(row - self.area.y);
        if symbol_only && (line >= buffer.line_count() || column < self.area.x + self.gutter) {
            return None;
        }
        let line = line.min(buffer.line_count() - 1);
        let text = buffer.line(line);
        let cell = if column < self.area.x + self.gutter {
            0
        } else {
            self.left + usize::from(column - self.area.x - self.gutter)
        };
        let scalar = char_at_display_column(&text, cell);
        if symbol_only && scalar == text.chars().count() {
            return None;
        }
        Some(buffer.text().line_to_char(line) + scalar)
    }
}

#[derive(Default)]
pub(crate) struct HitMap {
    pub source: Option<SourceArea>,
    regions: Vec<(Rect, Target)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.source = None;
        self.regions.clear();
    }

    pub fn add(&mut self, area: Rect, target: Target) {
        if !area.is_empty() {
            self.regions.push((area, target));
        }
    }

    fn hit(&self, column: u16, row: u16) -> Option<Target> {
        self.regions
            .iter()
            .rev()
            .find_map(|(area, target)| area.contains(Position::new(column, row)).then_some(*target))
    }

    /// Register existing footer labels, clipped by cells just like their text.
    pub fn labels(&mut self, area: Rect, text: &str, labels: &[(&str, Key)]) {
        for (label, key) in labels {
            if let Some(byte) = text.find(label) {
                let left = display_column(&text[..byte], usize::MAX);
                let width = display_column(label, usize::MAX);
                if left + width <= usize::from(area.width) {
                    self.add(
                        Rect::new(area.x + left as u16, area.y, width as u16, 1),
                        Target::Key(*key),
                    );
                }
            }
        }
    }
}

struct Drag {
    anchor: usize,
    instance: u64,
    generation: u64,
    linewise: bool,
    pointer: (u16, u16),
    scrolling: bool,
    last_scroll: Instant,
}

#[derive(Default)]
pub struct MouseState {
    pub settings: MouseSettings,
    pub(crate) map: HitMap,
    /// Wheel scrolling can leave the cursor offscreen until the next key.
    pub(crate) scrolled: bool,
    drag: Option<Drag>,
}

impl MouseState {
    pub fn dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| drag.scrolling)
    }
}

impl Editor {
    pub fn cancel_mouse_interaction(&mut self) -> bool {
        self.mouse.drag = None;
        self.mouse.scrolled = false;
        self.mouse.map.clear();
        self.dismiss_mouse_hover()
    }

    pub fn handle_mouse(&mut self, event: Mouse) -> bool {
        if !self.mouse.settings.enabled {
            return false;
        }
        if event.kind == MouseKind::Up {
            self.mouse.drag = None;
            return false;
        }
        if event.kind == MouseKind::Drag {
            return self.drag_mouse(event.column, event.row);
        }
        let target = self.mouse.map.hit(event.column, event.row);
        if event.kind == MouseKind::Move {
            // Unpressed movement also recovers from a lost release event.
            self.mouse.drag = None;
            // Skip line materialization/segmentation for unsupported or oversized
            // documents, disabled hover, overlays and active completions.
            if !self.mouse_hover_allowed() {
                return self.dismiss_mouse_hover();
            }
            if target == Some(Target::Popup) {
                return false;
            }
            let position = if target.is_none() {
                self.mouse.map.source.and_then(|area| {
                    area.position(&self.document.buffer, event.column, event.row, true)
                })
            } else {
                None
            };
            return self.move_mouse_hover(position, (event.column, event.row));
        }
        let dismissed = self.dismiss_mouse_hover();
        if event.kind == MouseKind::RightDown {
            self.mouse.drag = None;
            if dismissed {
                return true;
            }
            self.handle_key(Key::Escape);
            return true;
        }
        let scrolling = matches!(
            event.kind,
            MouseKind::ScrollUp
                | MouseKind::ScrollDown
                | MouseKind::ScrollLeft
                | MouseKind::ScrollRight
        );
        if scrolling {
            return self.scroll_mouse(event, target) || dismissed;
        }
        if event.kind != MouseKind::Down {
            return dismissed;
        }
        self.mouse.drag = None;
        // Mouse clicks never submit command/search or filesystem prompts.
        if matches!(self.mode, Mode::Command | Mode::Search) && self.help.is_none() {
            return dismissed;
        }
        if let Some(target) = target {
            return self.click_mouse_target(target) || dismissed;
        }
        if self.view != View::Editing || self.help.is_some() || self.disk.view.is_some() {
            return dismissed;
        }
        let Some(position) =
            self.mouse.map.source.and_then(|area| {
                area.position(&self.document.buffer, event.column, event.row, false)
            })
        else {
            return dismissed;
        };
        if self.note_edit.is_some() && self.review.saving_note() {
            return dismissed;
        }
        self.dismiss_completion();
        self.clear_pending_input();
        self.document.buffer.commit();
        let old_cursor = self.document.buffer.cursor();
        let linewise = self.mode == Mode::VisualLine;
        if event.shift {
            self.selection_anchor.get_or_insert(old_cursor);
            if !linewise {
                self.mode = Mode::Visual;
            }
        }
        self.document
            .buffer
            .set_cursor(position, self.mode == Mode::Insert);
        if event.control && self.note_edit.is_none() {
            self.mode = Mode::Normal;
            self.selection_anchor = None;
            self.document.buffer.clamp_cursor(false);
            if let Err(error) = self.lsp_navigation_shortcut("Definition") {
                self.error(&error.to_string());
            }
        } else {
            self.mouse.drag = Some(Drag {
                anchor: self
                    .selection_anchor
                    .unwrap_or(self.document.buffer.cursor()),
                instance: self.document.buffer.instance(),
                generation: self.document.buffer.generation(),
                linewise,
                pointer: (event.column, event.row),
                scrolling: false,
                last_scroll: Instant::now(),
            });
        }
        true
    }

    fn click_mouse_target(&mut self, target: Target) -> bool {
        match target {
            Target::Key(key) => self.handle_key(key),
            Target::Command(command) => {
                if let Err(error) = self.execute_command(command) {
                    self.error(&error.to_string());
                }
            }
            Target::Buffer(id) => {
                if let Err(error) = self.switch_buffer(id) {
                    self.error(&error.to_string());
                }
            }
            Target::File(index) => {
                if let Some(picker) = &mut self.picker {
                    picker.selected = index;
                    self.handle_key(Key::Enter);
                }
            }
            Target::Tree(index) => self.click_tree(index),
            Target::Change(index) => {
                self.git.selected = index;
                self.handle_key(Key::Char('d'));
            }
            Target::Diff(index) => {
                if let Some(view) = &mut self.git.diff_view {
                    view.selected = index;
                    view.anchor = self
                        .git
                        .snapshot
                        .as_ref()
                        .and_then(|s| s.diff.as_ref().ok())
                        .and_then(|diff| diff.rows.get(index))
                        .map_or(0, |row| row.anchor);
                }
            }
            Target::Lsp(index) => self.click_lsp(index),
            Target::Completion(index) => {
                if let Some(menu) = &mut self.lsp.completion {
                    menu.selected = index.min(menu.items.len().saturating_sub(1));
                    self.completion_key(Key::Enter);
                }
            }
            Target::Recent(index) => {
                self.click_recent(index);
            }
            Target::Search(index) => {
                self.project_search.selected = index;
                self.handle_key(Key::Enter);
            }
            Target::Review(index) => {
                self.review.selected = index;
                self.handle_key(Key::Enter);
            }
            Target::Ai(index) => {
                self.ai.selected = index;
                self.handle_key(Key::Enter);
            }
            Target::Diagnostic(position) => {
                self.dismiss_completion();
                self.clear_pending_input();
                self.document.buffer.commit();
                self.document
                    .buffer
                    .set_cursor(position, self.mode == Mode::Insert);
                self.mouse.scrolled = false;
            }
            Target::Popup => {}
        }
        true
    }

    fn scroll_mouse(&mut self, event: Mouse, target: Option<Target>) -> bool {
        if matches!(self.mode, Mode::Command | Mode::Search) && self.help.is_none() {
            return false;
        }
        let backward = matches!(event.kind, MouseKind::ScrollUp | MouseKind::ScrollLeft);
        let horizontal =
            event.shift || matches!(event.kind, MouseKind::ScrollLeft | MouseKind::ScrollRight);
        if self.lsp.completion.is_some()
            && matches!(
                target,
                Some(Target::Completion(_) | Target::Popup | Target::Key(_))
            )
        {
            if !horizontal {
                self.completion_key(if backward { Key::Up } else { Key::Down });
                return true;
            }
            return false;
        }
        if self.help.is_some() || self.disk.view.is_some() || self.view != View::Editing {
            // Tree Left/Right fold directories, rather than scroll. Unsupported
            // horizontal scrolling must not accidentally activate those keys.
            if horizontal
                && self.disk.view.is_none()
                && (self.help.is_some()
                    || !matches!(
                        self.view,
                        View::Diff | View::Lsp | View::Recent | View::ProjectSearch | View::Ai
                    ))
            {
                return false;
            }
            let key = match (horizontal, backward) {
                (true, true) => Key::Left,
                (true, false) => Key::Right,
                (false, true) => Key::Up,
                (false, false) => Key::Down,
            };
            for _ in 0..3 {
                self.handle_key(key);
            }
            return true;
        }
        let Some(area) = self
            .mouse
            .map
            .source
            .filter(|area| area.area.contains(Position::new(event.column, event.row)))
        else {
            return false;
        };
        self.dismiss_completion();
        let before = self.viewport.clone();
        if horizontal {
            let width = usize::from(area.area.width.saturating_sub(area.gutter));
            // Only inspect the visible lines, even in very large documents.
            let end = (self.viewport.top + usize::from(area.area.height))
                .min(self.document.buffer.line_count());
            let max = (self.viewport.top..end)
                .map(|line| display_column(&self.document.buffer.line(line), usize::MAX))
                .max()
                .unwrap_or(0)
                .saturating_sub(width.saturating_sub(1));
            self.viewport.left = if backward {
                self.viewport.left.saturating_sub(4)
            } else {
                (self.viewport.left + 4).min(max)
            };
        } else {
            let max = self
                .document
                .buffer
                .line_count()
                .saturating_sub(usize::from(area.area.height).max(1));
            self.viewport.top = if backward {
                self.viewport.top.saturating_sub(3)
            } else {
                (self.viewport.top + 3).min(max)
            };
        }
        self.mouse.scrolled = true;
        if let Some(drag) = &self.mouse.drag {
            let (column, row) = drag.pointer;
            self.update_mouse_selection(column, row);
        }
        self.viewport != before
    }

    fn drag_mouse(&mut self, column: u16, row: u16) -> bool {
        if self.view != View::Editing || self.help.is_some() || self.disk.view.is_some() {
            return false;
        }
        let Some(area) = self.mouse.map.source else {
            return false;
        };
        let Some(drag) = &mut self.mouse.drag else {
            return false;
        };
        if drag.instance != self.document.buffer.instance()
            || drag.generation != self.document.buffer.generation()
        {
            self.mouse.drag = None;
            return false;
        }
        drag.pointer = (column, row);
        drag.scrolling = row <= area.area.y
            || row >= area.area.bottom().saturating_sub(1)
            || column < area.area.x + area.gutter
            || column >= area.area.right().saturating_sub(1);
        self.update_mouse_selection(column, row)
    }

    fn update_mouse_selection(&mut self, column: u16, row: u16) -> bool {
        let Some(mut area) = self.mouse.map.source else {
            return false;
        };
        let Some(drag) = &self.mouse.drag else {
            return false;
        };
        if area.area.is_empty() {
            return false;
        }
        area.top = self.viewport.top;
        area.left = self.viewport.left;
        let column = column.clamp(area.area.x, area.area.right() - 1);
        let row = row.clamp(area.area.y, area.area.bottom() - 1);
        let Some(position) = area.position(&self.document.buffer, column, row, false) else {
            return false;
        };
        self.selection_anchor = Some(drag.anchor);
        self.mode = if drag.linewise {
            Mode::VisualLine
        } else {
            Mode::Visual
        };
        self.document.buffer.set_cursor(position, false);
        self.mouse.scrolled = true;
        true
    }

    pub(crate) fn tick_mouse_drag(&mut self) -> bool {
        let Some(drag) = &mut self.mouse.drag else {
            return false;
        };
        if !drag.scrolling || drag.last_scroll.elapsed() < Duration::from_millis(50) {
            return false;
        }
        if drag.instance != self.document.buffer.instance()
            || drag.generation != self.document.buffer.generation()
            || self.view != View::Editing
            || self.help.is_some()
            || self.disk.view.is_some()
        {
            self.mouse.drag = None;
            return false;
        }
        let Some(area) = self.mouse.map.source else {
            return false;
        };
        drag.last_scroll = Instant::now();
        let (column, row) = drag.pointer;
        let before = (
            self.viewport.top,
            self.viewport.left,
            self.document.buffer.cursor(),
        );
        if row <= area.area.y {
            self.viewport.top = self.viewport.top.saturating_sub(1);
        } else if row >= area.area.bottom().saturating_sub(1) {
            self.viewport.top = (self.viewport.top + 1).min(
                self.document
                    .buffer
                    .line_count()
                    .saturating_sub(usize::from(area.area.height).max(1)),
            );
        }
        if column < area.area.x + area.gutter {
            self.viewport.left = self.viewport.left.saturating_sub(1);
        } else if column >= area.area.right().saturating_sub(1) {
            let line = (self.viewport.top + usize::from(row.saturating_sub(area.area.y)))
                .min(self.document.buffer.line_count() - 1);
            let max = display_column(&self.document.buffer.line(line), usize::MAX);
            self.viewport.left = (self.viewport.left + 1).min(max);
        }
        self.update_mouse_selection(column, row);
        before
            != (
                self.viewport.top,
                self.viewport.left,
                self.document.buffer.cursor(),
            )
    }
}
