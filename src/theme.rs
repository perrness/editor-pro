use ratatui::style::Color;

use crate::editor::Mode;

/// Semantic palette; rendering code never chooses literal colors.
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub current_line: Color,
    pub status: Color,
    pub normal: Color,
    pub insert: Color,
    pub command: Color,
    pub error: Color,
    pub modified: Color,
}

impl Theme {
    pub fn night() -> Self {
        Self {
            background: Color::Rgb(26, 27, 38),
            foreground: Color::Rgb(192, 202, 245),
            muted: Color::Rgb(86, 95, 137),
            current_line: Color::Rgb(34, 36, 54),
            status: Color::Rgb(41, 46, 66),
            normal: Color::Rgb(122, 162, 247),
            insert: Color::Rgb(158, 206, 106),
            command: Color::Rgb(187, 154, 247),
            error: Color::Rgb(247, 118, 142),
            modified: Color::Rgb(224, 175, 104),
        }
    }

    pub fn mode_color(&self, mode: Mode) -> Color {
        match mode {
            Mode::Normal => self.normal,
            Mode::Insert => self.insert,
            Mode::Command => self.command,
        }
    }
}
