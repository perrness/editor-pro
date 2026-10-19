use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use std::{ops::Range, path::Path};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    editor::{Editor, Mode, View},
    mouse::{HitMap, SourceArea, Target},
    search::SearchMatch,
    syntax::{HighlightSpan, highlight_at},
    text::{cell_width, display_column, display_grapheme, visible_text},
    theme::Theme,
};

#[path = "ui_diagnostics.rs"]
mod diagnostics;

/// Startup artwork is a read-only projection, never text in the unnamed buffer.
pub fn render_splash(frame: &mut Frame<'_>, editor: &Editor, theme: &Theme) {
    let area = frame.area();
    let base = Style::default().fg(theme.foreground).bg(theme.background);
    frame.render_widget(Block::default().style(base), area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.height <= 2 {
        crate::splash::render(frame, area, theme);
        return;
    }
    let height = area.height - 2;
    crate::splash::render(frame, Rect::new(area.x, area.y, area.width, height), theme);
    let _ = render_status(
        frame,
        editor,
        theme,
        Rect::new(area.x, area.y + height, area.width, 1),
    );
    render_message(
        frame,
        editor,
        theme,
        Rect::new(area.x, area.y + height + 1, area.width, 1),
    );
}

pub fn render(frame: &mut Frame<'_>, editor: &mut Editor, theme: &Theme) {
    editor.mouse.map.clear();
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let base = Style::default().fg(theme.foreground).bg(theme.background);
    frame.render_widget(Block::default().style(base), area);
    if let Some(help) = &mut editor.help {
        crate::help::render(frame, help, theme, &mut editor.mouse.map);
        return;
    }
    if editor.disk.view.is_some() {
        crate::disk::render(frame, editor, theme);
        return;
    }
    if editor.view == View::Tree {
        crate::file_tree::render(frame, editor, theme);
        return;
    }
    if editor.view == View::Ai {
        crate::ai_view::render(frame, editor, theme);
        return;
    }
    if editor.view == View::ProjectSearch {
        crate::project_search::render(frame, editor, theme);
        return;
    }
    if editor.view == View::Recent {
        crate::navigation::render(frame, editor, theme);
        return;
    }
    if editor.view == View::Lsp {
        crate::lsp_view::render(frame, editor, theme);
        return;
    }
    if matches!(editor.view, View::Changes | View::Diff) {
        crate::git_ui::render(frame, editor, theme);
        return;
    }
    if editor.view == View::Review {
        crate::review_view::render(frame, editor, theme);
        return;
    }
    editor.document.sync_syntax();
    editor.search.refresh(&editor.document.buffer);
    let diagnostic_panel = diagnostics::panel(editor, theme, area.width, area.height);
    let height = area
        .height
        .saturating_sub(2 + diagnostic_panel.len() as u16);
    editor.viewport.height = usize::from(height);
    let buffer = &editor.document.buffer;
    let (cursor_line, cursor_column) = buffer.position();
    let cursor_text = buffer.line(cursor_line);
    let cursor_cell = display_column(&cursor_text, cursor_column);
    let gutter =
        (buffer.line_count().to_string().len() as u16 + 2).min(area.width.saturating_sub(1));
    let text_width = usize::from(area.width - gutter);

    if !editor.mouse.scrolled {
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
    }
    editor.viewport.top = editor.viewport.top.min(buffer.line_count() - 1);
    if editor.view == View::Editing {
        editor.mouse.map.source = Some(SourceArea {
            area: Rect::new(area.x, area.y, area.width, height),
            gutter,
            top: editor.viewport.top,
            left: editor.viewport.left,
        });
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
    let selection = editor.selection();
    let selected_bytes = selection.as_ref().map(|s| {
        buffer.text().char_to_byte(s.chars.start)..buffer.text().char_to_byte(s.chars.end)
    });
    let styles = TextStyles {
        highlights: &highlights,
        search: if editor.search.visible {
            &editor.search.matches
        } else {
            &[]
        },
        selection: selected_bytes,
        theme,
    };
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
                if editor.settings.relative_line_numbers && line_index != cursor_line {
                    line_index.abs_diff(cursor_line)
                } else {
                    line_index + 1
                },
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
        if gutter >= 3
            && let Some(mark) = editor.git_mark(line_index)
        {
            let (symbol, color) = match mark {
                crate::diff::Mark::Added => ("+", theme.diff_added),
                crate::diff::Mark::Modified => ("~", theme.diff_modified),
                crate::diff::Mark::Deleted => ("-", theme.diff_deleted),
            };
            frame.render_widget(
                Paragraph::new(symbol).style(style.fg(color)),
                Rect::new(area.x, area.y + row, 1, 1),
            );
        }
        let line_diagnostics = diagnostics::on_line(editor, line_index);
        let diagnostic_position = line_diagnostics
            .first()
            .and_then(|diagnostic| crate::lsp::scalar(buffer.text(), diagnostic.range.start).ok());
        if gutter >= 2
            && let Some(diagnostic) = line_diagnostics.first()
        {
            frame.render_widget(
                Paragraph::new("!").style(style.fg(diagnostics::color(diagnostic, theme))),
                Rect::new(area.x + gutter - 1, area.y + row, 1, 1),
            );
        }
        let line_selected = selection.as_ref().is_some_and(|s| {
            s.linewise && {
                let anchor_line = buffer.text().char_to_line(editor.selection_anchor.unwrap());
                (anchor_line.min(cursor_line)..=anchor_line.max(cursor_line)).contains(&line_index)
            }
        });
        let text_style = if line_selected {
            style.bg(theme.selection)
        } else {
            style
        };
        let source = buffer.line(line_index);
        let text = styled_text(
            &source,
            buffer.text().line_to_byte(line_index),
            editor.viewport.left,
            text_width,
            text_style,
            &styles,
        );
        frame.render_widget(
            Paragraph::new(text).style(text_style),
            Rect::new(area.x + gutter, area.y + row, text_width as u16, 1),
        );
        if let Some(diagnostic) = line_diagnostics.first() {
            let end = display_column(&source, usize::MAX);
            let start = end.saturating_sub(editor.viewport.left) + 2;
            if start < text_width {
                let versioned = editor
                    .current_diagnostics()
                    .is_some_and(|set| set.versioned);
                let message = diagnostics::summary(
                    diagnostic,
                    versioned,
                    line_diagnostics.len(),
                    text_width - start,
                );
                frame.render_widget(
                    Paragraph::new(message).style(style.fg(diagnostics::color(diagnostic, theme))),
                    Rect::new(
                        area.x + gutter + start as u16,
                        area.y + row,
                        (text_width - start) as u16,
                        1,
                    ),
                );
            }
        }
        if editor.view == View::Editing
            && let Some(position) = diagnostic_position
        {
            let end = display_column(&source, usize::MAX);
            let start = end.saturating_sub(editor.viewport.left) + 2;
            if start < text_width {
                editor.mouse.map.add(
                    Rect::new(
                        area.x + gutter + start as u16,
                        area.y + row,
                        (text_width - start) as u16,
                        1,
                    ),
                    Target::Diagnostic(position),
                );
            }
            if gutter >= 2 {
                editor.mouse.map.add(
                    Rect::new(area.x + gutter - 1, area.y + row, 1, 1),
                    Target::Diagnostic(position),
                );
            }
        }
    }

    if !diagnostic_panel.is_empty() {
        editor.mouse.map.add(
            Rect::new(
                area.x,
                area.y + height,
                area.width,
                area.height - height - 2,
            ),
            Target::Command("Diagnostics"),
        );
        frame.render_widget(
            Paragraph::new(diagnostic_panel).style(base.bg(theme.panel)),
            Rect::new(
                area.x,
                area.y + height,
                area.width,
                area.height - height - 2,
            ),
        );
    }

    let status_y = area.y + area.height.saturating_sub(2);
    if let Some(rect) = render_status(
        frame,
        editor,
        theme,
        Rect::new(area.x, status_y, area.width, 1),
    ) && editor.view == View::Editing
        && editor.note_edit.is_none()
    {
        editor.mouse.map.add(rect, Target::Command("ls"));
    }
    if area.height >= 2 {
        let command_area = Rect::new(area.x, status_y + 1, area.width, 1);
        if matches!(editor.mode, Mode::Command | Mode::Search) {
            let prefix = if editor.mode == Mode::Command {
                ':'
            } else if editor.search_backward {
                '?'
            } else {
                '/'
            };
            let command = format!("{prefix}{}", editor.command);
            let cells = display_column(&command, command.chars().count());
            let offset = cells.saturating_sub(usize::from(area.width) - 1);
            frame.render_widget(
                Paragraph::new(visible_text(&command, offset, usize::from(area.width))).style(base),
                command_area,
            );
            frame.set_cursor_position((area.x + (cells - offset) as u16, command_area.y));
        } else {
            render_message(frame, editor, theme, command_area);
        }
    }
    if editor.note_edit.is_some() && area.height > 1 {
        frame.render_widget(
            Paragraph::new(crate::text::visible_text(
                &format!(
                    "NOTE {} — :note save / :note cancel | {}",
                    editor.mode.label(),
                    editor.review.status
                ),
                0,
                area.width as usize,
            ))
            .style(base.fg(theme.command).bg(theme.status)),
            Rect::new(area.x, status_y, area.width, 1),
        );
    }
    if editor.view == View::Editing
        && !matches!(editor.mode, Mode::Command | Mode::Search)
        && height > 0
        && text_width > 0
        && cursor_line >= editor.viewport.top
        && cursor_line < editor.viewport.top + usize::from(height)
        && cursor_cell >= editor.viewport.left
        && cursor_cell < editor.viewport.left + text_width
    {
        let cursor = (
            area.x + gutter + (cursor_cell - editor.viewport.left) as u16,
            area.y + (cursor_line - editor.viewport.top) as u16,
        );
        crate::lsp_view::render_completion(
            frame,
            editor,
            theme,
            Rect::new(area.x, area.y, area.width, height),
            cursor,
        );
        frame.set_cursor_position(cursor);
    }
    if let View::Buffers { selected } = editor.view {
        render_buffers(
            frame,
            editor,
            theme,
            Rect::new(area.x, area.y, area.width, height),
            selected,
        );
        if area.height >= 2 {
            frame.render_widget(
                Paragraph::new(visible_text(
                    "Buffers: ↑/↓ or j/k  Enter open  Esc return",
                    0,
                    usize::from(area.width),
                ))
                .style(base),
                Rect::new(area.x, status_y + 1, area.width, 1),
            );
        }
    }
    if editor.view == View::Files
        && let Some(picker) = &editor.picker
    {
        render_picker(
            frame,
            picker,
            theme,
            Rect::new(area.x, area.y, area.width, height),
            &mut editor.mouse.map,
        );
        if area.height >= 2 && !editor.message_is_error {
            frame.render_widget(
                Paragraph::new(visible_text(
                    "Files: type to filter  ↑/↓ select  Enter open  Esc return",
                    0,
                    usize::from(area.width),
                ))
                .style(base),
                Rect::new(area.x, status_y + 1, area.width, 1),
            );
        }
    }
    crate::lsp_view::render_mouse_hover(frame, editor, theme);
}

fn render_message(frame: &mut Frame<'_>, editor: &Editor, theme: &Theme, area: Rect) {
    let color = if editor.message_is_error || editor.git.problem().is_some() {
        theme.error
    } else {
        theme.muted
    };
    let message = if editor.message_is_error {
        &editor.message
    } else {
        editor.git.problem().unwrap_or(&editor.message)
    };
    frame.render_widget(
        Paragraph::new(visible_text(message, 0, usize::from(area.width)))
            .style(Style::default().fg(color).bg(theme.background)),
        area,
    );
}

/// Clip styled UI text by terminal cells, sanitizing paths and other external text.
pub(crate) fn clipped_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut remaining = width;
    let mut visible = Vec::new();
    for mut span in spans {
        if remaining == 0 {
            break;
        }
        let text = visible_text(&span.content, 0, remaining);
        remaining = remaining.saturating_sub(display_column(&text, text.chars().count()));
        span.content = text.into();
        visible.push(span);
    }
    Line::from(visible)
}

pub(crate) fn render_panel_header(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    detail: &str,
    meta: &str,
    theme: &Theme,
) {
    let width = usize::from(area.width);
    let style = Style::default().fg(theme.foreground).bg(theme.status);
    let title = format!(" {title} ");
    let meta = if width >= 50 {
        visible_text(&format!(" {meta} "), 0, width / 3)
    } else {
        String::new()
    };
    let meta_width = display_column(&meta, meta.chars().count());
    let detail_width = width.saturating_sub(title.len() + meta_width + 2);
    let detail_cells = display_column(detail, detail.chars().count());
    let detail = visible_text(
        detail,
        detail_cells.saturating_sub(detail_width),
        detail_width,
    );
    frame.render_widget(
        Paragraph::new(clipped_line(
            vec![
                Span::styled(
                    title,
                    style
                        .fg(theme.background)
                        .bg(theme.normal)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {detail}"), style.fg(theme.type_name)),
            ],
            width.saturating_sub(meta_width),
        ))
        .style(style),
        area,
    );
    if meta_width > 0 {
        frame.render_widget(
            Paragraph::new(meta).style(style.fg(theme.modified).bg(theme.current_line)),
            Rect::new(
                area.right() - meta_width as u16,
                area.y,
                meta_width as u16,
                1,
            ),
        );
    }
}

/// Filename-only decoration: no file reads, parser work, or special icon font.
pub(crate) fn file_badge(path: &Path, theme: &Theme) -> Span<'static> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let (label, color) = match extension.as_str() {
        "rs" => ("rs", theme.number),
        "kt" | "kts" => ("kt", theme.command),
        "java" => ("jv", theme.number),
        "cs" => ("cs", theme.command),
        "fs" | "fsx" => ("fs", theme.command),
        "js" | "jsx" => ("js", theme.modified),
        "ts" | "tsx" => ("ts", theme.normal),
        "py" => ("py", theme.modified),
        "rb" | "go" | "c" | "h" | "cpp" | "hpp" | "swift" => ("<>", theme.type_name),
        "sh" | "bash" | "zsh" | "fish" => ("sh", theme.insert),
        "md" | "markdown" => ("md", theme.type_name),
        "txt" | "rst" => ("tx", theme.type_name),
        "json" | "jsonc" | "toml" | "yaml" | "yml" | "xml" | "ini" | "conf" => {
            ("{}", theme.command)
        }
        "html" | "css" | "scss" | "vue" | "svelte" => ("<>", theme.error),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => ("im", theme.insert),
        "lock" => ("==", theme.muted),
        _ if name.starts_with('.') || matches!(name.as_str(), "dockerfile" | "makefile") => {
            ("{}", theme.command)
        }
        _ if name.starts_with("readme") || name.starts_with("license") => ("md", theme.type_name),
        _ => ("--", theme.muted),
    };
    Span::styled(format!("[{label}] "), Style::default().fg(color))
}

fn render_picker(
    frame: &mut Frame<'_>,
    picker: &crate::picker::FilePicker,
    theme: &Theme,
    area: Rect,
    hits: &mut HitMap,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let style = Style::default().fg(theme.foreground).bg(theme.panel);
    frame.render_widget(Block::default().style(style), area);
    if area.height > 1 {
        let summary = format!(
            "{}/{}{}{} ({} unreadable)",
            picker.match_count(),
            picker.item_count(),
            if picker.working() { " …" } else { "" },
            if picker.limited { " [100k limit]" } else { "" },
            picker.errors
        );
        render_panel_header(
            frame,
            Rect::new(area.x, area.y, area.width, 1),
            "Files",
            &picker.root.display().to_string(),
            &summary,
            theme,
        );
    }
    let prompt_row = u16::from(area.height > 1);
    let prompt = format!("> {}", picker.query);
    let cells = display_column(&prompt, prompt.chars().count());
    let left = cells.saturating_sub(usize::from(area.width).saturating_sub(1));
    frame.render_widget(
        Paragraph::new(visible_text(&prompt, left, usize::from(area.width)))
            .style(style.fg(theme.normal)),
        Rect::new(area.x, area.y + prompt_row, area.width, 1),
    );
    frame.set_cursor_position((area.x + (cells - left) as u16, area.y + prompt_row));
    let rows = area.height.saturating_sub(prompt_row + 1);
    let top = picker
        .selected
        .saturating_sub(u32::from(rows.saturating_sub(1)));
    for row in 0..rows {
        let index = top + u32::from(row);
        let Some(candidate) = picker.candidate(index) else {
            break;
        };
        let row_style = if index == picker.selected {
            style.bg(theme.selection).add_modifier(Modifier::BOLD)
        } else {
            style
        };
        hits.add(
            Rect::new(area.x, area.y + prompt_row + 1 + row, area.width, 1),
            Target::File(index),
        );
        frame.render_widget(
            Paragraph::new(clipped_line(
                vec![
                    Span::styled(
                        if index == picker.selected {
                            "▎ "
                        } else {
                            "  "
                        },
                        Style::default().fg(theme.normal),
                    ),
                    file_badge(Path::new(&candidate.display), theme),
                    Span::raw(candidate.display.clone()),
                ],
                usize::from(area.width),
            ))
            .style(row_style),
            Rect::new(area.x, area.y + prompt_row + 1 + row, area.width, 1),
        );
    }
    if rows > 0 && picker.match_count() == 0 {
        let message = if picker.working() {
            "Finding files…"
        } else {
            "No matching files"
        };
        frame.render_widget(
            Paragraph::new(visible_text(message, 0, usize::from(area.width)))
                .style(style.fg(theme.muted)),
            Rect::new(area.x, area.y + prompt_row + 1, area.width, 1),
        );
    }
}

fn render_buffers(
    frame: &mut Frame<'_>,
    editor: &mut Editor,
    theme: &Theme,
    area: Rect,
    selected: usize,
) {
    let style = Style::default().fg(theme.foreground).bg(theme.panel);
    frame.render_widget(Block::default().style(style), area);
    let infos = editor.buffer_infos();
    let header = u16::from(area.height > 1);
    if header > 0 {
        render_panel_header(
            frame,
            Rect::new(area.x, area.y, area.width, 1),
            "Buffers",
            "Open documents",
            &format!(
                "{} open · {} dirty",
                infos.len(),
                editor.dirty_buffer_count()
            ),
            theme,
        );
    }
    let rows = usize::from(area.height - header);
    let top = selected.saturating_sub(rows.saturating_sub(1));
    for (row, info) in infos.iter().skip(top).take(rows).enumerate() {
        editor.mouse.map.add(
            Rect::new(area.x, area.y + header + row as u16, area.width, 1),
            Target::Buffer(info.id),
        );
        let style = if row + top == selected {
            style.bg(theme.selection).add_modifier(Modifier::BOLD)
        } else {
            style
        };
        frame.render_widget(
            Paragraph::new(clipped_line(
                vec![
                    Span::styled(
                        if row + top == selected { "▎ " } else { "  " },
                        Style::default().fg(theme.normal),
                    ),
                    Span::styled(
                        format!("{} {:>3} ", if info.active { '%' } else { ' ' }, info.id),
                        Style::default().fg(theme.normal),
                    ),
                    Span::styled(
                        if info.modified { "+" } else { " " },
                        Style::default().fg(theme.modified),
                    ),
                    Span::styled(
                        if info.external_changed { "! " } else { "  " },
                        Style::default().fg(theme.error),
                    ),
                    file_badge(Path::new(&info.name), theme),
                    Span::raw(info.name.clone()),
                ],
                usize::from(area.width),
            ))
            .style(style),
            Rect::new(area.x, area.y + header + row as u16, area.width, 1),
        );
    }
}

struct TextStyles<'a> {
    highlights: &'a [HighlightSpan],
    search: &'a [SearchMatch],
    selection: Option<Range<usize>>,
    theme: &'a Theme,
}

fn styled_text(
    line: &str,
    byte_start: usize,
    left: usize,
    width: usize,
    base: Style,
    styles: &TextStyles<'_>,
) -> Line<'static> {
    let mut spans = Vec::new();
    let mut column = 0;
    let right = left.saturating_add(width);
    let highlights: Vec<_> = styles
        .highlights
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
            let mut style = highlight_at(&highlights, byte_start + byte)
                .map_or(base, |kind| base.fg(styles.theme.highlight(kind)));
            let match_index = styles
                .search
                .partition_point(|m| m.bytes.end <= byte_start + byte);
            if styles
                .search
                .get(match_index)
                .is_some_and(|m| m.bytes.contains(&(byte_start + byte)))
            {
                style = style.bg(styles.theme.search);
            }
            if styles
                .selection
                .as_ref()
                .is_some_and(|s| s.contains(&(byte_start + byte)))
            {
                style = style.bg(styles.theme.selection);
            }
            spans.push(Span::styled(text, style));
        }
        column = end;
    }
    if column >= left
        && column < right
        && styles
            .selection
            .as_ref()
            .is_some_and(|s| s.contains(&(byte_start + line.len())))
    {
        spans.push(Span::styled(" ", base.bg(styles.theme.selection)));
    }
    Line::from(spans)
}

fn render_status(
    frame: &mut Frame<'_>,
    editor: &Editor,
    theme: &Theme,
    area: Rect,
) -> Option<Rect> {
    let style = Style::default().fg(theme.foreground).bg(theme.status);
    frame.render_widget(Block::default().style(style), area);
    let label = if area.width >= 28 {
        editor.mode.label()
    } else {
        match editor.mode {
            Mode::Normal => "N",
            Mode::Insert => "I",
            Mode::Visual => "V",
            Mode::VisualLine => "VL",
            Mode::Command => ":",
            Mode::Search => "/",
        }
    };
    let mode = format!(" {label} ");
    let identity = if area.width >= 40 {
        format!(" [B{}]", editor.active_buffer_id)
    } else {
        String::new()
    };
    let buffer_area = (!identity.is_empty())
        .then(|| Rect::new(area.x + mode.len() as u16, area.y, identity.len() as u16, 1));
    let modified = if editor.document.buffer.is_read_only() {
        match editor.document.virtual_description() {
            Some("Decompiled source") => " [RO] Decompiled",
            Some("Dependency source") => " [RO] Source",
            _ => " [RO]",
        }
    } else if editor.document.buffer.is_modified() {
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
    let percent = (line + 1) * 100 / editor.document.buffer.line_count();
    let syntax = &editor.document.syntax;
    let language = format!(
        "{}{}",
        syntax.language,
        if syntax.fallback.is_some() {
            "[plain]"
        } else {
            ""
        }
    );
    let pending = editor.pending_label();
    let pending = if pending.is_empty() {
        String::new()
    } else {
        format!("{pending} ")
    };
    let mut right = Vec::new();
    let separator = Span::styled(" | ", style.fg(theme.border));
    if area.width >= 20 {
        right.push(Span::styled(
            format!(" {pending}"),
            style.fg(theme.modified).add_modifier(Modifier::BOLD),
        ));
    }
    if area.width >= 100 {
        right.push(Span::styled(
            format!("{}buf ", editor.buffer_count()),
            style.fg(theme.muted),
        ));
        right.push(Span::styled(
            format!("{}dirty", editor.dirty_buffer_count()),
            style.fg(if editor.dirty_buffer_count() > 0 {
                theme.modified
            } else {
                theme.muted
            }),
        ));
        right.push(separator.clone());
    }
    if area.width >= 70 {
        right.push(Span::styled(
            language,
            style.fg(theme.type_name).add_modifier(Modifier::BOLD),
        ));
        right.push(Span::styled(
            format!(" UTF-8 {}", editor.document.buffer.newline_name()),
            style.fg(theme.muted),
        ));
        if area.width >= 100 {
            right.push(separator.clone());
        }
    }
    if area.width >= 20 {
        let position = if area.width >= 40 {
            format!(" {}:{} {percent}% ", line + 1, column)
        } else {
            format!(" {}:{} ", line + 1, column)
        };
        right.push(Span::styled(
            position,
            style
                .fg(theme.normal)
                .bg(theme.panel)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if area.width >= 70 && editor.git.visible() {
        let git = format!(
            "{}→buffer{} {}",
            editor.git.comparison.label(),
            if editor.document.buffer.is_modified() {
                "+"
            } else {
                ""
            },
            if editor.git.loading {
                "[loading/stale]".into()
            } else if editor.git.problem().is_some() {
                "[Git error]".into()
            } else {
                editor.git.summary()
            }
        );
        let color = if editor.git.problem().is_some() {
            theme.error
        } else if editor.git.loading {
            theme.modified
        } else {
            theme.insert
        };
        right.insert(0, separator.clone());
        right.insert(
            0,
            Span::styled(
                format!(" {}", visible_text(&git, 0, usize::from(area.width) / 3)),
                style.fg(color),
            ),
        );
    }
    let width = usize::from(area.width);
    let lsp = editor.lsp_summary();
    if area.width >= 70 && !lsp.is_empty() {
        right.insert(0, separator);
        right.insert(
            0,
            Span::styled(
                format!(" {}", visible_text(&lsp, 0, 30)),
                style.fg(theme.command),
            ),
        );
    }
    let right = clipped_line(
        right,
        width.saturating_sub(mode.len() + identity.len() + modified.len() + external.len() + 3),
    );
    let right_cells = right.width();
    let name_width = width.saturating_sub(
        mode.len() + identity.len() + modified.len() + external.len() + right_cells + 2,
    );
    let name = editor
        .document
        .path()
        .and_then(|path| editor.git.relative_path(path))
        .map_or_else(
            || editor.document.display_name(),
            |path| path.display().to_string(),
        );
    let mut name = name.as_str();
    let mut name_cells = display_column(name, name.chars().count());
    // Keep directory names whole when shortening a file path. Only clip within
    // the filename if it cannot fit by itself; non-file labels keep their layout.
    while name_cells > name_width
        && editor.document.path().is_some()
        && let Some((_, suffix)) = name.split_once(std::path::MAIN_SEPARATOR)
    {
        name = suffix;
        name_cells = display_column(name, name.chars().count());
    }
    let name = visible_text(name, name_cells.saturating_sub(name_width), name_width);
    let spans = vec![
        Span::styled(
            mode,
            Style::default()
                .fg(theme.background)
                .bg(theme.mode_color(editor.mode))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(identity, style.fg(theme.normal)),
        Span::styled(format!(" {name}"), style.add_modifier(Modifier::BOLD)),
        Span::styled(modified, style.fg(theme.modified)),
        Span::styled(external, style.fg(theme.error)),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
    if right_cells > 0 {
        let right_width = right_cells.min(width) as u16;
        frame.render_widget(
            Paragraph::new(right).style(style),
            Rect::new(area.right() - right_width, area.y, right_width, 1),
        );
    }
    buffer_area
}
