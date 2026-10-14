//! Grapheme-aware motions and text objects, with no terminal or editing mode.
use crate::buffer::Buffer;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub chars: Range<usize>,
    pub linewise: bool,
}

pub fn lines(buffer: &Buffer, first: usize, last: usize) -> Selection {
    let start = first.min(last).min(buffer.line_count() - 1);
    let end = (first.max(last) + 1).min(buffer.line_count());
    Selection {
        chars: buffer.text().line_to_char(start)..buffer.text().line_to_char(end),
        linewise: true,
    }
}

pub fn visual(buffer: &Buffer, anchor: usize, linewise: bool) -> Selection {
    let cursor = buffer.cursor();
    if linewise {
        lines(
            buffer,
            buffer.text().char_to_line(anchor),
            buffer.position().0,
        )
    } else {
        Selection {
            chars: anchor.min(cursor)..buffer.grapheme_end(anchor.max(cursor)),
            linewise: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Start,
    FirstNonblank,
    End,
    WordForward(bool),
    WordBackward(bool),
    WordEnd(bool),
    FirstLine,
    LastLine,
    MatchingPair,
    Find {
        character: char,
        backward: bool,
        till: bool,
    },
}

#[derive(Clone, Copy)]
pub enum Extent {
    Exclusive,
    Inclusive,
    Lines,
}

pub fn apply(buffer: &mut Buffer, motion: Motion, count: usize, operator: bool) -> Option<Extent> {
    let repetitions = count.max(1);
    let extent = match motion {
        Motion::Left | Motion::Right => {
            for _ in 0..repetitions {
                if matches!(motion, Motion::Left) {
                    buffer.move_left(operator);
                } else {
                    buffer.move_right(operator);
                }
            }
            Extent::Exclusive
        }
        Motion::Up | Motion::Down => {
            let delta = if matches!(motion, Motion::Up) {
                -(repetitions as isize)
            } else {
                repetitions as isize
            };
            buffer.move_vertical(delta, false);
            Extent::Lines
        }
        Motion::Start => {
            buffer.line_start();
            Extent::Exclusive
        }
        Motion::FirstNonblank => {
            buffer.first_nonblank();
            buffer.clamp_cursor(false);
            Extent::Exclusive
        }
        Motion::End => {
            if repetitions > 1 {
                buffer.move_vertical(repetitions as isize - 1, false);
            }
            buffer.line_end(false);
            Extent::Inclusive
        }
        Motion::FirstLine | Motion::LastLine => {
            let line = if count > 0 {
                count - 1
            } else if matches!(motion, Motion::FirstLine) {
                0
            } else {
                buffer.line_count() - 1
            };
            buffer.goto_line(line, false);
            Extent::Lines
        }
        Motion::WordForward(big) | Motion::WordBackward(big) | Motion::WordEnd(big) => {
            let mut target = buffer.cursor();
            for _ in 0..repetitions {
                target = word_target(buffer, target, motion, big);
            }
            buffer.set_cursor(target, operator);
            if matches!(motion, Motion::WordEnd(_)) {
                Extent::Inclusive
            } else {
                Extent::Exclusive
            }
        }
        Motion::MatchingPair => {
            let target = matching_pair(buffer)?;
            buffer.set_cursor(target, false);
            Extent::Inclusive
        }
        Motion::Find {
            character,
            backward,
            till,
        } => {
            let target = find(buffer, character, backward, till, repetitions)?;
            buffer.set_cursor(target, false);
            Extent::Inclusive
        }
    };
    Some(extent)
}

pub fn operated(buffer: &Buffer, origin: usize, extent: Extent) -> Selection {
    let target = buffer.cursor();
    if matches!(extent, Extent::Lines) {
        lines(
            buffer,
            buffer.text().char_to_line(origin),
            buffer.position().0,
        )
    } else {
        let start = origin.min(target);
        let end = origin.max(target);
        Selection {
            chars: start
                ..if matches!(extent, Extent::Inclusive)
                    && !(origin == target && buffer.line(buffer.position().0).is_empty())
                {
                    buffer.grapheme_end(end)
                } else {
                    end
                },
            linewise: false,
        }
    }
}

#[derive(Clone, Copy)]
struct Atom {
    start: usize,
    end: usize,
    class: u8,
}
fn class(grapheme: &str, big: bool) -> u8 {
    if grapheme.chars().all(char::is_whitespace) {
        0
    } else if big
        || grapheme
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        1
    } else {
        2
    }
}

struct Atoms<'a> {
    buffer: &'a Buffer,
    row: usize,
    atoms: Vec<Atom>,
    index: usize,
    big: bool,
}
impl<'a> Atoms<'a> {
    fn new(buffer: &'a Buffer, position: usize, big: bool) -> Self {
        let row = buffer.text().char_to_line(position);
        let mut result = Self {
            buffer,
            row,
            atoms: vec![],
            index: 0,
            big,
        };
        result.load();
        result.index = result.atoms.partition_point(|atom| atom.start < position);
        result
    }
    fn load(&mut self) {
        let mut start = self.buffer.text().line_to_char(self.row);
        self.atoms = self
            .buffer
            .text()
            .line(self.row)
            .to_string()
            .graphemes(true)
            .map(|g| {
                let end = start + g.chars().count();
                let atom = Atom {
                    start,
                    end,
                    class: class(g, self.big),
                };
                start = end;
                atom
            })
            .collect();
    }
    fn current(&self) -> Option<Atom> {
        self.atoms.get(self.index).copied()
    }
    fn next(&mut self) -> Option<Atom> {
        self.index += 1;
        while self.index >= self.atoms.len() {
            if self.row + 1 >= self.buffer.line_count() {
                return None;
            }
            self.row += 1;
            self.load();
            self.index = 0;
        }
        self.current()
    }
    fn previous(&mut self) -> Option<Atom> {
        while self.index == 0 {
            if self.row == 0 {
                return None;
            }
            self.row -= 1;
            self.load();
            self.index = self.atoms.len();
        }
        self.index -= 1;
        self.current()
    }
}

fn word_target(buffer: &Buffer, position: usize, motion: Motion, big: bool) -> usize {
    let mut atoms = Atoms::new(buffer, position, big);
    match motion {
        Motion::WordForward(_) => {
            let Some(first) = atoms.current() else {
                return position;
            };
            let mut current = Some(first);
            while current.is_some_and(|a| a.class == first.class) {
                current = atoms.next();
            }
            while current.is_some_and(|a| a.class == 0) {
                current = atoms.next();
            }
            current.map_or(buffer.text().len_chars(), |a| a.start)
        }
        Motion::WordBackward(_) => {
            let mut current = atoms.previous();
            while current.is_some_and(|a| a.class == 0) {
                current = atoms.previous();
            }
            let Some(first) = current else {
                return 0;
            };
            let mut target = first.start;
            while let Some(previous) = atoms.previous() {
                if previous.class != first.class {
                    break;
                }
                target = previous.start;
            }
            target
        }
        _ => {
            let mut current = atoms.next();
            while current.is_some_and(|a| a.class == 0) {
                current = atoms.next();
            }
            let Some(first) = current else {
                return position;
            };
            let mut target = first.start;
            while let Some(next) = atoms.next() {
                if next.class != first.class {
                    break;
                }
                target = next.start;
            }
            target
        }
    }
}

pub fn text_object(buffer: &Buffer, object: char, around: bool, count: usize) -> Option<Selection> {
    let cursor = buffer.cursor();
    let range = match object {
        'w' | 'W' => {
            let big = object == 'W';
            let mut atoms = Atoms::new(buffer, cursor, big);
            let first = atoms.current()?;
            let mut start = first.start;
            while let Some(previous) = atoms.previous() {
                if previous.class != first.class {
                    break;
                }
                start = previous.start;
            }
            let mut atoms = Atoms::new(buffer, cursor, big);
            let mut end = first.end;
            while let Some(next) = atoms.next() {
                if next.class != first.class {
                    break;
                }
                end = next.end;
            }
            for _ in 1..count.max(1) {
                let target = word_target(
                    buffer,
                    buffer.grapheme_start_before(end),
                    Motion::WordEnd(big),
                    big,
                );
                end = buffer.grapheme_end(target);
            }
            if around {
                let line_end = buffer.text().line_to_char(buffer.text().char_to_line(end))
                    + buffer.line(buffer.text().char_to_line(end)).chars().count();
                let original_end = end;
                while end < line_end && buffer.text().char(end).is_whitespace() {
                    end = buffer.grapheme_end(end);
                }
                if end == original_end {
                    let line_start = buffer
                        .text()
                        .line_to_char(buffer.text().char_to_line(start));
                    while start > line_start {
                        let previous = buffer.grapheme_start_before(start);
                        if !buffer.text().char(previous).is_whitespace() {
                            break;
                        }
                        start = previous;
                    }
                }
            }
            start..end
        }
        '"' | '\'' | '`' => {
            let row = buffer.position().0;
            let mut opening = None;
            let mut escaped = false;
            let mut found = None;
            for (offset, ch) in buffer.line(row).chars().enumerate() {
                let at = buffer.text().line_to_char(row) + offset;
                if ch == object && !escaped {
                    if let Some(start) = opening.take() {
                        if start <= cursor && cursor <= at {
                            found = Some((start, at));
                            break;
                        }
                    } else {
                        opening = Some(at);
                    }
                }
                escaped = ch == '\\' && !escaped;
            }
            let (start, end) = found?;
            if around {
                start..buffer.grapheme_end(end)
            } else {
                buffer.grapheme_end(start)..end
            }
        }
        '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' => {
            let (open, close) = match object {
                '(' | ')' | 'b' => ('(', ')'),
                '[' | ']' => ('[', ']'),
                '{' | '}' | 'B' => ('{', '}'),
                _ => ('<', '>'),
            };
            let mut stack = Vec::new();
            let mut pairs = Vec::new();
            for (at, ch) in buffer.text().chars().enumerate() {
                if ch == open {
                    stack.push(at);
                } else if ch == close
                    && let Some(start) = stack.pop()
                    && start <= cursor
                    && cursor <= at
                {
                    pairs.push((start, at));
                }
            }
            pairs.sort_by_key(|(start, end)| end - start);
            let &(start, end) = pairs.get(count.max(1) - 1)?;
            if around {
                start..buffer.grapheme_end(end)
            } else {
                buffer.grapheme_end(start)..end
            }
        }
        _ => return None,
    };
    Some(Selection {
        chars: range,
        linewise: false,
    })
}

fn matching_pair(buffer: &Buffer) -> Option<usize> {
    let line = buffer.position().0;
    let end = buffer.text().line_to_char(line) + buffer.line(line).chars().count();
    let (start, ch) = (buffer.cursor()..end)
        .map(|i| (i, buffer.text().char(i)))
        .find(|(_, c)| "()[]{}".contains(*c))?;
    let (open, close, backward) = match ch {
        '(' => ('(', ')', false),
        ')' => ('(', ')', true),
        '[' => ('[', ']', false),
        ']' => ('[', ']', true),
        '{' => ('{', '}', false),
        _ => ('{', '}', true),
    };
    let mut depth = 1;
    let positions: Box<dyn Iterator<Item = usize>> = if backward {
        Box::new((0..start).rev())
    } else {
        Box::new(start + 1..buffer.text().len_chars())
    };
    for at in positions {
        let c = buffer.text().char(at);
        if c == if backward { close } else { open } {
            depth += 1;
        }
        if c == if backward { open } else { close } {
            depth -= 1;
        }
        if depth == 0 {
            return Some(at);
        }
    }
    None
}

fn find(
    buffer: &Buffer,
    character: char,
    backward: bool,
    till: bool,
    count: usize,
) -> Option<usize> {
    let row = buffer.position().0;
    let start = buffer.text().line_to_char(row);
    let mut at = start;
    let mut positions: Vec<_> = buffer
        .line(row)
        .graphemes(true)
        .filter_map(|g| {
            let position = at;
            at += g.chars().count();
            (g.starts_with(character)
                && if backward {
                    position < buffer.cursor()
                } else {
                    position > buffer.cursor()
                })
            .then_some(position)
        })
        .collect();
    if backward {
        positions.reverse();
    }
    let target = *positions.get(count - 1)?;
    Some(if !till {
        target
    } else if backward {
        buffer.grapheme_end(target)
    } else {
        buffer.grapheme_start_before(target)
    })
}
