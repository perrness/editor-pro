//! Newline planning over current text and derived syntax; no terminal or I/O work.
use std::ops::Range;

use crate::{buffer::Buffer, syntax::Syntax};

#[derive(Clone, Copy)]
pub(crate) enum NewlineKind {
    Split,
    Above,
    Below,
}

pub(crate) struct NewlineEdit {
    pub range: Range<usize>,
    pub text: String,
    pub cursor: usize,
}

/// Syntax must be synchronized before planning. Unknown/disabled syntax still
/// inherits whitespace, but never guesses whether punctuation is code.
pub(crate) fn newline(buffer: &Buffer, syntax: &Syntax, kind: NewlineKind) -> NewlineEdit {
    let (row, column) = buffer.position();
    let line = buffer.line(row);
    let start = buffer.text().line_to_char(row);
    let start_byte = buffer.text().line_to_byte(row);
    let split = line
        .char_indices()
        .nth(column)
        .map_or(line.len(), |(b, _)| b);
    let prefix = if matches!(kind, NewlineKind::Split) {
        &line[..split]
    } else {
        &line
    };
    let base = leading_whitespace(prefix);
    let unit = indent_unit(buffer, row, syntax, base);
    let opener = last_opener(syntax, start_byte, prefix);
    let mut indent = base.to_string();
    let eol = buffer.newline();

    match kind {
        NewlineKind::Above => {
            let content = &line[base.len()..];
            if closing_token(syntax, start_byte + base.len(), content).is_some() {
                indent.push_str(&unit);
            }
            NewlineEdit {
                range: start..start,
                cursor: start + indent.len(),
                text: format!("{indent}{eol}"),
            }
        }
        NewlineKind::Below => {
            if opener.is_some() {
                indent.push_str(&unit);
            }
            let end = start + line.chars().count();
            NewlineEdit {
                range: end..end,
                cursor: end + eol.len() + indent.len(),
                text: format!("{eol}{indent}"),
            }
        }
        NewlineKind::Split => {
            let suffix = &line[split..];
            // Inside leading whitespace, retain the untouched suffix so splitting
            // indentation cannot duplicate or lose its remaining spaces/tabs.
            let skipped = if prefix == base {
                ""
            } else {
                leading_whitespace(suffix)
            };
            let content = &suffix[skipped.len()..];
            let close_byte = start_byte + split + skipped.len();
            let closer = closing_token(syntax, close_byte, content);
            if opener.is_some() {
                indent.push_str(&unit);
            } else if let Some(closer) = closer
                && let Some(aligned) = closing_indent(buffer, syntax, close_byte, closer)
            {
                indent = aligned;
            }
            let mut text = format!("{eol}{indent}");
            let cursor = buffer.cursor() + text.chars().count();
            if opener.zip(closer).is_some_and(|(open, close)| {
                matches!((open, close), ('{', '}') | ('[', ']') | ('(', ')'))
            }) {
                text.push_str(eol);
                text.push_str(base);
            }
            NewlineEdit {
                range: buffer.cursor()..buffer.cursor() + skipped.len(),
                text,
                cursor,
            }
        }
    }
}

fn leading_whitespace(text: &str) -> &str {
    &text[..text.len() - text.trim_start_matches([' ', '\t']).len()]
}

/// Sample at most 200 nearby lines and 256 indentation bytes per line. Existing
/// tab indentation wins locally; otherwise vote on small changes in space depth.
fn indent_unit(buffer: &Buffer, row: usize, syntax: &Syntax, base: &str) -> String {
    if base.contains('\t') {
        return "\t".into();
    }
    let mut tabs = 0;
    let mut spaces = 0;
    let mut widths = [0_usize; 9];
    let mut previous = 0_usize;
    let start = row.saturating_sub(100);
    for index in start..(start + 200).min(buffer.line_count()) {
        let mut depth = 0_usize;
        let mut has_tab = false;
        for ch in buffer.text().line(index).chars().take(257) {
            match ch {
                ' ' => depth += 1,
                '\t' => {
                    depth += 1;
                    has_tab = true;
                }
                '\r' | '\n' => break,
                _ => {
                    let byte = buffer.text().line_to_byte(index) + depth;
                    if comment_start(syntax, byte..byte + ch.len_utf8()).is_some() {
                        break;
                    }
                    if has_tab {
                        tabs += 1;
                    } else {
                        spaces += usize::from(depth > 0);
                        let difference = depth.abs_diff(previous);
                        if (1..widths.len()).contains(&difference) {
                            widths[difference] += 1;
                        }
                        previous = depth;
                    }
                    break;
                }
            }
        }
    }
    if base.is_empty() && tabs > spaces {
        return "\t".into();
    }
    let default = if matches!(syntax.language, "JSON" | "YAML" | "TOML") {
        2
    } else {
        4
    };
    let width = (1..widths.len())
        .filter(|&width| widths[width] > 0)
        .max_by_key(|&width| (widths[width], std::cmp::Reverse(width)))
        .unwrap_or(default);
    " ".repeat(width)
}

fn token(syntax: &Syntax, byte: usize, ch: char) -> bool {
    if !matches!(
        syntax.language,
        "Rust" | "Java" | "Kotlin" | "C#" | "JSON" | "YAML" | "TOML"
    ) {
        return false;
    }
    syntax
        .tree()
        .and_then(|tree| {
            tree.root_node()
                .descendant_for_byte_range(byte, byte + ch.len_utf8())
        })
        .is_some_and(|node| node.start_byte() == byte && node.kind().as_bytes() == [ch as u8])
}

fn last_opener(syntax: &Syntax, start: usize, mut prefix: &str) -> Option<char> {
    loop {
        prefix = prefix.trim_end_matches([' ', '\t']);
        let (offset, ch) = prefix.char_indices().next_back()?;
        // Comments after an opener do not hide it. Strings and character literals
        // are not delimiter tokens, including multiline strings and raw strings.
        if let Some(byte) = comment_start(syntax, start + offset..start + prefix.len()) {
            prefix = &prefix[..byte.saturating_sub(start)];
            continue;
        }
        return ((matches!(ch, '{' | '[' | '(') || (ch == ':' && syntax.language == "YAML"))
            && token(syntax, start + offset, ch))
        .then_some(ch);
    }
}

fn comment_start(syntax: &Syntax, bytes: Range<usize>) -> Option<usize> {
    let mut node = syntax
        .tree()?
        .root_node()
        .descendant_for_byte_range(bytes.start, bytes.end)?;
    loop {
        if node.kind().contains("comment") {
            return Some(node.start_byte());
        }
        node = node.parent()?;
    }
}

fn closing_token(syntax: &Syntax, byte: usize, text: &str) -> Option<char> {
    let ch = text.chars().next()?;
    (matches!(ch, '}' | ']' | ')') && token(syntax, byte, ch)).then_some(ch)
}

fn closing_indent(buffer: &Buffer, syntax: &Syntax, byte: usize, closer: char) -> Option<String> {
    let open = match closer {
        '}' => "{",
        ']' => "[",
        ')' => "(",
        _ => return None,
    };
    let node = syntax
        .tree()?
        .root_node()
        .descendant_for_byte_range(byte, byte + 1)?;
    let parent = node.parent()?;
    let mut cursor = parent.walk();
    let opener = parent
        .children(&mut cursor)
        .find(|child| child.kind() == open && child.start_byte() < byte)?;
    Some(
        leading_whitespace(&buffer.line(buffer.text().byte_to_line(opener.start_byte())))
            .to_string(),
    )
}
