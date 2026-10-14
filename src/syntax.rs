//! Derived syntax state. Text, cursor coordinates and undo remain in Buffer.
use std::{
    ops::{ControlFlow, Range},
    path::Path,
    time::{Duration, Instant},
};

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language, ParseOptions, Parser, Point, Query, QueryCursor, QueryCursorOptions,
    StreamingIterator, TextProvider, Tree,
};

use tree_sitter_md::{MarkdownParseOptions, MarkdownParser, MarkdownTree};

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

enum ParserState {
    Single(Parser, Option<Tree>),
    Markdown(MarkdownParser, Option<MarkdownTree>),
}

pub struct Syntax {
    pub language: &'static str,
    pub fallback: Option<String>,
    parser: Option<ParserState>,
    queries: Vec<Query>,
}

fn query(language: Language, source: &str) -> Result<Query, String> {
    let query = Query::new(&language, source).map_err(|error| error.to_string())?;
    for pattern in 0..query.pattern_count() {
        if !query.general_predicates(pattern).is_empty() {
            return Err("unsupported highlight predicate".into());
        }
    }
    Ok(query)
}

impl Syntax {
    pub fn for_path(path: Option<&Path>) -> Self {
        let extension = path
            .and_then(Path::extension)
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let (language, layers): (_, Vec<(Language, &str)>) = match extension {
            "rs" => (
                "Rust",
                vec![(
                    tree_sitter_rust::LANGUAGE.into(),
                    tree_sitter_rust::HIGHLIGHTS_QUERY,
                )],
            ),
            "java" => (
                "Java",
                vec![(
                    tree_sitter_java::LANGUAGE.into(),
                    tree_sitter_java::HIGHLIGHTS_QUERY,
                )],
            ),
            "kt" | "kts" => (
                "Kotlin",
                vec![(
                    tree_sitter_kotlin::LANGUAGE.into(),
                    tree_sitter_kotlin::HIGHLIGHTS_QUERY,
                )],
            ),
            "cs" => (
                "C#",
                vec![(
                    tree_sitter_c_sharp::LANGUAGE.into(),
                    tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
                )],
            ),
            "json" => (
                "JSON",
                vec![(
                    tree_sitter_json::LANGUAGE.into(),
                    tree_sitter_json::HIGHLIGHTS_QUERY,
                )],
            ),
            "yaml" | "yml" => (
                "YAML",
                vec![(
                    tree_sitter_yaml::LANGUAGE.into(),
                    tree_sitter_yaml::HIGHLIGHTS_QUERY,
                )],
            ),
            "toml" => (
                "TOML",
                vec![(
                    tree_sitter_toml_ng::LANGUAGE.into(),
                    tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                )],
            ),
            "md" | "markdown" => (
                "Markdown",
                vec![
                    (
                        tree_sitter_md::LANGUAGE.into(),
                        tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                    ),
                    (
                        tree_sitter_md::INLINE_LANGUAGE.into(),
                        tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                    ),
                ],
            ),
            _ => ("Text", vec![]),
        };
        let build = || -> Result<_, String> {
            let parser = if language == "Markdown" {
                Some(ParserState::Markdown(MarkdownParser::default(), None))
            } else if let Some((grammar, _)) = layers.first() {
                let mut parser = Parser::new();
                parser
                    .set_language(grammar)
                    .map_err(|error| error.to_string())?;
                Some(ParserState::Single(parser, None))
            } else {
                None
            };
            let queries = layers
                .into_iter()
                .map(|(grammar, source)| query(grammar, source))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((parser, queries))
        };
        match build() {
            Ok((parser, queries)) => Self {
                language,
                fallback: None,
                parser,
                queries,
            },
            Err(error) => Self {
                language,
                fallback: Some(error),
                parser: None,
                queries: vec![],
            },
        }
    }

    pub fn update(&mut self, text: &Rope, edits: &[TextEdit]) {
        // A failed parse is retried on the next edit, not on every cursor move.
        if edits.is_empty() && (self.tree().is_some() || self.fallback.is_some()) {
            return;
        }
        let Some(parser) = &mut self.parser else {
            return;
        };
        if text.len_bytes() > MAX_SYNTAX_BYTES {
            match parser {
                ParserState::Single(_, tree) => *tree = None,
                ParserState::Markdown(_, tree) => *tree = None,
            }
            self.fallback = Some("syntax limit: 2 MiB".into());
            return;
        }
        let started = Instant::now();
        let mut cancel = |_state: &tree_sitter::ParseState| budget(started, 100);
        let mut read = |byte, _| {
            if byte >= text.len_bytes() {
                return &[][..];
            }
            let (chunk, start, _, _) = text.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - start..]
        };
        let input_edits = edits.iter().map(|edit| InputEdit {
            start_byte: edit.start_byte,
            old_end_byte: edit.old_end_byte,
            new_end_byte: edit.new_end_byte,
            start_position: Point::new(edit.start_position.0, edit.start_position.1),
            old_end_position: Point::new(edit.old_end_position.0, edit.old_end_position.1),
            new_end_position: Point::new(edit.new_end_position.0, edit.new_end_position.1),
        });
        let parsed = match parser {
            ParserState::Single(parser, tree) => {
                if let Some(tree) = tree {
                    for edit in input_edits {
                        tree.edit(&edit);
                    }
                }
                *tree = parser.parse_with_options(
                    &mut read,
                    tree.as_ref(),
                    Some(ParseOptions::new().progress_callback(&mut cancel)),
                );
                if tree.is_none() {
                    parser.reset();
                }
                tree.is_some()
            }
            ParserState::Markdown(parser, tree) => {
                if let Some(tree) = tree {
                    for edit in input_edits {
                        tree.edit(&edit);
                    }
                }
                // The upstream parser keeps a separate incremental tree for each
                // inline block, including table cells and block continuations.
                let mut cancel_inline = |_state: &tree_sitter::ParseState| budget(started, 100);
                *tree = parser.parse_with_options(
                    &mut read,
                    tree.as_ref(),
                    MarkdownParseOptions::new(
                        Some(ParseOptions::new().progress_callback(&mut cancel)),
                        Some(ParseOptions::new().progress_callback(&mut cancel_inline)),
                    ),
                );
                if tree.is_none() {
                    *parser = MarkdownParser::default();
                }
                tree.is_some()
            }
        };
        self.fallback = (!parsed).then(|| "syntax parse budget exceeded".into());
    }

    pub fn tree(&self) -> Option<&Tree> {
        match self.parser.as_ref()? {
            ParserState::Single(_, tree) => tree.as_ref(),
            ParserState::Markdown(_, tree) => tree.as_ref().map(MarkdownTree::block_tree),
        }
    }

    /// Query only the viewport. The Rope provider avoids copying the document.
    pub fn highlights(&self, text: &Rope, bytes: Range<usize>) -> Vec<HighlightSpan> {
        let mut spans = Vec::new();
        let started = Instant::now();
        let mut collect = |query: &Query, tree: &Tree| {
            let range = tree.root_node().byte_range();
            if range.end <= bytes.start
                || range.start >= bytes.end
                || spans.len() >= 20_000
                || started.elapsed() > Duration::from_millis(20)
            {
                return;
            }
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(bytes.clone());
            let mut cancel = |_state: &tree_sitter::QueryCursorState| budget(started, 20);
            let mut captures = cursor.captures_with_options(
                query,
                tree.root_node(),
                RopeProvider(text),
                QueryCursorOptions::new().progress_callback(&mut cancel),
            );
            while let Some((matched, index)) = captures.next() {
                let capture = matched.captures[*index];
                if let Some(kind) =
                    Highlight::capture(query.capture_names()[capture.index as usize])
                {
                    spans.push(HighlightSpan {
                        bytes: capture.node.byte_range(),
                        kind,
                        priority: matched.pattern_index,
                    });
                }
                if spans.len() >= 20_000 {
                    break;
                }
            }
        };
        if let Some(tree) = self.tree() {
            collect(&self.queries[0], tree);
        }
        if let Some(ParserState::Markdown(_, Some(tree))) = &self.parser {
            for inline in tree.inline_trees() {
                collect(&self.queries[1], inline);
            }
        }
        spans
    }
}

fn budget(started: Instant, millis: u64) -> ControlFlow<()> {
    if started.elapsed() > Duration::from_millis(millis) {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

struct RopeProvider<'a>(&'a Rope);
impl<'a> TextProvider<&'a str> for RopeProvider<'a> {
    type I = ropey::iter::Chunks<'a>;
    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        self.0.byte_slice(node.byte_range()).chunks()
    }
}
