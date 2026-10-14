//! Shared grapheme and terminal-cell rules. Never use a byte offset as a cursor.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const TAB_WIDTH: usize = 4;

pub fn cell_width(grapheme: &str, column: usize) -> usize {
    if grapheme == "\t" {
        TAB_WIDTH - column % TAB_WIDTH
    } else if grapheme.chars().any(char::is_control) {
        1
    } else {
        UnicodeWidthStr::width(grapheme).max(1)
    }
}

/// Render control characters visibly, never as terminal escape sequences.
pub fn display_grapheme(grapheme: &str, column: usize) -> String {
    if grapheme == "\t" {
        " ".repeat(cell_width(grapheme, column))
    } else if grapheme.chars().any(char::is_control) {
        "�".into()
    } else if UnicodeWidthStr::width(grapheme) == 0 {
        format!("◌{grapheme}")
    } else {
        grapheme.into()
    }
}

pub fn display_column(line: &str, char_column: usize) -> usize {
    let mut chars = 0;
    let mut cells = 0;
    for grapheme in line.graphemes(true) {
        if chars >= char_column {
            break;
        }
        chars += grapheme.chars().count();
        cells += cell_width(grapheme, cells);
    }
    cells
}

pub fn char_at_display_column(line: &str, goal: usize) -> usize {
    let mut chars = 0;
    let mut cells = 0;
    for grapheme in line.graphemes(true) {
        let next = cells + cell_width(grapheme, cells);
        if next > goal {
            break;
        }
        cells = next;
        chars += grapheme.chars().count();
    }
    chars
}

/// Horizontal clipping keeps partial wide graphemes blank, preserving cell alignment.
pub fn visible_text(line: &str, left: usize, width: usize) -> String {
    let mut output = String::new();
    let mut column = 0;
    let right = left.saturating_add(width);
    for grapheme in line.graphemes(true) {
        if column >= right {
            break;
        }
        let end = column + cell_width(grapheme, column);
        if end > left {
            if column >= left && end <= right {
                output.push_str(&display_grapheme(grapheme, column));
            } else {
                output.push_str(&" ".repeat(end.min(right) - column.max(left)));
            }
        }
        column = end;
    }
    output
}

pub fn pop_grapheme(text: &mut String) {
    if let Some((index, _)) = text.grapheme_indices(true).next_back() {
        text.truncate(index);
    }
}
