use editor_pro::{
    buffer::Buffer,
    editor::{Editor, Mode},
    file::Document,
    input::Key,
    search::Search,
};
fn editor(source: &str) -> Editor {
    let mut d = Document::default();
    d.buffer = Buffer::new(source);
    Editor::new(d)
}
fn keys(e: &mut Editor, keys: &str) {
    for c in keys.chars() {
        e.handle_key(Key::Char(c));
    }
}
fn enter(e: &mut Editor) {
    e.handle_key(Key::Enter);
}
#[test]
fn search_navigates_both_directions_with_counts_and_wraps() {
    let mut e = editor("one 界 one\r\none");
    keys(&mut e, "/one");
    enter(&mut e);
    assert_eq!(e.document.buffer.cursor(), 6);
    keys(&mut e, "n");
    assert_eq!(e.document.buffer.position(), (1, 0));
    keys(&mut e, "n");
    assert_eq!(e.document.buffer.cursor(), 0);
    assert!(e.message.contains("wrapped"));
    keys(&mut e, "N");
    assert_eq!(e.document.buffer.position(), (1, 0));
    keys(&mut e, "2n");
    assert_eq!(e.document.buffer.cursor(), 6);
    keys(&mut e, "?one");
    enter(&mut e);
    assert_eq!(e.document.buffer.cursor(), 0);
    keys(&mut e, "n");
    assert_eq!(e.document.buffer.position(), (1, 0));
}
#[test]
fn literal_search_never_lands_inside_graphemes_or_crlf() {
    let mut b = Buffer::new("e\u{301} e 👩‍💻 👩\r\n界");
    let mut s = Search::default();
    s.set("e", false);
    s.refresh(&b);
    assert_eq!(s.matches.len(), 1);
    assert_eq!(s.matches[0].chars, 3..4);
    s.set("👩", false);
    s.navigate(&mut b, false, 1).unwrap();
    assert_eq!(s.matches.len(), 1);
    assert_eq!(b.cursor(), 9);
    s.set("\u{301}", false);
    s.refresh(&b);
    assert!(s.matches.is_empty());
    s.set("\r", false);
    s.refresh(&b);
    assert!(s.matches.is_empty());
    s.set("e\u{301}", false);
    s.navigate(&mut b, false, 1).unwrap();
    assert_eq!(b.cursor(), 0);
}
#[test]
fn cancelled_search_keeps_previous_query_cursor_and_viewport() {
    let mut e = editor("one two one");
    keys(&mut e, "/one");
    enter(&mut e);
    let cursor = e.document.buffer.cursor();
    e.viewport.left = 7;
    e.viewport.top = 0;
    keys(&mut e, "?two");
    e.handle_key(Key::Escape);
    assert_eq!(e.mode, Mode::Normal);
    assert_eq!(e.search.query, "one");
    assert!(!e.search.backward);
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.viewport.left, 7);
    keys(&mut e, "/");
    enter(&mut e);
    assert_eq!(e.document.buffer.cursor(), 0);
}
#[test]
fn missing_pattern_does_not_move_cursor_or_modify_history() {
    let mut e = editor("one");
    keys(&mut e, "/.*");
    enter(&mut e);
    assert!(e.message_is_error);
    assert_eq!(e.document.buffer.cursor(), 0);
    assert!(!e.document.buffer.undo());
}
#[test]
fn search_cache_refreshes_after_edit_undo_and_redo() {
    let mut e = editor("one one");
    keys(&mut e, "/one");
    enter(&mut e);
    keys(&mut e, "dw");
    e.search.refresh(&e.document.buffer);
    assert_eq!(e.search.matches.len(), 1);
    keys(&mut e, "u");
    e.search.refresh(&e.document.buffer);
    assert_eq!(e.search.matches.len(), 2);
    e.handle_key(Key::Ctrl('r'));
    e.search.refresh(&e.document.buffer);
    assert_eq!(e.search.matches.len(), 1);
}
#[test]
fn search_extends_visual_selection_and_escape_preserves_visual_line_mode() {
    let mut e = editor("one\ntwo\nthree");
    keys(&mut e, "V/three");
    e.handle_key(Key::Escape);
    assert_eq!(e.mode, Mode::VisualLine);
    keys(&mut e, "/three");
    enter(&mut e);
    assert_eq!(e.mode, Mode::VisualLine);
    assert!(e.selection().unwrap().linewise);
    keys(&mut e, "y");
    assert_eq!(e.register.text, "one\ntwo\nthree\n");
}
#[test]
fn search_highlights_can_be_hidden_and_repeat_restores_them() {
    let mut e = editor("one one");
    keys(&mut e, "/one");
    enter(&mut e);
    e.execute_command("noh").unwrap();
    assert!(!e.search.visible);
    keys(&mut e, "n");
    assert!(e.search.visible);
}
