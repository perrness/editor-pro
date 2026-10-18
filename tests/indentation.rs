use std::path::Path;

use editor_pro::{
    buffer::Buffer,
    editor::{Editor, Mode},
    file::Document,
    input::Key,
    syntax::Syntax,
};

fn editor(path: &str, text: &str) -> Editor {
    let mut document = Document::default();
    document.buffer = Buffer::new(text);
    document.syntax = Syntax::for_path(Some(Path::new(path)));
    Editor::new(document)
}

fn keys(editor: &mut Editor, text: &str) {
    for ch in text.chars() {
        editor.handle_key(Key::Char(ch));
    }
}

fn content(editor: &Editor) -> String {
    editor.document.buffer.text().to_string()
}

#[test]
fn enter_indents_incomplete_blocks_using_language_defaults() {
    for (path, source, indent) in [
        ("Main.java", "class Main {", "    "),
        ("main.rs", "fn main() {", "    "),
        ("Main.kt", "fun main() {", "    "),
        ("build.gradle.kts", "plugins {", "    "),
        ("Main.cs", "class Main {", "    "),
        ("data.json", "{", "  "),
        ("config.yaml", "parent:", "  "),
        ("config.yml", "parent:", "  "),
        ("config.toml", "values = [", "  "),
    ] {
        let mut e = editor(path, source);
        keys(&mut e, "A");
        e.handle_key(Key::Enter);
        assert_eq!(content(&e), format!("{source}\n{indent}"), "{path}");
        assert_eq!(e.document.buffer.position(), (1, indent.len()), "{path}");
    }
}

#[test]
fn enter_between_pairs_moves_closer_and_keeps_cursor_inside() {
    for (path, source, marker, expected, column) in [
        (
            "Main.java",
            "class Main {}",
            "{",
            "class Main {\n    \n}",
            4,
        ),
        ("main.rs", "fn café() {}", "{", "fn café() {\n    \n}", 4),
        (
            "Main.kt",
            "fun main() { call() }",
            "call(",
            "fun main() { call(\n    \n) }",
            4,
        ),
        (
            "data.json",
            "{\"values\": []}",
            "[",
            "{\"values\": [\n  \n]}",
            2,
        ),
        ("config.toml", "values = []", "[", "values = [\n  \n]", 2),
    ] {
        let mut e = editor(path, source);
        keys(&mut e, "i");
        let byte = source.find(marker).unwrap() + marker.len();
        e.document
            .buffer
            .set_cursor(source[..byte].chars().count(), true);
        e.handle_key(Key::Enter);
        assert_eq!(content(&e), expected, "{path}");
        assert_eq!(e.document.buffer.position(), (1, column));
        keys(&mut e, "x");
        assert_eq!(
            e.document.buffer.line(1),
            format!("{}x", " ".repeat(column))
        );
    }
}

#[test]
fn nested_java_pair_preserves_crlf_unicode_and_one_undo_group() {
    let source = "class Café {\r\n    void 界() {}\r\n}\r\n";
    let expected = "class Café {\r\n    void 界() {\r\n        e\u{301}👩‍💻\r\n    }\r\n}\r\n";
    let mut e = editor("Main.java", source);
    keys(&mut e, "i");
    // Insert directly between the method's braces, using scalar coordinates.
    let byte = source.find("{}").unwrap() + 1;
    e.document
        .buffer
        .set_cursor(source[..byte].chars().count(), true);
    e.handle_key(Key::Enter);
    assert_eq!(e.document.buffer.position(), (2, 8));
    keys(&mut e, "e\u{301}👩‍💻");
    e.handle_key(Key::Escape);
    assert_eq!(content(&e), expected);
    keys(&mut e, "u");
    assert_eq!(content(&e), source);
    assert!(!e.document.buffer.is_modified());
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(content(&e), expected);
    e.document.sync_syntax();
    let mut fresh = Syntax::for_path(Some(Path::new("Main.java")));
    fresh.update(e.document.buffer.text(), &[]);
    assert_eq!(
        e.document.syntax.tree().unwrap().root_node().to_sexp(),
        fresh.tree().unwrap().root_node().to_sexp()
    );
}

#[test]
fn nearby_spaces_or_tabs_override_language_defaults() {
    for indent in ["  ", "   ", "    ", "\t"] {
        let source = format!("class Main {{\n{indent}void run() {{}}\n}}\n");
        let mut e = editor("Main.java", &source);
        keys(&mut e, "i");
        let byte = source.find("{}").unwrap() + 1;
        e.document.buffer.set_cursor(byte, true);
        e.handle_key(Key::Enter);
        assert_eq!(
            content(&e),
            format!("class Main {{\n{indent}void run() {{\n{indent}{indent}\n{indent}}}\n}}\n")
        );
        assert_eq!(e.document.buffer.position(), (2, indent.len() * 2));
        // A top-level opener also learns the convention from existing content.
        e.handle_key(Key::Escape);
        keys(&mut e, "ggo");
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.document.buffer.line(1), indent);
    }
}

#[test]
fn comment_margins_do_not_override_language_or_existing_code_indentation() {
    for (path, source, indent) in [
        ("Main.java", "/**\n * Main\n */\nclass Main {", "    "),
        ("Main.kt", "/**\n * Main\n */\nfun main() {", "    "),
        ("main.rs", "/**\n * Main\n */\nfn main() {", "    "),
        ("config.yaml", " # comment\nroot:", "  "),
        ("config.toml", " # comment\nvalues = [", "  "),
        (
            "Main.java",
            "/**\n * Main\n */\nclass Existing {\n  int x;\n}\nclass Main {",
            "  ",
        ),
    ] {
        let mut e = editor(path, source);
        keys(&mut e, "GA");
        e.handle_key(Key::Enter);
        assert_eq!(content(&e), format!("{source}\n{indent}"), "{path}");
    }
}

#[test]
fn open_above_and_below_use_context_and_group_typing_with_newline() {
    for (source, command, row, expected) in [
        ("class Main {\n}\n", "o", 1, "class Main {\n    x\n}\n"),
        ("class Main {\n}\n", "jO", 1, "class Main {\n    x\n}\n"),
        (
            "class Main {\n    int x;\n}\n",
            "jO",
            1,
            "class Main {\n    x\n    int x;\n}\n",
        ),
        (
            "class Main {\n    int x;\n}\n",
            "jo",
            2,
            "class Main {\n    int x;\n    x\n}\n",
        ),
        ("class Main {\n}\n", "jo", 2, "class Main {\n}\nx\n"),
        ("\ttext", "O", 0, "\tx\n\ttext"),
        ("\ttext", "o", 1, "\ttext\n\tx"),
    ] {
        let mut e = editor("Main.java", source);
        keys(&mut e, command);
        assert_eq!(e.document.buffer.position().0, row);
        keys(&mut e, "x");
        e.handle_key(Key::Escape);
        assert_eq!(content(&e), expected, "{command} in {source:?}");
        keys(&mut e, "u");
        assert_eq!(content(&e), source);
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(content(&e), expected);
    }
}

#[test]
fn comments_after_openers_are_ignored_but_delimiters_inside_comments_are_not_code() {
    for (source, expected_indent) in [
        ("class Main { // comment", "    "),
        ("class Main { /* comment */", "    "),
        ("    // {", "    "),
        ("    /* { */", "    "),
        ("    // }", "    "),
    ] {
        let mut e = editor("Main.java", source);
        keys(&mut e, "A");
        e.handle_key(Key::Enter);
        assert_eq!(content(&e), format!("{source}\n{expected_indent}"));
    }
}

#[test]
fn split_inside_string_or_comment_does_not_add_a_block_level() {
    for (path, source) in [
        ("Main.java", "class Main {\n    String s = \"{}\";\n}"),
        ("Main.kt", "fun main() {\n    val s = \"{}\"\n}"),
        ("main.rs", "fn main() {\n    let s = r#\"{}\"#;\n}"),
        ("Main.cs", "class Main {\n    string s = @\"{}\";\n}"),
        ("Main.java", "class Main {\n    /* {} */\n}"),
    ] {
        let mut e = editor(path, source);
        keys(&mut e, "i");
        let byte = source.find("{}").unwrap() + 1;
        e.document.buffer.set_cursor(byte, true);
        e.handle_key(Key::Enter);
        assert_eq!(
            content(&e),
            format!("{}\n    {}", &source[..byte], &source[byte..]),
            "{path}"
        );
    }
}

#[test]
fn enter_before_existing_closer_aligns_with_its_opening_line() {
    let mut e = editor("Main.java", "class Main {\n    int x;}");
    keys(&mut e, "j$i");
    e.handle_key(Key::Enter);
    assert_eq!(content(&e), "class Main {\n    int x;\n}");
    assert_eq!(e.document.buffer.position(), (2, 0));
}

#[test]
fn plain_text_markdown_and_whitespace_only_lines_inherit_exact_indent() {
    for path in ["notes.txt", "README.md"] {
        for source in ["  \ttext {", "  \t", "", "   界e\u{301}👩‍💻"] {
            let mut e = editor(path, source);
            keys(&mut e, "A");
            e.handle_key(Key::Enter);
            assert_eq!(
                content(&e),
                format!(
                    "{source}\n{}",
                    source
                        .chars()
                        .take_while(|ch| matches!(ch, ' ' | '\t'))
                        .collect::<String>()
                )
            );
        }
    }
}

#[test]
fn splitting_leading_whitespace_never_doubles_the_remaining_indent() {
    for column in 0..=4 {
        let mut e = editor("notes.txt", "    text");
        keys(&mut e, "i");
        e.document.buffer.set_cursor(column, true);
        e.handle_key(Key::Enter);
        assert_eq!(content(&e), format!("{}\n    text", " ".repeat(column)));
        assert_eq!(e.document.buffer.position(), (1, column));
    }
}

#[test]
fn splitting_indented_text_removes_only_horizontal_separator_whitespace() {
    let mut e = editor("notes.txt", "    界e\u{301}👩‍💻 \t next\r\n");
    keys(&mut e, "i");
    e.document
        .buffer
        .set_cursor("    界e\u{301}👩‍💻".chars().count(), true);
    e.handle_key(Key::Enter);
    assert_eq!(content(&e), "    界e\u{301}👩‍💻\r\n    next\r\n");
    assert_eq!(e.document.buffer.position(), (1, 4));
}

#[test]
fn bracketed_paste_and_tab_keep_their_literal_whitespace() {
    let mut e = editor("Main.java", "class Main {\r\n}\r\n");
    keys(&mut e, "o");
    e.paste("if (true) {\n x;\n}\n");
    e.handle_key(Key::Tab);
    assert_eq!(
        content(&e),
        "class Main {\r\n    if (true) {\r\n x;\r\n}\r\n\t\r\n}\r\n"
    );
    e.handle_key(Key::Escape);
    keys(&mut e, "u");
    keys(&mut e, "u");
    assert_eq!(content(&e), "class Main {\r\n    \r\n}\r\n");
    keys(&mut e, "u");
    assert_eq!(content(&e), "class Main {\r\n}\r\n");
}

#[test]
fn unavailable_syntax_falls_back_to_copying_indentation() {
    let source = format!(
        "    class Main {{\n{}",
        "x\n".repeat(editor_pro::syntax::MAX_SYNTAX_BYTES / 2)
    );
    let mut e = editor("Main.java", &source);
    keys(&mut e, "A");
    e.handle_key(Key::Enter);
    assert!(e.document.syntax.tree().is_none());
    assert_eq!(e.document.buffer.line(1), "    ");
    assert_eq!(e.document.buffer.position(), (1, 4));
}

#[test]
fn save_as_updates_indentation_language_and_conflicts_keep_unsaved_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Main.java");
    let mut e = editor("notes.txt", "class Main {}");
    e.document.save(Some(&path)).unwrap();
    keys(&mut e, "$i");
    e.handle_key(Key::Enter);
    keys(&mut e, "int x;");
    e.handle_key(Key::Escape);
    assert_eq!(content(&e), "class Main {\n    int x;\n}");
    std::fs::write(&path, "external\n").unwrap();
    assert!(e.document.save(None).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
    assert_eq!(content(&e), "class Main {\n    int x;\n}");
    keys(&mut e, "u");
    assert_eq!(content(&e), "class Main {}");
}
