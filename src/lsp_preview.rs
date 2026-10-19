//! Read-only reference snapshots. Loading happens on selection, never on redraw.
use super::{Item, Panel};
use crate::{
    editor::Editor,
    file, lsp,
    syntax::Syntax,
    text::{display_column, visible_text},
    theme::Theme,
    ui::{TextStyles, styled_text},
};
use lsp_types::{Position, Range};
use ratatui::{Frame, layout::Rect, style::Style, widgets::Paragraph};
use ropey::Rope;
use std::{io, path::Path};
use unicode_segmentation::UnicodeSegmentation;

pub(super) struct Preview {
    label: String,
    source: Result<Source, String>,
    pub(super) scroll: isize,
}

struct Source {
    text: Rope,
    syntax: Syntax,
    line: usize,
    column: usize,
    focus: std::ops::Range<usize>,
    origin: &'static str,
}

pub(super) fn prepare(editor: &Editor, panel: &mut Panel) {
    let context = panel.context.as_ref().unwrap();
    // A workspace can be a subproject inside a repository. Worktree .git files
    // count as boundaries too; outside Git use the language server's project.
    let start = if context.path.is_absolute() {
        context.path.parent().unwrap_or(&context.key.root)
    } else {
        &context.key.root
    };
    let root = start
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(&context.key.root);
    for row in &mut panel.rows {
        if let Item::Jump { path, position, .. } = &row.action {
            row.label = format!(
                "{}:{}:{}",
                crate::git::display_path(path.strip_prefix(root).unwrap_or(path)),
                position.line.saturating_add(1),
                position.character.saturating_add(1),
            );
        }
    }
    select(editor, panel);
}

pub(super) fn select(editor: &Editor, panel: &mut Panel) {
    let Some(row) = panel.rows.get(panel.selected) else {
        return;
    };
    let source = match &row.action {
        Item::Jump {
            path,
            position,
            range,
            ..
        } => load(editor, path, *position, *range),
        Item::VirtualJump { uri, position } => panel
            .context
            .as_ref()
            .and_then(|context| editor.virtual_buffer(uri, &context.key, context.epoch))
            .and_then(|id| editor.lsp_document(id))
            .map_or_else(
                || Err(io::Error::other(row.detail.clone())),
                |doc| {
                    let position = editor
                        .lsp
                        .virtual_sources
                        .get(&doc.buffer.instance())
                        .and_then(|source| source.protocol_text.as_ref())
                        .map_or(Ok(*position), |original| {
                            crate::lsp_source::map_position(original, doc.buffer.text(), *position)
                        })?;
                    let syntax_path = match doc.syntax.language {
                        "Java" => "source.java",
                        "Kotlin" => "source.kt",
                        _ => "source.cs",
                    };
                    Source::new(
                        doc.buffer.text().clone(),
                        Path::new(syntax_path),
                        position,
                        None,
                        "read only",
                    )
                },
            ),
        _ => Err(io::Error::other(row.detail.clone())),
    };
    panel.preview = Some(Preview {
        label: row.label.clone(),
        source: source.map_err(|error| format!("Preview unavailable: {error}")),
        scroll: 0,
    });
}

fn load(
    editor: &Editor,
    path: &Path,
    position: Position,
    range: Option<Range>,
) -> io::Result<Source> {
    let path = if editor.has_open_path(path) {
        path.to_path_buf()
    } else {
        file::absolute_path(path)?
    };
    let open = std::iter::once(&editor.document)
        .chain(editor.buffers.values().map(|slot| &slot.document))
        .find(|doc| doc.path() == Some(path.as_path()));
    let (text, origin) = if let Some(doc) = open {
        (
            doc.buffer.text().clone(),
            if doc.buffer.is_modified() {
                "unsaved buffer"
            } else {
                "buffer"
            },
        )
    } else {
        // The ordinary loader rejects symlinks, special files and oversized
        // reads. Validate text and retain only one <=2 MiB preview.
        let bytes = file::read_optional(&path)?
            .ok_or_else(|| io::Error::other("Target file is unavailable"))?;
        if bytes.len() > lsp::MAX_DOCUMENT_BYTES {
            return Err(io::Error::other("Source exceeds the 2 MiB preview limit"));
        }
        let text = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
        if text.contains('\0') {
            return Err(io::Error::other("Binary source contains NUL bytes"));
        }
        (Rope::from_str(text), "disk snapshot")
    };
    Source::new(text, &path, position, range, origin)
}

impl Source {
    fn new(
        text: Rope,
        path: &Path,
        position: Position,
        range: Option<Range>,
        origin: &'static str,
    ) -> io::Result<Self> {
        if text.len_bytes() > lsp::MAX_DOCUMENT_BYTES {
            return Err(io::Error::other("Source exceeds the 2 MiB preview limit"));
        }
        let scalar = lsp::scalar(&text, position)?;
        let range = lsp::scalar_range(&text, range.unwrap_or(Range::new(position, position)))?;
        let mut focus = text.char_to_byte(range.start)..text.char_to_byte(range.end);
        let line = position.line as usize;
        let line_byte = text.line_to_byte(line);
        let line_text = text.line(line).to_string();
        // Keep the focus on a whole grapheme, including zero-length locations.
        if let Some((byte, grapheme)) = line_text
            .grapheme_indices(true)
            .find(|(byte, grapheme)| line_byte + byte + grapheme.len() > focus.start)
        {
            focus.start = line_byte + byte;
            focus.end = focus.end.max(focus.start + grapheme.len());
        } else {
            focus.end = focus.end.max(focus.start + 1);
        }
        let column = display_column(&line_text, scalar - text.line_to_char(line));
        let mut syntax = Syntax::for_path(Some(path));
        syntax.update(&text, &[]);
        Ok(Self {
            text,
            syntax,
            line,
            column,
            focus,
            origin,
        })
    }
}

pub(super) fn render(
    frame: &mut Frame<'_>,
    preview: &mut Preview,
    area: Rect,
    scroll: usize,
    theme: &Theme,
) {
    let base = Style::default().fg(theme.foreground).bg(theme.background);
    let title = match &preview.source {
        Ok(source) => format!("Preview · {}", source.origin),
        Err(_) => "Preview".into(),
    };
    frame.render_widget(
        Paragraph::new(visible_text(&title, 0, area.width as usize)).style(base.fg(theme.command)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    frame.render_widget(
        Paragraph::new(visible_text(&preview.label, scroll, area.width as usize))
            .style(base.fg(theme.muted)),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
    let height = usize::from(area.height.saturating_sub(2));
    let source = match &preview.source {
        Ok(source) => source,
        Err(message) => {
            // Wrap errors so an unavailable target explains itself in a half pane.
            frame.render_widget(
                Paragraph::new(visible_text(message, 0, area.width as usize * height))
                    .wrap(ratatui::widgets::Wrap { trim: false })
                    .style(base.fg(theme.muted)),
                Rect::new(area.x, area.y + 2, area.width, height as u16),
            );
            return;
        }
    };
    let text = &source.text;
    let top = source
        .line
        .saturating_sub(height / 2)
        .min(text.len_lines().saturating_sub(height));
    let end = (top + height).min(text.len_lines());
    let highlights = source
        .syntax
        .highlights(text, text.line_to_byte(top)..text.line_to_byte(end));
    let styles = TextStyles {
        highlights: &highlights,
        search: &[],
        selection: Some(source.focus.clone()),
        theme,
    };
    let gutter = (text.len_lines().to_string().len() + 3).min(area.width as usize);
    let width = area.width as usize - gutter;
    let centered = source.column.saturating_sub(width / 2);
    preview.scroll = preview.scroll.max(-(centered as isize));
    let left = centered.saturating_add_signed(preview.scroll);
    for (row, index) in (top..end).enumerate() {
        let selected = index == source.line;
        let style = if selected {
            base.bg(theme.current_line)
        } else {
            base
        };
        let number = format!(
            "{}{:>digits$} ",
            if selected { '>' } else { ' ' },
            index + 1,
            digits = gutter.saturating_sub(2),
        );
        let y = area.y + 2 + row as u16;
        frame.render_widget(
            Paragraph::new(number).style(style.fg(if selected {
                theme.normal
            } else {
                theme.muted
            })),
            Rect::new(area.x, y, gutter as u16, 1),
        );
        let line = text.line(index).to_string();
        let line = line.strip_suffix('\n').map_or(line.as_str(), |line| {
            line.strip_suffix('\r').unwrap_or(line)
        });
        frame.render_widget(
            Paragraph::new(styled_text(
                line,
                text.line_to_byte(index),
                left,
                width,
                style,
                &styles,
            ))
            .style(style),
            Rect::new(area.x + gutter as u16, y, width as u16, 1),
        );
    }
}
