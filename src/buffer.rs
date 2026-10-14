use std::collections::VecDeque;
use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use crate::text::{char_at_display_column, display_column};

const HISTORY_LIMIT: usize = 1_000;

/// Byte/point coordinates at a text mutation boundary. Points count LF rows
/// and UTF-8 bytes, never terminal cells or Unicode scalar columns.
#[derive(Clone, Debug)]
pub struct TextEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_position: (usize, usize),
    pub old_end_position: (usize, usize),
    pub new_end_position: (usize, usize),
}

#[derive(Clone)]
struct Snapshot {
    text: Rope,
    cursor: usize,
    revision: u64,
}

/// Text and editing history, independent of terminal input, rendering, and disk I/O.
pub struct Buffer {
    text: Rope,
    cursor: usize,
    desired_column: Option<usize>,
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    transaction: Option<Snapshot>,
    revision: u64,
    saved_revision: u64,
    next_revision: u64,
    newline: &'static str,
    edits: Vec<TextEdit>,
    generation: u64,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        // Preserve all existing bytes; use the first line ending for new lines.
        let newline = text.find('\n').map_or("\n", |index| {
            if index > 0 && text.as_bytes()[index - 1] == b'\r' {
                "\r\n"
            } else {
                "\n"
            }
        });
        Self {
            text: Rope::from_str(text),
            cursor: 0,
            desired_column: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            transaction: None,
            revision: 0,
            saved_revision: 0,
            next_revision: 1,
            newline,
            edits: Vec::new(),
            generation: 0,
        }
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn take_edits(&mut self) -> Vec<TextEdit> {
        std::mem::take(&mut self.edits)
    }

    fn splice(&mut self, range: Range<usize>, text: &str) {
        let start_byte = self.text.char_to_byte(range.start);
        let old_end_byte = self.text.char_to_byte(range.end);
        let start_position = point(&self.text, start_byte);
        let old_end_position = point(&self.text, old_end_byte);
        self.text.remove(range.clone());
        self.text.insert(range.start, text);
        let new_end_byte = start_byte + text.len();
        self.edits.push(TextEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position: point(&self.text, new_end_byte),
        });
        self.generation += 1;
    }

    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    pub fn position(&self) -> (usize, usize) {
        let line = self.text.char_to_line(self.cursor);
        (line, self.cursor - self.text.line_to_char(line))
    }

    pub fn line(&self, index: usize) -> String {
        let mut line = self.text.line(index).to_string();
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        line
    }

    pub fn newline_name(&self) -> &'static str {
        if self.newline == "\r\n" { "CRLF" } else { "LF" }
    }

    pub fn is_modified(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.commit();
        self.saved_revision = self.revision;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            revision: self.revision,
        }
    }

    pub fn begin(&mut self) {
        if self.transaction.is_none() {
            self.transaction = Some(self.snapshot());
        }
    }

    pub fn commit(&mut self) {
        if let Some(before) = self.transaction.take() {
            if before.text == self.text {
                self.revision = before.revision;
            } else {
                if self.undo.len() == HISTORY_LIMIT {
                    self.undo.pop_front();
                }
                self.undo.push_back(before);
                self.redo.clear();
            }
        }
    }

    fn changed(&mut self) {
        self.revision = self.next_revision;
        self.next_revision += 1;
        self.desired_column = None;
    }

    /// Insert one typed character or one bracketed paste. The caller groups undo.
    pub fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.begin();
        self.splice(self.cursor..self.cursor, text);
        self.cursor += text.chars().count();
        self.changed();
        // Inserting a ZWJ or combining mark can join text on either side.
        self.snap_cursor_forward();
    }

    pub fn insert_newline(&mut self) {
        self.insert(self.newline);
    }

    pub fn backspace(&mut self) {
        let (line_index, column) = self.position();
        if self.cursor == 0 {
            return;
        }
        let start = if column == 0 {
            self.text.line_to_char(line_index - 1) + self.line(line_index - 1).chars().count()
        } else {
            self.text.line_to_char(line_index) + previous_boundary(&self.line(line_index), column)
        };
        self.begin();
        self.splice(start..self.cursor, "");
        self.cursor = start;
        self.changed();
        self.clamp_cursor(true);
    }

    /// Delete at the cursor; Insert Delete may join lines, Normal x never does.
    pub fn delete(&mut self, join_lines: bool) {
        let (line_index, column) = self.position();
        let line = self.line(line_index);
        let length = line.chars().count();
        let end = if column < length {
            self.text.line_to_char(line_index) + next_boundary(&line, column)
        } else if join_lines && line_index + 1 < self.line_count() {
            self.text.line_to_char(line_index + 1)
        } else {
            return;
        };
        self.begin();
        self.splice(self.cursor..end, "");
        self.changed();
        self.clamp_cursor(join_lines);
    }

    pub fn move_left(&mut self, insert: bool) {
        let (line_index, column) = self.position();
        self.cursor =
            self.text.line_to_char(line_index) + previous_boundary(&self.line(line_index), column);
        self.desired_column = None;
        self.clamp_cursor(insert);
    }

    pub fn move_right(&mut self, insert: bool) {
        let (line_index, column) = self.position();
        self.cursor =
            self.text.line_to_char(line_index) + next_boundary(&self.line(line_index), column);
        self.desired_column = None;
        self.clamp_cursor(insert);
    }

    pub fn move_vertical(&mut self, delta: isize, insert: bool) {
        let (line, column) = self.position();
        let goal = self
            .desired_column
            .unwrap_or_else(|| display_column(&self.line(line), column));
        let target = line.saturating_add_signed(delta).min(self.line_count() - 1);
        self.cursor =
            self.text.line_to_char(target) + char_at_display_column(&self.line(target), goal);
        self.clamp_cursor(insert);
        self.desired_column = Some(goal);
    }

    pub fn line_start(&mut self) {
        let (line, _) = self.position();
        self.cursor = self.text.line_to_char(line);
        self.desired_column = None;
    }

    pub fn first_nonblank(&mut self) {
        let (line, _) = self.position();
        let indent = self
            .line(line)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();
        self.cursor = self.text.line_to_char(line) + indent;
        self.desired_column = None;
        self.clamp_cursor(true);
    }

    pub fn line_end(&mut self, insert: bool) {
        let (line, _) = self.position();
        self.cursor = self.text.line_to_char(line) + self.line(line).chars().count();
        self.desired_column = None;
        self.clamp_cursor(insert);
    }

    pub fn goto_line(&mut self, line: usize, insert: bool) {
        self.cursor = self.text.line_to_char(line.min(self.line_count() - 1));
        self.desired_column = None;
        self.clamp_cursor(insert);
    }

    /// Open a line above/below in the same undo transaction as subsequent typing.
    pub fn open_line(&mut self, above: bool) {
        self.begin();
        if above {
            self.line_start();
            let start = self.cursor;
            self.insert_newline();
            self.cursor = start;
        } else {
            self.line_end(true);
            self.insert_newline();
        }
    }

    pub fn clamp_cursor(&mut self, insert: bool) {
        self.cursor = self.cursor.min(self.text.len_chars());
        let (line_index, column) = self.position();
        let line = self.line(line_index);
        let length = line.chars().count();
        let limit = if insert || length == 0 {
            length
        } else {
            previous_boundary(&line, length)
        };
        let mut boundary = 0;
        for grapheme in line.graphemes(true) {
            let next = boundary + grapheme.chars().count();
            if next > column.min(limit) {
                break;
            }
            boundary = next;
        }
        self.cursor = self.text.line_to_char(line_index) + boundary;
    }

    fn snap_cursor_forward(&mut self) {
        let (line_index, column) = self.position();
        let line = self.line(line_index);
        let mut boundary = 0;
        for grapheme in line.graphemes(true) {
            if boundary >= column {
                break;
            }
            boundary += grapheme.chars().count();
        }
        self.cursor = self.text.line_to_char(line_index) + boundary;
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.edits.push(TextEdit {
            start_byte: 0,
            old_end_byte: self.text.len_bytes(),
            new_end_byte: snapshot.text.len_bytes(),
            start_position: (0, 0),
            old_end_position: point(&self.text, self.text.len_bytes()),
            new_end_position: point(&snapshot.text, snapshot.text.len_bytes()),
        });
        self.generation += 1;
        self.text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.revision = snapshot.revision;
        self.desired_column = None;
        self.clamp_cursor(false);
    }

    pub fn undo(&mut self) -> bool {
        self.commit();
        if let Some(before) = self.undo.pop_back() {
            self.redo.push(self.snapshot());
            self.restore(before);
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        self.commit();
        if let Some(after) = self.redo.pop() {
            self.undo.push_back(self.snapshot());
            self.restore(after);
            true
        } else {
            false
        }
    }
}

fn point(text: &Rope, byte: usize) -> (usize, usize) {
    let row = text.byte_to_line(byte);
    (row, byte - text.line_to_byte(row))
}

fn previous_boundary(line: &str, column: usize) -> usize {
    let mut boundary = 0;
    for grapheme in line.graphemes(true) {
        let next = boundary + grapheme.chars().count();
        if next >= column {
            break;
        }
        boundary = next;
    }
    boundary
}

fn next_boundary(line: &str, column: usize) -> usize {
    let mut boundary = 0;
    for grapheme in line.graphemes(true) {
        boundary += grapheme.chars().count();
        if boundary > column {
            break;
        }
    }
    boundary
}
