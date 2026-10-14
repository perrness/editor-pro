//! Derived syntax state. Text, cursor coordinates and undo remain in Buffer.
use std::{
    ops::Range,
    path::Path,
    time::{Duration, Instant},
};

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language, ParseOptions, Parser, Point, Query, QueryCursor, QueryCursorOptions,
    StreamingIterator, TextProvider, Tree,
};

use crate::buffer::TextEdit;

pub const MAX_SYNTAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    Comment,
    Keyword,
    String,
    Number,
    Function,
    Type,
    Constant,
    Property,
    Operator,
    Punctuation,
    Heading,
    Emphasis,
}

impl Highlight {
    fn capture(name: &str) -> Option<Self> {
        Some(match name.split('.').next()? {
            "comment" => Self::Comment,
            "keyword" => Self::Keyword,
            "string" | "character" => Self::String,
            "number" | "float" => Self::Number,
            "function" | "constructor" => Self::Function,
            "type" => Self::Type,
            "constant" | "boolean" => Self::Constant,
            "property" | "attribute" | "label" | "tag" => Self::Property,
            "operator" => Self::Operator,
            "punctuation" => Self::Punctuation,
            "markup" | "text" => {
                if name.contains("heading") || name.contains("title") {
                    Self::Heading
                } else if name.contains("raw") || name.contains("literal") {
                    Self::String
                } else {
                    Self::Emphasis
                }
            }
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub bytes: Range<usize>,
    pub kind: Highlight,
    priority: usize,
}

pub fn highlight_at(spans: &[HighlightSpan], byte: usize) -> Option<Highlight> {
    spans
        .iter()
        .filter(|span| span.bytes.contains(&byte))
        .min_by_key(|span| (span.bytes.len(), std::cmp::Reverse(span.priority)))
        .map(|span| span.kind)
}

struct Layer {
    parser: Parser,
    query: Query,
    tree: Option<Tree>,
}

impl Layer {
    fn new(language: Language, query: &str) -> Result<Self, String> {
        let mut parser = Parser::new();
        parser.set_language(&language).map_err(|e| e.to_string())?;
        let query = Query::new(&language, query).map_err(|e| e.to_string())?;
        Ok(Self {
            parser,
            query,
            tree: None,
        })
    }
}

pub struct Syntax {
    pub language: &'static str,
    pub fallback: Option<String>,
    layers: Vec<Layer>,
}

impl Syntax {
    pub fn for_path(path: Option<&Path>) -> Self {
        let extension = path
            .and_then(Path::extension)
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let (language, layers) = match extension {
            "rs" => (
                "Rust",
                vec![Layer::new(
                    tree_sitter_rust::LANGUAGE.into(),
                    tree_sitter_rust::HIGHLIGHTS_QUERY,
                )],
            ),
            _ => ("Text", vec![]),
        };
        match layers.into_iter().collect::<Result<Vec<_>, _>>() {
            Ok(layers) => Self {
                language,
                fallback: None,
                layers,
            },
            Err(error) => Self {
                language,
                fallback: Some(error),
                layers: vec![],
            },
        }
    }

    pub fn update(&mut self, text: &Rope, edits: &[TextEdit]) {
        if self.layers.is_empty() {
            return;
        }
        if text.len_bytes() > MAX_SYNTAX_BYTES {
            for layer in &mut self.layers {
                layer.tree = None;
            }
            self.fallback = Some("syntax limit: 2 MiB".into());
            return;
        }
        // A failed parse is retried on the next edit, not on every cursor move.
        if edits.is_empty() && (self.layers[0].tree.is_some() || self.fallback.is_some()) {
            return;
        }
        self.fallback = None;
        for layer in &mut self.layers {
            if let Some(tree) = &mut layer.tree {
                for edit in edits {
                    tree.edit(&InputEdit {
                        start_byte: edit.start_byte,
                        old_end_byte: edit.old_end_byte,
                        new_end_byte: edit.new_end_byte,
                        start_position: Point::new(edit.start_position.0, edit.start_position.1),
                        old_end_position: Point::new(
                            edit.old_end_position.0,
                            edit.old_end_position.1,
                        ),
                        new_end_position: Point::new(
                            edit.new_end_position.0,
                            edit.new_end_position.1,
                        ),
                    });
                }
            }
            let started = Instant::now();
            let mut cancel =
                |_state: &tree_sitter::ParseState| started.elapsed() > Duration::from_millis(100);
            let tree = layer.parser.parse_with_options(
                &mut |byte, _| {
                    if byte >= text.len_bytes() {
                        return &[][..];
                    }
                    let (chunk, start, _, _) = text.chunk_at_byte(byte);
                    &chunk.as_bytes()[byte - start..]
                },
                layer.tree.as_ref(),
                Some(ParseOptions::new().progress_callback(&mut cancel)),
            );
            if tree.is_none() {
                layer.parser.reset();
                self.fallback = Some("syntax parse budget exceeded".into());
            }
            layer.tree = tree;
        }
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.layers.first()?.tree.as_ref()
    }

    /// Query only the viewport. The Rope provider avoids copying the document.
    pub fn highlights(&self, text: &Rope, bytes: Range<usize>) -> Vec<HighlightSpan> {
        let mut spans = Vec::new();
        let started = Instant::now();
        for layer in &self.layers {
            let Some(tree) = &layer.tree else {
                continue;
            };
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(bytes.clone());
            let mut cancel = |_state: &tree_sitter::QueryCursorState| {
                started.elapsed() > Duration::from_millis(20)
            };
            let mut captures = cursor.captures_with_options(
                &layer.query,
                tree.root_node(),
                RopeProvider(text),
                QueryCursorOptions::new().progress_callback(&mut cancel),
            );
            while let Some((matched, index)) = captures.next() {
                let capture = matched.captures[*index];
                if let Some(kind) =
                    Highlight::capture(layer.query.capture_names()[capture.index as usize])
                {
                    spans.push(HighlightSpan {
                        bytes: capture.node.byte_range(),
                        kind,
                        priority: matched.pattern_index,
                    });
                }
                if spans.len() >= 20_000 {
                    return spans;
                }
            }
        }
        spans
    }
}

struct RopeProvider<'a>(&'a Rope);
impl<'a> TextProvider<&'a str> for RopeProvider<'a> {
    type I = ropey::iter::Chunks<'a>;
    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        self.0.byte_slice(node.byte_range()).chunks()
    }
}
