//! Bounded, ASCII-only startup artwork. All colors come from the active palette.
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::theme::Theme;

const LOGO: [&str; 6] = [
    r"           __ _ __",
    r"  ___ ____/ /(_) /_____  _____      ____  _________",
    r" / _ \/ __  / / __/ __ \/ ___/_____/ __ \/ ___/ __ \",
    r"/  __/ /_/ / / /_/ /_/ / /  /_____/ /_/ / /  / /_/ /",
    r"\___/\__,_/_/\__/\____/_/        / .___/_/   \____/",
    r"                               /_/",
];

pub(crate) fn render(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let accent = Style::default()
        .fg(theme.normal)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.muted);
    let logo_width = LOGO.iter().map(|line| line.len()).max().unwrap_or(0);
    let full = usize::from(area.width) >= logo_width && area.height >= 16;
    let mut lines = Vec::new();
    if full {
        for (index, row) in LOGO.iter().enumerate() {
            let color = match index {
                0 | 1 => theme.type_name,
                2 | 3 => theme.normal,
                _ => theme.command,
            };
            // Center the whole wordmark, preserving alignment between its rows.
            lines.push(Line::styled(
                format!("{row:<logo_width$}"),
                accent.fg(color),
            ));
        }
        lines.push(Line::default());
    }
    let title = format!("editor-pro  v{}", env!("CARGO_PKG_VERSION"));
    lines.push(Line::styled(
        if usize::from(area.width) >= title.len() {
            title
        } else {
            "editor-pro".into()
        },
        accent,
    ));
    if full {
        lines.push(Line::styled("Your terminal. Your flow.", muted));
    }
    if area.width >= 24 && area.height >= 8 {
        lines.push(Line::default());
        for (key, label) in [
            ("i", "start editing"),
            ("Ctrl-p", "find a file"),
            ("F1", "keyboard help"),
            (":q", "quit"),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("{key:>6}"), accent),
                Span::raw("   "),
                Span::styled(
                    format!("{label:<13}"),
                    Style::default().fg(theme.foreground),
                ),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            if area.width >= 35 {
                "Start with a shortcut, or press Esc."
            } else {
                "Esc to begin"
            },
            muted,
        ));
    } else if area.width >= 18 && area.height >= 3 {
        lines.push(Line::styled("i insert | F1 help", muted));
        lines.push(Line::styled(":q quit", muted));
    }
    let height = (lines.len() as u16).min(area.height);
    let content = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height) / 2,
        area.width,
        height,
    );
    frame.render_widget(Paragraph::new(lines).centered(), content);
}
