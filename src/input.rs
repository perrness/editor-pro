//! Terminal event decoding. Editor commands do not depend on Crossterm.
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    BackTab,
    F1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Up,
    Drag,
    Move,
    RightDown,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    pub column: u16,
    pub row: u16,
    pub control: bool,
    pub shift: bool,
}

pub fn decode_mouse(event: ratatui::crossterm::event::MouseEvent) -> Option<Mouse> {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    if event
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    Some(Mouse {
        kind: match event.kind {
            MouseEventKind::Down(MouseButton::Left) => MouseKind::Down,
            MouseEventKind::Up(MouseButton::Left) => MouseKind::Up,
            MouseEventKind::Drag(MouseButton::Left) => MouseKind::Drag,
            MouseEventKind::Down(MouseButton::Right) => MouseKind::RightDown,
            MouseEventKind::Moved => MouseKind::Move,
            MouseEventKind::ScrollUp => MouseKind::ScrollUp,
            MouseEventKind::ScrollDown => MouseKind::ScrollDown,
            MouseEventKind::ScrollLeft => MouseKind::ScrollLeft,
            MouseEventKind::ScrollRight => MouseKind::ScrollRight,
            _ => return None,
        },
        column: event.column,
        row: event.row,
        control: event.modifiers.contains(KeyModifiers::CONTROL),
        shift: event.modifiers.contains(KeyModifiers::SHIFT),
    })
}

pub fn decode(event: KeyEvent) -> Option<Key> {
    if event.kind == KeyEventKind::Release
        || event
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        return match event.code {
            KeyCode::Char(character) => Some(Key::Ctrl(character.to_ascii_lowercase())),
            _ => None,
        };
    }
    Some(match event.code {
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Esc => Key::Escape,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Tab => Key::Tab,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::F(1) => Key::F1,
        _ => return None,
    })
}
