//! Session-local source locations and recent paths. These never own source text.
use crate::{
    editor::{Editor, Mode, View, Viewport},
    input::Key,
};
use std::{io, path::PathBuf};

const HISTORY_LIMIT: usize = 256;
const RECENT_LIMIT: usize = 100;

#[derive(Clone)]
pub(crate) struct Location {
    buffer_id: usize,
    path: Option<PathBuf>,
    cursor: usize,
    viewport: Viewport,
}
impl Location {
    fn same_position(&self, other: &Self) -> bool {
        self.buffer_id == other.buffer_id && self.cursor == other.cursor
    }
}

#[derive(Default)]
pub struct Navigation {
    back: Vec<Location>,
    forward: Vec<Location>,
    pub recent: Vec<PathBuf>,
    selected: usize,
    top: usize,
    left: usize,
}

fn push_location(stack: &mut Vec<Location>, location: Location) {
    if stack
        .last()
        .is_some_and(|last| last.same_position(&location))
    {
        *stack.last_mut().unwrap() = location;
    } else {
        stack.push(location);
        if stack.len() > HISTORY_LIMIT {
            stack.remove(0);
        }
    }
}

impl Editor {
    pub(crate) fn source_location(&self) -> Location {
        Location {
            buffer_id: self.active_buffer_id,
            path: self.document.path().map(Into::into),
            cursor: self.document.buffer.cursor(),
            viewport: self.viewport.clone(),
        }
    }

    pub(crate) fn record_navigation(&mut self, origin: Location) {
        if self.note_edit.is_none() && !origin.same_position(&self.source_location()) {
            push_location(&mut self.navigation.back, origin);
            self.navigation.forward.clear();
        }
    }

    pub(crate) fn remember_file(&mut self) {
        if self.note_edit.is_none()
            && let Some(path) = self.document.path()
        {
            self.navigation.recent.retain(|p| p != path);
            self.navigation.recent.insert(0, path.to_path_buf());
            self.navigation.recent.truncate(RECENT_LIMIT);
            for location in self
                .navigation
                .back
                .iter_mut()
                .chain(&mut self.navigation.forward)
            {
                if location.buffer_id == self.active_buffer_id {
                    location.path = Some(path.to_path_buf());
                }
            }
        }
    }

    pub fn navigate_history(&mut self, backward: bool) -> io::Result<()> {
        if self.note_edit.is_some() || self.view != View::Editing {
            return Err(io::Error::other(
                "Return to source before navigating history",
            ));
        }
        let stack = if backward {
            &self.navigation.back
        } else {
            &self.navigation.forward
        };
        let target = stack.last().cloned().ok_or_else(|| {
            io::Error::other(if backward {
                "No earlier location"
            } else {
                "No later location"
            })
        })?;
        let origin = self.source_location();
        let result = if target.buffer_id == self.active_buffer_id
            || self.buffers.contains_key(&target.buffer_id)
        {
            self.switch_buffer_unrecorded(target.buffer_id)
        } else if let Some(path) = &target.path {
            // History must not turn a deleted file into a new empty document.
            if !self.has_open_path(path) && !path.is_file() {
                Err(io::Error::other("History file is unavailable"))
            } else {
                self.open_buffer_unrecorded(path)
            }
        } else {
            Err(io::Error::other("Unnamed buffer was closed"))
        };
        if let Err(e) = result {
            // Discard only this inaccessible entry, so another back/forward can
            // reach the rest of the history without moving the current source.
            if backward {
                self.navigation.back.pop();
            } else {
                self.navigation.forward.pop();
            }
            return Err(io::Error::other(format!(
                "{e}; unavailable history entry skipped"
            )));
        }
        self.document.buffer.set_cursor(target.cursor, false);
        self.viewport = target.viewport;
        if backward {
            self.navigation.back.pop();
            push_location(&mut self.navigation.forward, origin);
        } else {
            self.navigation.forward.pop();
            push_location(&mut self.navigation.back, origin);
        }
        self.info(if backward {
            "Earlier location"
        } else {
            "Later location"
        });
        Ok(())
    }

    pub(crate) fn open_recent(&mut self) {
        self.document.buffer.commit();
        self.reset_context();
        self.navigation.selected = 0;
        self.navigation.top = 0;
        self.navigation.left = 0;
        self.view = View::Recent;
        self.info("Recent files (this session); Enter opens, Escape returns");
    }

    pub(crate) fn click_recent(&mut self, index: usize) {
        self.navigation.selected = index;
        self.recent_key(Key::Enter);
    }

    pub(crate) fn recent_key(&mut self, key: Key) {
        let nav = &mut self.navigation;
        match key {
            Key::Escape | Key::Ctrl('c' | '[') => self.view = View::Editing,
            Key::Char(':') => {
                self.mode = Mode::Command;
                self.command.clear();
            }
            Key::Char('j') | Key::Down | Key::Ctrl('n') => {
                nav.selected = (nav.selected + 1).min(nav.recent.len().saturating_sub(1))
            }
            Key::Char('k') | Key::Up | Key::Ctrl('p') => {
                nav.selected = nav.selected.saturating_sub(1)
            }
            Key::PageDown => {
                nav.selected = (nav.selected + 10).min(nav.recent.len().saturating_sub(1))
            }
            Key::PageUp => nav.selected = nav.selected.saturating_sub(10),
            Key::Char('g') => nav.selected = 0,
            Key::Char('G') => nav.selected = nav.recent.len().saturating_sub(1),
            Key::Left | Key::Char('h') => nav.left = nav.left.saturating_sub(4),
            Key::Right | Key::Char('l') => nav.left = nav.left.saturating_add(4),
            Key::Enter => {
                if let Some(path) = nav.recent.get(nav.selected).cloned() {
                    let result = if !self.has_open_path(&path) && !path.is_file() {
                        Err(io::Error::other("Recent file is unavailable"))
                    } else {
                        self.open_buffer(&path)
                    };
                    if let Err(e) = result {
                        self.error(&e.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn render(frame: &mut ratatui::Frame<'_>, editor: &mut Editor, theme: &crate::theme::Theme) {
    use ratatui::{layout::Rect, style::Style, widgets::Paragraph};
    let area = frame.area();
    let nav = &mut editor.navigation;
    let height = area.height.saturating_sub(2) as usize;
    nav.top = nav.top.min(nav.selected);
    if height > 0 && nav.selected >= nav.top + height {
        nav.top = nav.selected + 1 - height;
    }
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let title = format!("Recent files · {} · this session", nav.recent.len());
    frame.render_widget(
        Paragraph::new(crate::text::visible_text(&title, 0, area.width as usize))
            .style(style.fg(theme.command)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    for (row, path) in nav.recent.iter().skip(nav.top).take(height).enumerate() {
        editor.mouse.map.add(
            Rect::new(area.x, area.y + row as u16 + 1, area.width, 1),
            crate::mouse::Target::Recent(nav.top + row),
        );
        let text = crate::text::visible_text(
            &crate::git::display_path(path),
            nav.left,
            area.width as usize,
        );
        frame.render_widget(
            Paragraph::new(text).style(if nav.top + row == nav.selected {
                style.bg(theme.selection)
            } else {
                style
            }),
            Rect::new(area.x, area.y + row as u16 + 1, area.width, 1),
        );
    }
    if area.height > 1 {
        let footer = if editor.mode == Mode::Command {
            format!(":{}", editor.command)
        } else if editor.message_is_error {
            editor.message.clone()
        } else {
            "Enter open · Esc return · j/k select · h/l scroll".into()
        };
        frame.render_widget(
            Paragraph::new(crate::text::visible_text(
                &footer,
                if editor.mode == Mode::Command {
                    crate::text::display_column(&footer, footer.chars().count())
                        .saturating_sub(area.width.saturating_sub(1) as usize)
                } else {
                    0
                },
                area.width as usize,
            ))
            .style(style.fg(if editor.message_is_error {
                theme.error
            } else {
                theme.muted
            })),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
        if editor.mode == Mode::Command {
            let column = crate::text::display_column(&footer, footer.chars().count())
                .min(area.width.saturating_sub(1) as usize);
            frame.set_cursor_position((area.x + column as u16, area.bottom() - 1));
        }
    }
}

impl Navigation {
    pub(crate) fn move_tree_path(&mut self, source: &std::path::Path, target: &std::path::Path) {
        for path in &mut self.recent {
            if let Ok(relative) = path.strip_prefix(source) {
                *path = if relative.as_os_str().is_empty() {
                    target.to_path_buf()
                } else {
                    target.join(relative)
                };
            }
        }
        for location in self.back.iter_mut().chain(&mut self.forward) {
            if let Some(path) = &mut location.path
                && let Ok(relative) = path.strip_prefix(source)
            {
                *path = if relative.as_os_str().is_empty() {
                    target.to_path_buf()
                } else {
                    target.join(relative)
                };
            }
        }
    }
    pub(crate) fn forget_tree_path(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| !p.starts_with(path));
        self.back
            .retain(|l| !l.path.as_ref().is_some_and(|p| p.starts_with(path)));
        self.forward
            .retain(|l| !l.path.as_ref().is_some_and(|p| p.starts_with(path)));
    }
}
