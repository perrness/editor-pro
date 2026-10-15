use ratatui::style::Color;

use crate::editor::Mode;
use crate::syntax::Highlight;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    #[default]
    TokyoNight,
    Catppuccin,
    Gruvbox,
}

impl ThemeName {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "tokyo-night" => Some(Self::TokyoNight),
            "catppuccin" => Some(Self::Catppuccin),
            "gruvbox" => Some(Self::Gruvbox),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::TokyoNight => "tokyo-night",
            Self::Catppuccin => "catppuccin",
            Self::Gruvbox => "gruvbox",
        }
    }
    pub fn palette(self) -> Theme {
        match self {
            Self::TokyoNight => Theme::night(),
            Self::Catppuccin => Theme::catppuccin(),
            Self::Gruvbox => Theme::gruvbox(),
        }
    }
}

/// Semantic palette; rendering code never chooses literal colors.
#[derive(Clone, Copy)]
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
    pub diff_added: Color,
    pub diff_modified: Color,
    pub diff_deleted: Color,
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
            diff_added: Color::Rgb(158, 206, 106),
            diff_modified: Color::Rgb(224, 175, 104),
            diff_deleted: Color::Rgb(247, 118, 142),
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

    pub fn catppuccin() -> Self {
        Self {
            background: rgb(0x1e1e2e),
            foreground: rgb(0xcdd6f4),
            muted: rgb(0x6c7086),
            current_line: rgb(0x313244),
            status: rgb(0x181825),
            normal: rgb(0x89b4fa),
            insert: rgb(0xa6e3a1),
            command: rgb(0xcba6f7),
            error: rgb(0xf38ba8),
            modified: rgb(0xf9e2af),
            diff_added: rgb(0xa6e3a1),
            diff_modified: rgb(0xf9e2af),
            diff_deleted: rgb(0xf38ba8),
            comment: rgb(0x6c7086),
            keyword: rgb(0xcba6f7),
            string: rgb(0xa6e3a1),
            number: rgb(0xfab387),
            function: rgb(0x89b4fa),
            type_name: rgb(0x94e2d5),
            property: rgb(0x89dceb),
            selection: rgb(0x45475a),
            search: rgb(0x665a3a),
        }
    }

    pub fn gruvbox() -> Self {
        Self {
            background: rgb(0x282828),
            foreground: rgb(0xebdbb2),
            muted: rgb(0x928374),
            current_line: rgb(0x32302f),
            status: rgb(0x3c3836),
            normal: rgb(0x83a598),
            insert: rgb(0xb8bb26),
            command: rgb(0xd3869b),
            error: rgb(0xfb4934),
            modified: rgb(0xfabd2f),
            diff_added: rgb(0xb8bb26),
            diff_modified: rgb(0xfabd2f),
            diff_deleted: rgb(0xfb4934),
            comment: rgb(0x928374),
            keyword: rgb(0xfb4934),
            string: rgb(0xb8bb26),
            number: rgb(0xd3869b),
            function: rgb(0xfabd2f),
            type_name: rgb(0x8ec07c),
            property: rgb(0x83a598),
            selection: rgb(0x504945),
            search: rgb(0x665c20),
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

fn rgb(value: u32) -> Color {
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}
