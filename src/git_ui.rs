//! Rendering for the two concrete Git views; no general window framework.
use crate::{
    diff::{Diff, Kind, Row},
    editor::{Editor, Mode, View},
    text::{display_column, visible_text},
    theme::Theme,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

fn line(frame: &mut Frame<'_>, area: Rect, row: u16, text: &str, style: Style) {
    if row < area.height {
        frame.render_widget(
            Paragraph::new(visible_text(text, 0, usize::from(area.width))).style(style),
            Rect::new(area.x, area.y + row, area.width, 1),
        );
    }
}
pub fn render(frame: &mut Frame<'_>, editor: &mut Editor, theme: &Theme) {
    let area = frame.area();
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(2));
    if editor.view == View::Changes {
        line(
            frame,
            body,
            0,
            &if editor.git.comparison.is_review() {
                format!(
                    "Changes — {}; A/M/D/R: snapshot delta; B+: unsaved buffer",
                    editor.git.comparison.description()
                )
            } else {
                "Changes — XY: index/disk; B+: unsaved buffer".into()
            },
            style.fg(theme.normal),
        );
        line(
            frame,
            body,
            1,
            &editor.git.summary(),
            style.fg(if editor.git.ready() {
                theme.muted
            } else {
                theme.modified
            }),
        );
        if let Some(snapshot) = &editor.git.snapshot {
            line(
                frame,
                body,
                2,
                &format!(
                    "{} | {}",
                    editor.review_summary(),
                    crate::git::display_path(&snapshot.repository.root)
                ),
                style.fg(theme.muted),
            );
        }
        let files = editor.changed_files();
        let progress = editor.review_progress();
        let empty = crate::review_view::FileProgress::default();
        let height = body.height.saturating_sub(3) as usize;
        let top = editor.git.selected.saturating_sub(height.saturating_sub(1));
        for (index, file) in files.iter().enumerate().skip(top).take(height) {
            let progress = progress.get(&file.path).unwrap_or(&empty);
            line(
                frame,
                body,
                (index - top + 3) as u16,
                &format!(
                    "[{}] {}",
                    if editor.review.store.is_some() {
                        progress.label()
                    } else {
                        "review unavailable".into()
                    },
                    file.label
                ),
                if index == editor.git.selected {
                    style.bg(theme.selection)
                } else {
                    style.fg(if progress.attention > 0 {
                        theme.error
                    } else if progress.state == Some(crate::review::State::Reviewed) {
                        theme.diff_added
                    } else {
                        theme.foreground
                    })
                },
            );
        }
        if files.is_empty() && editor.git.ready() {
            line(
                frame,
                body,
                3,
                editor
                    .git
                    .problem()
                    .unwrap_or(if editor.git.comparison.is_review() {
                        "No changes from the selected review baseline"
                    } else {
                        "No disk/index changes or unsaved buffers"
                    }),
                style.fg(theme.muted),
            );
        }
    } else {
        render_diff(frame, editor, theme, body);
    }
    let status = if editor.view == View::Changes {
        "CHANGES — Enter source/history  d diff  r refresh  Esc return".to_string()
    } else {
        let view = editor.git.diff_view.as_ref();
        let layout = if view.is_some_and(|v| v.effective_paired) {
            "paired"
        } else {
            "inline"
        };
        format!(
            "DIFF {layout} — ]/[ hunks  n/p files  Enter source  Esc return  Tab layout  F/R reviewed  A attention  :review help"
        )
    };
    if area.height > 1 {
        line(
            frame,
            area,
            area.height - 2,
            &status,
            style.bg(theme.status),
        );
    }
    if editor.mode == Mode::Command {
        let command = format!(":{}", editor.command);
        let cells = display_column(&command, command.chars().count());
        let offset = cells.saturating_sub(usize::from(area.width).saturating_sub(1));
        line(
            frame,
            area,
            area.height - 1,
            &visible_text(&command, offset, usize::from(area.width)),
            style,
        );
        frame.set_cursor_position((
            area.x + (cells - offset).min(usize::from(area.width.saturating_sub(1))) as u16,
            area.bottom() - 1,
        ));
    } else {
        let help = editor.message.as_str();
        line(
            frame,
            area,
            area.height - 1,
            help,
            style.fg(if editor.message_is_error {
                theme.error
            } else {
                theme.muted
            }),
        );
    }
}
fn render_diff(frame: &mut Frame<'_>, editor: &mut Editor, theme: &Theme, area: Rect) {
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let state = if editor.git.loading {
        "STALE / loading".into()
    } else if !editor.git.message.is_empty() {
        editor.git.message.clone()
    } else {
        editor.git.summary()
    };
    line(
        frame,
        area,
        2,
        &state,
        style.fg(if editor.git.ready() {
            theme.muted
        } else {
            theme.modified
        }),
    );
    let review = editor
        .git
        .diff_view
        .as_ref()
        .map(|v| editor.review_file_label(&v.path))
        .unwrap_or_default();
    let selected_hunk = editor
        .git
        .diff_view
        .as_ref()
        .and_then(|v| {
            editor
                .git
                .diff()
                .and_then(|d| d.attachment(v.selected, "hunk").ok())
        })
        .map(|(a, _)| {
            editor
                .review
                .store
                .as_ref()
                .and_then(|s| {
                    s.records.iter().find(|r| {
                        r.attachment == a
                            && editor.git.diff().is_some_and(|d| r.matches_diff(d))
                            && r.state.is_some()
                            && editor.review_freshness(r) == crate::review::Freshness::Fresh
                    })
                })
                .and_then(|r| r.state)
                .unwrap_or(crate::review::State::Unreviewed)
                .label()
        })
        .unwrap_or("no unique hunk");
    line(
        frame,
        area,
        3,
        &format!(
            "File {review} | Hunk {selected_hunk} | {}",
            editor.review.status
        ),
        style.fg(if review.starts_with("reviewed") {
            theme.diff_added
        } else {
            theme.modified
        }),
    );
    let Some(snapshot) = &editor.git.snapshot else {
        line(
            frame,
            area,
            0,
            &if editor.git.comparison == crate::git::Comparison::Staged {
                "Diff HEAD → index (staged)".into()
            } else {
                format!(
                    "Diff {} → working buffer/disk",
                    editor.git.comparison.label()
                )
            },
            style.fg(theme.normal),
        );
        return;
    };
    let diff = match &snapshot.diff {
        Ok(diff) => diff,
        Err(error) => {
            line(
                frame,
                area,
                0,
                &format!("Diff {}", snapshot.comparison.label()),
                style.fg(theme.normal),
            );
            line(frame, area, 3, error, style.fg(theme.error));
            return;
        }
    };
    line(
        frame,
        area,
        0,
        &format!("OLD {}", diff.left_label),
        style.fg(theme.diff_deleted),
    );
    line(
        frame,
        area,
        1,
        &format!("NEW {}", diff.right_label),
        style.fg(theme.diff_added),
    );
    let Some(view) = &mut editor.git.diff_view else {
        return;
    };
    let paired = view.paired && area.width >= 100;
    let layout_changed = view.effective_paired != paired;
    view.effective_paired = paired;
    view.height = area.height.saturating_sub(4) as usize;
    if diff.rows.is_empty() {
        line(
            frame,
            area,
            4,
            "No textual changes (empty files and mode-only changes have no hunks)",
            style.fg(theme.muted),
        );
        return;
    }
    view.selected = view.selected.min(diff.rows.len() - 1);
    let selected = if paired {
        diff.pair_index(view.selected)
    } else {
        view.selected
    };
    if layout_changed {
        view.top = selected.saturating_sub(view.height / 2);
    }
    view.top = view.top.min(selected);
    if view.height > 0 && selected >= view.top + view.height {
        view.top = selected + 1 - view.height;
    }
    for row in 0..view.height {
        let index = view.top + row;
        let rect = Rect::new(area.x, area.y + 4 + row as u16, area.width, 1);
        let base = if index == selected {
            style.bg(theme.selection)
        } else {
            style
        };
        frame.render_widget(Block::default().style(base), rect);
        if paired {
            let Some(pair) = diff.pairs.get(index) else {
                break;
            };
            let width = (area.width - 1) / 2;
            render_side(
                frame,
                diff,
                pair.left,
                false,
                Rect::new(rect.x, rect.y, width, 1),
                view.left,
                base,
                theme,
            );
            frame.render_widget(
                Paragraph::new("│").style(style.fg(theme.muted)),
                Rect::new(rect.x + width, rect.y, 1, 1),
            );
            render_side(
                frame,
                diff,
                pair.right,
                true,
                Rect::new(rect.x + width + 1, rect.y, rect.width - width - 1, 1),
                view.left,
                base,
                theme,
            );
        } else if let Some(row) = diff.rows.get(index) {
            let old = row.old.map_or(String::new(), |n| (n + 1).to_string());
            let new = row.new.map_or(String::new(), |n| (n + 1).to_string());
            let prefix = if row.kind == Kind::Header {
                String::new()
            } else {
                format!("{old:>5} {new:>5} {} ", symbol(row.kind))
            };
            render_row(frame, rect, row, &prefix, view.left, base, theme);
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn render_side(
    frame: &mut Frame<'_>,
    diff: &Diff,
    index: Option<usize>,
    new: bool,
    area: Rect,
    left: usize,
    style: Style,
    theme: &Theme,
) {
    let Some(row) = index.and_then(|i| diff.rows.get(i)) else {
        return;
    };
    let number = if new { row.new } else { row.old }.map_or(String::new(), |n| (n + 1).to_string());
    let prefix = if row.kind == Kind::Header {
        String::new()
    } else {
        format!("{number:>5} {} ", symbol(row.kind))
    };
    render_row(frame, area, row, &prefix, left, style, theme);
}
fn symbol(kind: Kind) -> char {
    match kind {
        Kind::Header => '@',
        Kind::Context => ' ',
        Kind::Add => '+',
        Kind::Delete => '-',
    }
}
fn render_row(
    frame: &mut Frame<'_>,
    area: Rect,
    row: &Row,
    prefix: &str,
    left: usize,
    style: Style,
    theme: &Theme,
) {
    let color = match row.kind {
        Kind::Header => theme.diff_modified,
        Kind::Add => theme.diff_added,
        Kind::Delete => theme.diff_deleted,
        Kind::Context => theme.foreground,
    };
    let prefix = visible_text(prefix, 0, usize::from(area.width));
    let width = usize::from(area.width).saturating_sub(prefix.len());
    let text = format!(
        "{}{}{}",
        row.text.strip_suffix('\r').unwrap_or(&row.text),
        if row.text.ends_with('\r') && matches!(row.kind, Kind::Add | Kind::Delete) {
            "  [CRLF]"
        } else {
            ""
        },
        if row.no_newline {
            "  [no final newline]"
        } else {
            ""
        }
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prefix, style.fg(theme.muted)),
            Span::styled(visible_text(&text, left, width), style.fg(color)),
        ]))
        .style(style),
        area,
    );
}
