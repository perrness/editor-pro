use editor_pro::{
    buffer::Buffer,
    syntax::{Highlight, MAX_SYNTAX_BYTES, Syntax, highlight_at},
};
use std::path::Path;

fn syntax(path: &str, buffer: &mut Buffer) -> Syntax {
    let mut syntax = Syntax::for_path(Some(Path::new(path)));
    let edits = buffer.take_edits();
    syntax.update(buffer.text(), &edits);
    syntax
}

#[test]
fn rust_uses_upstream_queries_and_semantic_captures() {
    let mut buffer = Buffer::new("// comment\r\nfn café() { let value = \"界\"; }\r\n");
    let syntax = syntax("source.rs", &mut buffer);
    assert!(syntax.fallback.is_none(), "{:?}", syntax.fallback);
    assert!(!syntax.tree().unwrap().root_node().has_error());
    let text = buffer.text().to_string();
    let spans = syntax.highlights(buffer.text(), 0..text.len());
    for (needle, kind) in [
        ("//", Highlight::Comment),
        ("fn", Highlight::Keyword),
        ("café", Highlight::Function),
        ("\"界\"", Highlight::String),
    ] {
        assert_eq!(
            highlight_at(&spans, text.find(needle).unwrap()),
            Some(kind),
            "{needle}"
        );
    }
}

#[test]
fn incremental_unicode_crlf_edits_and_history_match_a_fresh_parse() {
    let mut buffer = Buffer::new("fn main() {\r\n    let café = \"界👩‍💻\";\r\n}\r\n");
    let mut parsed = syntax("source.rs", &mut buffer);
    for step in 0..8 {
        match step {
            0 => {
                buffer.insert("/*界\r\n");
                buffer.commit();
            }
            1 => {
                buffer.insert("*/");
                buffer.commit();
            }
            2 => {
                buffer.goto_line(2, true);
                buffer.line_end(true);
                buffer.backspace();
                buffer.commit();
            }
            3 | 4 => {
                buffer.undo();
            }
            5 => {
                buffer.redo();
            }
            6 => {
                buffer.goto_line(0, false);
                buffer.delete(false);
                buffer.commit();
            }
            _ => {
                buffer.insert("\u{301}");
                buffer.commit();
            }
        }
        let edits = buffer.take_edits();
        parsed.update(buffer.text(), &edits);
        let fresh = syntax("source.rs", &mut buffer);
        assert_eq!(
            parsed.tree().unwrap().root_node().to_sexp(),
            fresh.tree().unwrap().root_node().to_sexp()
        );
        let range = 0..buffer.text().len_bytes();
        assert_eq!(
            parsed.highlights(buffer.text(), range.clone()),
            fresh.highlights(buffer.text(), range)
        );
    }
    let root_id = parsed.tree().unwrap().root_node().id();
    buffer.move_right(false);
    let edits = buffer.take_edits();
    parsed.update(buffer.text(), &edits);
    assert_eq!(parsed.tree().unwrap().root_node().id(), root_id);
}

#[test]
fn multiline_capture_is_available_when_only_middle_is_visible() {
    let mut buffer = Buffer::new("/* start\n界 middle\nend */\nfn main() {}\n");
    let parsed = syntax("source.rs", &mut buffer);
    let start = buffer.text().line_to_byte(1);
    let end = buffer.text().line_to_byte(2);
    let spans = parsed.highlights(buffer.text(), start..end);
    assert_eq!(highlight_at(&spans, start), Some(Highlight::Comment));
}

#[test]
fn unknown_and_large_documents_fall_back_without_altering_text() {
    let mut buffer = Buffer::new("plain 👩‍💻\r\n");
    let parsed = syntax("unknown.ext", &mut buffer);
    assert!(parsed.tree().is_none());
    assert!(
        parsed
            .highlights(buffer.text(), 0..buffer.text().len_bytes())
            .is_empty()
    );
    let mut large = Buffer::new(&" ".repeat(MAX_SYNTAX_BYTES + 1));
    let parsed = syntax("large.rs", &mut large);
    assert!(parsed.tree().is_none());
    assert!(parsed.fallback.as_ref().unwrap().contains("2 MiB"));
    assert_eq!(large.text().len_bytes(), MAX_SYNTAX_BYTES + 1);
}
