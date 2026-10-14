use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    editor::{Editor, Mode},
    syntax::{HighlightSpan, highlight_at},
    text::{cell_width, display_column, display_grapheme, visible_text},
    theme::Theme,
};

pub fn render(frame: &mut Frame<'_>, editor: &mut Editor, theme: &Theme) {
    editor.document.sync_syntax();
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let base = Style::default().fg(theme.foreground).bg(theme.background);
    frame.render_widget(Block::default().style(base), area);
    let height = area.height.saturating_sub(2);
    editor.viewport.height = usize::from(height);
    let buffer = &editor.document.buffer;
    let (cursor_line, cursor_column) = buffer.position();
    let cursor_text = buffer.line(cursor_line);
    let cursor_cell = display_column(&cursor_text, cursor_column);
    let gutter =
        (buffer.line_count().to_string().len() as u16 + 2).min(area.width.saturating_sub(1));
    let text_width = usize::from(area.width - gutter);

    editor.viewport.top = editor.viewport.top.min(cursor_line);
    if height > 0 && cursor_line >= editor.viewport.top + usize::from(height) {
        editor.viewport.top = cursor_line + 1 - usize::from(height);
    }
    // Keep an entire wide grapheme visible when there is enough room.
    let grapheme = cursor_text
        .char_indices()
        .nth(cursor_column)
        .map(|(byte, _)| &cursor_text[byte..])
        .and_then(|tail| tail.graphemes(true).next())
        .unwrap_or(" ");
    let cursor_width = cell_width(grapheme, cursor_cell).min(text_width).max(1);
    editor.viewport.left = editor.viewport.left.min(cursor_cell);
    if cursor_cell + cursor_width > editor.viewport.left + text_width {
        editor.viewport.left = cursor_cell + cursor_width - text_width;
    }

    let first_byte = buffer.text().line_to_byte(editor.viewport.top);
    let end_line = (editor.viewport.top + usize::from(height)).min(buffer.line_count());
    let last_byte = if end_line == buffer.line_count() {
        buffer.text().len_bytes()
    } else {
        buffer.text().line_to_byte(end_line)
    };
    let highlights = editor
        .document
        .syntax
        .highlights(buffer.text(), first_byte..last_byte);
    for row in 0..height {
        let line_index = editor.viewport.top + usize::from(row);
        let rect = Rect::new(area.x, area.y + row, area.width, 1);
        if line_index >= buffer.line_count() {
            frame.render_widget(Paragraph::new("~").style(base.fg(theme.muted)), rect);
            continue;
        }
        let style = if line_index == cursor_line {
            base.bg(theme.current_line)
        } else {
            base
        };
        frame.render_widget(Block::default().style(style), rect);
        if gutter > 0 {
            let number = format!(
                "{:>width$} ",
                line_index + 1,
                width = usize::from(gutter - 1)
            );
            let color = if line_index == cursor_line {
                theme.normal
            } else {
                theme.muted
            };
            frame.render_widget(
                Paragraph::new(number).style(style.fg(color)),
                Rect::new(area.x, area.y + row, gutter, 1),
            );
        }
        let text = styled_text(
            &buffer.line(line_index),
            buffer.text().line_to_byte(line_index),
            editor.viewport.left,
            text_width,
            &highlights,
            style,
            theme,
        );
        frame.render_widget(
            Paragraph::new(text).style(style),
            Rect::new(area.x + gutter, area.y + row, text_width as u16, 1),
        );
    }

    let status_y = area.y + area.height.saturating_sub(2);
    render_status(
        frame,
        editor,
        theme,
        Rect::new(area.x, status_y, area.width, 1),
    );
    if area.height >= 2 {
        let command_area = Rect::new(area.x, status_y + 1, area.width, 1);
        if editor.mode == Mode::Command {
            let command = format!(":{}", editor.command);
            let cells = display_column(&command, command.chars().count());
            let offset = cells.saturating_sub(usize::from(area.width) - 1);
            frame.render_widget(
                Paragraph::new(visible_text(&command, offset, usize::from(area.width))).style(base),
                command_area,
            );
            frame.set_cursor_position((area.x + (cells - offset) as u16, command_area.y));
        } else {
            let color = if editor.message_is_error {
                theme.error
            } else {
                theme.muted
            };
            frame.render_widget(
                Paragraph::new(visible_text(&editor.message, 0, usize::from(area.width)))
                    .style(base.fg(color)),
                command_area,
            );
        }
    }
    if editor.mode != Mode::Command && height > 0 && text_width > 0 {
        frame.set_cursor_position((
            area.x + gutter + (cursor_cell - editor.viewport.left) as u16,
            area.y + (cursor_line - editor.viewport.top) as u16,
        ));
    }
}

fn styled_text(
    line: &str,
    byte_start: usize,
    left: usize,
    width: usize,
    highlights: &[HighlightSpan],
    base: Style,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = Vec::new();
    let mut column = 0;
    let right = left.saturating_add(width);
    let highlights: Vec<_> = highlights
        .iter()
        .filter(|h| h.bytes.end > byte_start && h.bytes.start < byte_start + line.len())
        .cloned()
        .collect();
    for (byte, grapheme) in line.grapheme_indices(true) {
        if column >= right {
            break;
        }
        let end = column + cell_width(grapheme, column);
        if end > left {
            let text = if column >= left && end <= right {
                display_grapheme(grapheme, column)
            } else {
                " ".repeat(end.min(right) - column.max(left))
            };
            let style = highlight_at(&highlights, byte_start + byte)
                .map_or(base, |kind| base.fg(theme.highlight(kind)));
            spans.push(Span::styled(text, style));
        }
        column = end;
    }
    Line::from(spans)
}

fn render_status(frame: &mut Frame<'_>, editor: &Editor, theme: &Theme, area: Rect) {
    let style = Style::default().fg(theme.foreground).bg(theme.status);
    frame.render_widget(Block::default().style(style), area);
    let mode = format!(" {} ", editor.mode.label());
    let modified = if editor.document.buffer.is_modified() {
        " [+]"
    } else {
        ""
    };
    let external = if editor.document.external_changed {
        " !DISK"
    } else {
        ""
    };
    let (line, column) = editor.document.buffer.position();
    let text = editor.document.buffer.line(line);
    let byte = text
        .char_indices()
        .nth(column)
        .map_or(text.len(), |(i, _)| i);
    let column = text[..byte].graphemes(true).count() + 1;
    let right = if area.width >= 55 {
        format!(
            " {}:{}  UTF-8 {} ",
            line + 1,
            column,
            editor.document.buffer.newline_name()
        )
    } else if area.width >= 25 {
        format!(" {}:{} ", line + 1, column)
    } else {
        String::new()
    };
    let width = usize::from(area.width);
    let name_width =
        width.saturating_sub(mode.len() + modified.len() + external.len() + right.len() + 2);
    let name = editor.document.display_name();
    let name_cells = display_column(&name, name.chars().count());
    let name = visible_text(&name, name_cells.saturating_sub(name_width), name_width);
    let spans = vec![
        Span::styled(
            mode,
            Style::default()
                .fg(theme.background)
                .bg(theme.mode_color(editor.mode))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {name}")),
        Span::styled(modified, style.fg(theme.modified)),
        Span::styled(external, style.fg(theme.error)),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
    if !right.is_empty() {
        let right_width = right.len().min(width) as u16;
        frame.render_widget(
            Paragraph::new(right).style(style),
            Rect::new(area.right() - right_width, area.y, right_width, 1),
        );
    }
}
