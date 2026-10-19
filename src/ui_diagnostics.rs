//! Read-only, viewport-bounded diagnostic messages for the source renderer.
use lsp_types::{Diagnostic, DiagnosticSeverity};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    editor::{Editor, View},
    text::{cell_width, display_column, display_grapheme, visible_text},
    theme::Theme,
};

fn severity(diagnostic: &Diagnostic) -> (u8, &'static str) {
    match diagnostic.severity {
        Some(DiagnosticSeverity::WARNING) => (1, "Warning"),
        Some(DiagnosticSeverity::INFORMATION) => (2, "Info"),
        Some(DiagnosticSeverity::HINT) => (3, "Hint"),
        _ => (0, "Error"),
    }
}

pub(super) fn color(diagnostic: &Diagnostic, theme: &Theme) -> Color {
    match severity(diagnostic).0 {
        1 => theme.modified,
        2 => theme.normal,
        3 => theme.muted,
        _ => theme.error,
    }
}

pub(super) fn on_line(editor: &Editor, line: usize) -> Vec<&Diagnostic> {
    let Some(set) = editor.visible_diagnostics() else {
        return Vec::new();
    };
    let mut items: Vec<_> = set
        .items
        .iter()
        .filter(|item| {
            let start = item.range.start;
            let end = item.range.end;
            // LSP ranges exclude their end. Keep zero-width diagnostics visible.
            line == start.line as usize
                || (line > start.line as usize
                    && (line < end.line as usize
                        || (line == end.line as usize && end.character > 0)))
        })
        .collect();
    items.sort_by_key(|item| (severity(item).0, item.range.start));
    items
}

pub(super) fn summary(
    diagnostic: &Diagnostic,
    versioned: bool,
    count: usize,
    width: usize,
) -> String {
    let prefix = format!(
        "{}{}{}: ",
        severity(diagnostic).1,
        if versioned { "" } else { "?" },
        if count > 1 {
            format!(" (+{})", count - 1)
        } else {
            String::new()
        }
    );
    let mut text = String::new();
    let mut cells = 0;
    for grapheme in prefix
        .graphemes(true)
        .chain(diagnostic.message.graphemes(true))
    {
        // Flatten server line breaks for inline text and subsequent word wrapping.
        let grapheme = if grapheme.chars().all(char::is_whitespace) {
            " "
        } else {
            grapheme
        };
        let size = cell_width(grapheme, cells);
        if cells + size > width {
            return format!("{}…", visible_text(&text, 0, width.saturating_sub(1)));
        }
        text.push_str(&display_grapheme(grapheme, cells));
        cells += size;
    }
    text
}

pub(super) fn panel(editor: &Editor, theme: &Theme, width: u16, height: u16) -> Vec<Line<'static>> {
    // Retain at least one source row and both status/prompt rows on small terminals.
    let max_rows = usize::from(height.saturating_sub(3).min(4));
    if editor.view != View::Editing || width == 0 || max_rows < 2 {
        return Vec::new();
    }
    let line = editor.document.buffer.position().0;
    let items = on_line(editor, line);
    if items.is_empty() {
        return Vec::new();
    }
    let versioned = editor
        .visible_diagnostics()
        .is_some_and(|set| set.versioned);
    let width = usize::from(width);
    let header = format!(
        "Diagnostics{} line {} ({}) — :Diagnostics for full text",
        if versioned { "" } else { "? unverified" },
        line + 1,
        items.len(),
    );
    let mut rows = vec![Line::styled(
        visible_text(&header, 0, width),
        Style::default()
            .fg(theme.normal)
            .add_modifier(Modifier::BOLD),
    )];
    for item in items {
        let remaining = max_rows - rows.len();
        if remaining == 0 {
            break;
        }
        let text = summary(item, versioned, 1, width * remaining);
        let style = Style::default().fg(color(item, theme));
        rows.extend(
            wrap(&text, width, remaining)
                .into_iter()
                .map(|row| Line::styled(row, style)),
        );
    }
    rows
}

fn wrap(text: &str, width: usize, max_rows: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut cells = 0;
    for word in text.split_whitespace() {
        if cells > 0 {
            if cells + 1 + display_column(word, usize::MAX) > width {
                rows.push(std::mem::take(&mut row));
                cells = 0;
            } else {
                row.push(' ');
                cells += 1;
            }
        }
        for grapheme in word.graphemes(true) {
            let size = cell_width(grapheme, cells);
            if cells > 0 && cells + size > width {
                rows.push(std::mem::take(&mut row));
                cells = 0;
            }
            if rows.len() == max_rows {
                let last = rows.last_mut().unwrap();
                *last = format!("{}…", visible_text(last, 0, width - 1));
                return rows;
            }
            if size > width {
                row.push('�');
                cells += 1;
            } else {
                row.push_str(grapheme);
                cells += size;
            }
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}
