use editor_pro::{
    buffer::Buffer,
    editor::{Editor, Mode},
    file::Document,
    input::{self, Key},
    text::{display_column, visible_text},
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

fn editor(text: &str) -> Editor {
    let mut document = Document::default();
    document.buffer = Buffer::new(text);
    Editor::new(document)
}

fn keys(editor: &mut Editor, text: &str) {
    for character in text.chars() {
        editor.handle_key(Key::Char(character));
    }
}

fn content(editor: &Editor) -> String {
    editor.document.buffer.text().to_string()
}

#[test]
fn insert_session_is_one_undo_step_and_redo_restores_it() {
    let mut editor = editor("");
    keys(&mut editor, "ihello");
    editor.handle_key(Key::Enter);
    keys(&mut editor, "world");
    editor.handle_key(Key::Escape);
    assert_eq!(editor.mode, Mode::Normal);
    assert_eq!(content(&editor), "hello\nworld");
    assert_eq!(editor.document.buffer.position(), (1, 4));
    keys(&mut editor, "u");
    assert_eq!(content(&editor), "");
    assert!(!editor.document.buffer.is_modified());
    editor.handle_key(Key::Ctrl('r'));
    assert_eq!(content(&editor), "hello\nworld");
    assert!(editor.document.buffer.is_modified());
}

#[test]
fn unicode_motions_and_deletion_use_extended_graphemes() {
    let mut editor = editor("a\u{301}👨‍👩‍👧‍👦界z");
    keys(&mut editor, "l");
    assert_eq!(editor.document.buffer.position(), (0, 2));
    keys(&mut editor, "x");
    assert_eq!(content(&editor), "a\u{301}界z");
    keys(&mut editor, "hxi");
    assert_eq!(content(&editor), "界z");
    keys(&mut editor, "e\u{301}");
    editor.handle_key(Key::Backspace);
    assert_eq!(content(&editor), "界z");
}

#[test]
fn combining_text_join_keeps_cursor_on_a_boundary() {
    let mut buffer = Buffer::new("👩👧");
    buffer.move_right(true);
    buffer.insert("\u{200d}");
    assert_eq!(buffer.text().to_string(), "👩‍👧");
    assert_eq!(buffer.cursor(), 3);
    buffer.backspace();
    assert_eq!(buffer.text().to_string(), "");
}

#[test]
fn vertical_movement_preserves_desired_terminal_column() {
    let mut buffer = Buffer::new("abcdef\nx\n\t界xyz");
    buffer.line_end(false);
    buffer.move_vertical(1, false);
    assert_eq!(buffer.position(), (1, 0));
    buffer.move_vertical(1, false);
    assert_eq!(buffer.position(), (2, 1));
    buffer.move_vertical(-2, false);
    assert_eq!(buffer.position(), (0, 5));
}

#[test]
fn normal_mode_clamps_to_characters_and_insert_can_reach_end() {
    let mut editor = editor("ab\n\nx");
    keys(&mut editor, "llll");
    assert_eq!(editor.document.buffer.position(), (0, 1));
    keys(&mut editor, "a");
    assert_eq!(editor.document.buffer.position(), (0, 2));
    editor.handle_key(Key::Escape);
    keys(&mut editor, "jjl");
    assert_eq!(editor.document.buffer.position(), (2, 0));
    keys(&mut editor, "gg");
    assert_eq!(editor.document.buffer.position(), (0, 0));
    keys(&mut editor, "G");
    assert_eq!(editor.document.buffer.position(), (2, 0));
}

#[test]
fn cancelled_g_prefix_does_not_leak_into_later_keys() {
    let mut editor = editor("ab\ncd");
    keys(&mut editor, "Gg");
    editor.handle_key(Key::Escape);
    keys(&mut editor, "g");
    assert_eq!(editor.document.buffer.position(), (1, 0));
    keys(&mut editor, "g");
    assert_eq!(editor.document.buffer.position(), (0, 0));
}

#[test]
fn crlf_line_join_and_newline_are_atomic_and_preserved() {
    let mut editor = editor("ab\r\ncd\r\n");
    keys(&mut editor, "ji");
    editor.handle_key(Key::Backspace);
    assert_eq!(content(&editor), "abcd\r\n");
    editor.handle_key(Key::Enter);
    assert_eq!(content(&editor), "ab\r\ncd\r\n");
    editor.handle_key(Key::Escape);
    assert!(!editor.document.buffer.is_modified());
}

#[test]
fn delete_joins_lines_only_in_insert_mode() {
    let mut editor = editor("a\nb");
    keys(&mut editor, "x");
    assert_eq!(content(&editor), "\nb");
    keys(&mut editor, "x");
    assert_eq!(content(&editor), "\nb");
    keys(&mut editor, "i");
    editor.handle_key(Key::Delete);
    assert_eq!(content(&editor), "b");
}

#[test]
fn open_lines_group_newline_and_typing_for_undo() {
    for (command, expected) in [('o', "one\nnew\ntwo"), ('O', "new\none\ntwo")] {
        let mut editor = editor("one\ntwo");
        editor.handle_key(Key::Char(command));
        keys(&mut editor, "new");
        editor.handle_key(Key::Escape);
        assert_eq!(content(&editor), expected);
        keys(&mut editor, "u");
        assert_eq!(content(&editor), "one\ntwo");
    }
}

#[test]
fn insert_movement_breaks_undo_group() {
    let mut editor = editor("");
    keys(&mut editor, "iab");
    editor.handle_key(Key::Left);
    keys(&mut editor, "x");
    editor.handle_key(Key::Escape);
    keys(&mut editor, "u");
    assert_eq!(content(&editor), "ab");
    keys(&mut editor, "u");
    assert_eq!(content(&editor), "");
}

#[test]
fn capital_i_inserts_after_indentation() {
    let mut editor = editor(" \tword");
    keys(&mut editor, "Ix");
    editor.handle_key(Key::Escape);
    assert_eq!(content(&editor), " \txword");
    keys(&mut editor, "u");
    assert_eq!(content(&editor), " \tword");
}

#[test]
fn savepoint_survives_undo_redo_and_branching() {
    let mut buffer = Buffer::new("original");
    buffer.insert("a");
    buffer.commit();
    buffer.mark_saved();
    assert!(!buffer.is_modified());
    assert!(buffer.undo());
    assert!(buffer.is_modified());
    assert!(buffer.redo());
    assert!(!buffer.is_modified());
    buffer.insert("b");
    buffer.commit();
    assert!(buffer.undo());
    buffer.insert("c");
    buffer.commit();
    assert!(!buffer.redo());
    assert!(buffer.is_modified());
}

#[test]
fn no_op_insert_preserves_redo() {
    let mut buffer = Buffer::new("a");
    buffer.insert("b");
    buffer.commit();
    buffer.undo();
    buffer.begin();
    buffer.commit();
    assert!(buffer.redo());
}

#[test]
fn bracketed_paste_never_executes_commands() {
    let mut editor = editor("");
    editor.paste(":q!\n");
    assert_eq!(content(&editor), "");
    assert!(!editor.quit);
    keys(&mut editor, "i");
    editor.paste(":q!\r\n界\rnext");
    editor.handle_key(Key::Escape);
    assert_eq!(content(&editor), ":q!\n界\nnext");
    assert!(!editor.quit);
    keys(&mut editor, "u");
    assert_eq!(content(&editor), "");
}

#[test]
fn dirty_quit_and_failed_write_quit_keep_editor_alive() {
    let mut editor = editor("");
    keys(&mut editor, "ix");
    editor.handle_key(Key::Escape);
    assert!(editor.execute_command("q").is_err());
    assert!(editor.execute_command("wq").is_err());
    assert!(!editor.quit);
    editor.execute_command("q!").unwrap();
    assert!(editor.quit);
}

#[test]
fn command_line_accepts_spaces_and_grapheme_backspace() {
    let mut editor = editor("");
    keys(&mut editor, ":w e\u{301}");
    editor.handle_key(Key::Backspace);
    assert_eq!(editor.command, "w ");
    editor.handle_key(Key::Escape);
    assert_eq!(editor.mode, Mode::Normal);
    assert!(editor.command.is_empty());
}

#[test]
fn text_clipping_accounts_for_tabs_wide_characters_and_controls() {
    assert_eq!(display_column("a\t界e\u{301}", 5), 7);
    assert_eq!(visible_text("a\t界z", 2, 5), "  界z");
    assert_eq!(visible_text("界z", 1, 2), " z");
    assert_eq!(visible_text("a界z", 0, 2), "a ");
    assert_eq!(visible_text("\x1b[31m", 0, 10), "�[31m");
    assert_eq!(visible_text("\u{301}", 0, 1), "◌\u{301}");
}

#[test]
fn key_release_and_alt_shortcuts_are_ignored() {
    let mut key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
    key.kind = KeyEventKind::Release;
    assert_eq!(input::decode(key), None);
    assert_eq!(
        input::decode(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)),
        None
    );
    assert_eq!(
        input::decode(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        Some(Key::Ctrl('r'))
    );
}

#[test]
fn mixed_edit_sequences_never_leave_cursor_inside_a_grapheme() {
    let mut editor = editor("a\u{301}\t界👩‍💻\r\nnext\r\n");
    let actions = [
        Key::Char('i'),
        Key::Char('界'),
        Key::Escape,
        Key::Char('x'),
        Key::Char('u'),
        Key::Ctrl('r'),
        Key::Char('j'),
        Key::Char('k'),
        Key::Char('h'),
        Key::Char('l'),
        Key::Char('a'),
        Key::Backspace,
        Key::Delete,
        Key::Enter,
        Key::Char('o'),
        Key::Char('G'),
        Key::Char('g'),
    ];
    let mut random = 17_u64;
    for _ in 0..2_000 {
        random = random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        editor.handle_key(actions[(random >> 32) as usize % actions.len()]);
        let buffer = &editor.document.buffer;
        let (line, column) = buffer.position();
        let text = buffer.line(line);
        let mut boundaries = vec![0];
        for grapheme in text.graphemes(true) {
            boundaries.push(boundaries.last().unwrap() + grapheme.chars().count());
        }
        assert!(
            boundaries.contains(&column),
            "cursor {column} inside {text:?}"
        );
        assert!(buffer.cursor() <= buffer.text().len_chars());
    }
}
