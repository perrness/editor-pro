//! Literal, case-sensitive, single-line search. Matches cover whole graphemes.
use crate::buffer::Buffer;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

const MATCH_LIMIT: usize = 100_000;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub chars: Range<usize>,
    pub bytes: Range<usize>,
}
#[derive(Default)]
pub struct Search {
    pub query: String,
    pub backward: bool,
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
    pub visible: bool,
    generation: Option<u64>,
}
impl Search {
    pub fn set(&mut self, query: &str, backward: bool) {
        self.query = query.into();
        self.backward = backward;
        self.visible = true;
        self.invalidate();
    }
    pub fn invalidate(&mut self) {
        self.generation = None;
        self.matches.clear();
    }
    pub fn refresh(&mut self, buffer: &Buffer) {
        if self.generation == Some(buffer.generation()) {
            return;
        }
        self.generation = Some(buffer.generation());
        self.matches.clear();
        self.truncated = false;
        if self.query.is_empty() || self.query.contains(['\n', '\r']) {
            return;
        }
        let mut char_start = 0;
        let mut byte_start = 0;
        for line in buffer.text().lines() {
            let source = line.to_string();
            let mut matches = source.match_indices(&self.query).peekable();
            if matches.peek().is_some() {
                let mut chars = 0;
                let mut boundaries: Vec<_> = source
                    .grapheme_indices(true)
                    .map(|(byte, g)| {
                        let entry = (byte, chars);
                        chars += g.chars().count();
                        entry
                    })
                    .collect();
                boundaries.push((source.len(), chars));
                for (start, found) in matches {
                    let (Ok(a), Ok(b)) = (
                        boundaries.binary_search_by_key(&start, |b| b.0),
                        boundaries.binary_search_by_key(&(start + found.len()), |b| b.0),
                    ) else {
                        continue;
                    };
                    if self.matches.len() == MATCH_LIMIT {
                        self.truncated = true;
                        return;
                    }
                    self.matches.push(SearchMatch {
                        chars: char_start + boundaries[a].1..char_start + boundaries[b].1,
                        bytes: byte_start + start..byte_start + start + found.len(),
                    });
                }
            }
            char_start += line.len_chars();
            byte_start += line.len_bytes();
        }
    }
    /// Return the result index and whether reaching it crossed a file boundary.
    pub fn navigate(
        &mut self,
        buffer: &mut Buffer,
        reverse: bool,
        count: usize,
    ) -> Option<(usize, bool)> {
        self.refresh(buffer);
        self.visible = true;
        let length = self.matches.len();
        if length == 0 {
            return None;
        }
        let steps = count.max(1) - 1;
        let (index, wrapped) = if self.backward != reverse {
            let split = self
                .matches
                .partition_point(|m| m.chars.start < buffer.cursor());
            let first = if split == 0 { length - 1 } else { split - 1 };
            (
                (first + length - steps % length) % length,
                split == 0 || steps > first,
            )
        } else {
            let split = self
                .matches
                .partition_point(|m| m.chars.start <= buffer.cursor());
            ((split + steps) % length, split + steps >= length)
        };
        buffer.set_cursor(self.matches[index].chars.start, false);
        Some((index, wrapped))
    }
}
