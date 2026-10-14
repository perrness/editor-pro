use ratatui::style::Color;

use crate::editor::Mode;
use crate::syntax::Highlight;

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
    pub comment: Color,
    pub keyword: Color,
    pub string: Color,
    pub number: Color,
    pub function: Color,
    pub type_name: Color,
    pub property: Color,
    pub selection: Color,
    pub search: Color,
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
            comment: Color::Rgb(86, 95, 137),
            keyword: Color::Rgb(187, 154, 247),
            string: Color::Rgb(158, 206, 106),
            number: Color::Rgb(255, 158, 100),
            function: Color::Rgb(122, 162, 247),
            type_name: Color::Rgb(42, 195, 222),
            property: Color::Rgb(125, 207, 255),
            selection: Color::Rgb(54, 74, 130),
            search: Color::Rgb(96, 75, 48),
        }
    }

    pub fn highlight(&self, kind: Highlight) -> Color {
        match kind {
            Highlight::Comment => self.comment,
            Highlight::Keyword => self.keyword,
            Highlight::String => self.string,
            Highlight::Number | Highlight::Constant => self.number,
            Highlight::Function | Highlight::Heading => self.function,
            Highlight::Type => self.type_name,
            Highlight::Property => self.property,
            Highlight::Operator | Highlight::Emphasis => self.keyword,
            Highlight::Punctuation => self.foreground,
        }
    }

    pub fn mode_color(&self, mode: Mode) -> Color {
        match mode {
            Mode::Normal => self.normal,
            Mode::Insert => self.insert,
            Mode::Visual | Mode::VisualLine => self.command,
            Mode::Command | Mode::Search => self.command,
        }
    }
}
