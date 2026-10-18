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
fn typed_delimiters_pair_with_the_cursor_inside_and_share_insert_undo() {
    for (open, close) in [
        ('(', ')'),
        ('[', ']'),
        ('{', '}'),
        ('"', '"'),
        ('\'', '\''),
        ('`', '`'),
    ] {
        let mut e = editor("\r\n");
        keys(&mut e, &format!("i{open}"));
        assert_eq!(content(&e), format!("{open}{close}\r\n"));
        assert_eq!(e.document.buffer.position(), (0, 1));
        let edits = e.document.buffer.take_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_end_byte, 2);

        let inside = "界e\u{301}👩‍💻";
        keys(&mut e, inside);
        let generation = e.document.buffer.generation();
        e.handle_key(Key::Char(close));
        assert_eq!(e.document.buffer.generation(), generation);
        assert_eq!(e.document.buffer.cursor(), inside.chars().count() + 2);
        let expected = format!("{open}{inside}{close}\r\n");
        assert_eq!(content(&e), expected);
        e.handle_key(Key::Escape);
        keys(&mut e, "u");
        assert_eq!(content(&e), "\r\n");
        assert!(!e.document.buffer.is_modified());
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(content(&e), expected);
    }
}

#[test]
fn nested_pairs_close_without_duplicates_across_crlf_lines() {
    let mut e = editor("\r\n");
    keys(&mut e, "i([{");
    assert_eq!(content(&e), "([{}])\r\n");
    assert_eq!(e.document.buffer.cursor(), 3);
    e.handle_key(Key::Enter);
    keys(&mut e, "\"value\"}])");
    assert_eq!(content(&e), "([{\r\n\"value\"}])\r\n");
    assert_eq!(e.document.buffer.position(), (1, 10));
    e.handle_key(Key::Escape);
    keys(&mut e, "u");
    assert_eq!(content(&e), "\r\n");
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(content(&e), "([{\r\n\"value\"}])\r\n");
}

#[test]
fn typing_existing_closers_moves_without_modifying_text_or_history() {
    for close in [')', ']', '}', '"', '\'', '`'] {
        let original = format!("{close} tail");
        let mut e = editor(&original);
        keys(&mut e, "ix");
        e.handle_key(Key::Escape);
        keys(&mut e, "ui");
        let generation = e.document.buffer.generation();
        e.handle_key(Key::Char(close));
        assert_eq!(content(&e), original);
        assert_eq!(e.document.buffer.cursor(), 1);
        assert_eq!(e.document.buffer.generation(), generation);
        e.handle_key(Key::Escape);
        assert!(!e.document.buffer.is_modified());
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(content(&e), format!("x{original}"));
    }
}

#[test]
fn backspace_removes_empty_pairs_as_one_edit_and_keeps_no_op_history() {
    for pair in ["()", "[]", "{}", "\"\"", "''", "``"] {
        let mut e = editor(pair);
        keys(&mut e, "a");
        e.handle_key(Key::Backspace);
        assert_eq!(content(&e), "");
        assert_eq!(e.document.buffer.cursor(), 0);
        assert_eq!(e.document.buffer.take_edits().len(), 1);
        e.handle_key(Key::Escape);
        keys(&mut e, "u");
        assert_eq!(content(&e), pair);
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(content(&e), "");

        let mut e = editor("");
        keys(&mut e, "ix");
        e.handle_key(Key::Escape);
        keys(&mut e, "ui");
        e.handle_key(Key::Char(pair.chars().next().unwrap()));
        e.handle_key(Key::Backspace);
        e.handle_key(Key::Escape);
        assert_eq!(content(&e), "");
        assert!(!e.document.buffer.is_modified());
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(content(&e), "x");
    }
}

#[test]
fn escaped_quotes_are_literal_and_even_backslashes_allow_pairing() {
    for quote in ['"', '\'', '`'] {
        for backslashes in 0..=4 {
            let prefix = "\\".repeat(backslashes);
            let mut e = editor(&prefix);
            keys(&mut e, "A");
            e.handle_key(Key::Char(quote));
            let suffix = if backslashes % 2 == 0 {
                format!("{quote}{quote}")
            } else {
                quote.to_string()
            };
            assert_eq!(content(&e), format!("{prefix}{suffix}"));
            assert_eq!(e.document.buffer.cursor(), backslashes + 1);
        }
        let mut e = editor(&format!("\\{quote}"));
        keys(&mut e, "a");
        e.handle_key(Key::Char(quote));
        assert_eq!(content(&e), format!("\\{quote}{quote}"));
        e.handle_key(Key::Backspace);
        assert_eq!(content(&e), format!("\\{quote}"));
    }
    let mut e = editor("");
    keys(&mut e, "i\"hello\\\"world\"");
    assert_eq!(content(&e), "\"hello\\\"world\"");
}

#[test]
fn pairing_respects_existing_words_apostrophes_and_comparison_operators() {
    for open in ['(', '[', '{', '"', '\'', '`'] {
        let mut e = editor("word");
        keys(&mut e, "i");
        e.handle_key(Key::Char(open));
        assert_eq!(content(&e), format!("{open}word"));
    }
    for word in ["don", "界", "e\u{301}", "name_"] {
        let mut e = editor(word);
        keys(&mut e, "A't");
        assert_eq!(content(&e), format!("{word}'t"));
    }
    for suffix in [" ", "\t", ")", "]", "}", ",", ";", ":"] {
        let mut e = editor(suffix);
        keys(&mut e, "i(");
        assert_eq!(content(&e), format!("(){suffix}"));
    }
    let mut e = editor("");
    keys(&mut e, "ia < b > c");
    assert_eq!(content(&e), "a < b > c");
}

#[test]
fn pair_backspace_preserves_neighboring_graphemes_and_nonempty_pairs() {
    for (original, cursor, expected) in [
        ("界e\u{301}()👩‍💻", 4, "界e\u{301}👩‍💻"),
        ("(\u{301})", 2, ")"),
        ("()\u{301}", 1, ")\u{301}"),
        ("(x)", 2, "()"),
        ("(]", 1, "]"),
        ("", 0, ""),
    ] {
        let mut e = editor(original);
        keys(&mut e, "i");
        e.document.buffer.set_cursor(cursor, true);
        e.handle_key(Key::Backspace);
        assert_eq!(content(&e), expected);
    }
}

#[test]
fn pairing_does_not_transform_paste_registers_or_prompts() {
    let literal = "([{'\"`<>";
    let mut e = editor("");
    keys(&mut e, "i(");
    e.paste(literal);
    assert_eq!(content(&e), format!("({literal})"));
    e.handle_key(Key::Char(')'));
    e.handle_key(Key::Escape);
    keys(&mut e, "u");
    assert_eq!(content(&e), "()");
    keys(&mut e, "u");
    assert_eq!(content(&e), "");
    e.register.text = literal.into();
    keys(&mut e, "p");
    assert_eq!(content(&e), literal);

    for prefix in [":", "/", "?"] {
        e.handle_key(Key::Escape);
        keys(&mut e, prefix);
        keys(&mut e, literal);
        assert_eq!(e.command, literal);
        assert_eq!(content(&e), literal);
    }
}

#[test]
fn insert_movement_keeps_pair_insertion_and_later_typing_in_separate_undo_groups() {
    let mut e = editor("");
    keys(&mut e, "i(");
    e.handle_key(Key::Right);
    keys(&mut e, "x");
    e.handle_key(Key::Escape);
    keys(&mut e, "u");
    assert_eq!(content(&e), "()");
    keys(&mut e, "u");
    assert_eq!(content(&e), "");
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
fn navigation_shortcuts_do_not_intercept_insert_visual_or_operator_input() {
    for prefix in ["v", "V", "d", "c", "y", "2d3"] {
        for shortcut in ["gd", "gr"] {
            let mut editor = editor("first\r\nsecond\r\n");
            keys(&mut editor, prefix);
            let mode = editor.mode;
            let anchor = editor.selection_anchor;
            let message = editor.message.clone();
            keys(&mut editor, shortcut);
            assert_eq!(editor.mode, mode);
            assert_eq!(editor.selection_anchor, anchor);
            assert_eq!(editor.message, message);
            assert_eq!(content(&editor), "first\r\nsecond\r\n");
            assert!(editor.pending_label().is_empty());
        }
    }
    let mut editor = editor("");
    keys(&mut editor, "igdgr");
    assert_eq!(content(&editor), "gdgr");
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn git_shortcut_prefixes_cancel_and_do_not_consume_later_editing_keys() {
    for prefix in ["[", "]", "3]", " ", " g"] {
        for cancel in [Key::Escape, Key::Ctrl('c'), Key::Ctrl('[')] {
            let mut editor = editor("abc\r\n");
            keys(&mut editor, prefix);
            assert!(!editor.pending_label().is_empty());
            editor.handle_key(cancel);
            assert!(editor.pending_label().is_empty());
            keys(&mut editor, "liX");
            assert_eq!(content(&editor), "aXbc\r\n");
        }
    }
    for invalid in ["]x", "[d", "3]g", " gx", " d"] {
        let mut editor = editor("abc");
        keys(&mut editor, invalid);
        assert_eq!(content(&editor), "abc");
        assert!(editor.pending_label().is_empty());
        keys(&mut editor, "l");
        assert_eq!(editor.document.buffer.position(), (0, 1));
    }
}

#[test]
fn git_shortcuts_preserve_insert_text_objects_find_and_operator_input() {
    let mut e = editor("");
    keys(&mut e, "i]c[c]f[f gd gc");
    assert_eq!(content(&e), "]c[c]f[f gd gc]");
    assert_eq!(e.mode, Mode::Insert);
    let mut e = editor("a[b]c");
    keys(&mut e, "f]");
    assert_eq!(e.document.buffer.position(), (0, 3));
    keys(&mut e, "F[");
    assert_eq!(e.document.buffer.position(), (0, 1));
    keys(&mut e, "di[");
    assert_eq!(content(&e), "a[]c");
    let mut e = editor("a[b]c");
    keys(&mut e, "df]");
    assert_eq!(content(&e), "c");
    for prefix in ["v", "V", "d", "c", "y"] {
        for bracket in ['[', ']'] {
            let mut e = editor("abc");
            keys(&mut e, prefix);
            let mode = e.mode;
            let anchor = e.selection_anchor;
            e.handle_key(Key::Char(bracket));
            assert_eq!(e.mode, mode);
            assert_eq!(e.selection_anchor, anchor);
            assert!(e.pending_label().is_empty());
            assert_eq!(content(&e), "abc");
        }
    }
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
        Key::Char('('),
        Key::Char(')'),
        Key::Char('['),
        Key::Char(']'),
        Key::Char('{'),
        Key::Char('}'),
        Key::Char('"'),
        Key::Char('\''),
        Key::Char('`'),
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

#[test]
fn repeated_undo_redo_walks_insert_and_operator_transactions() {
    let mut e = editor("a\u{301}界\r\nlast");
    let mut states = vec![content(&e)];
    keys(&mut e, "A👨‍👩‍👧‍👦");
    e.handle_key(Key::Enter);
    keys(&mut e, "new");
    e.handle_key(Key::Escape);
    states.push(content(&e));
    keys(&mut e, "ggdd");
    states.push(content(&e));
    keys(&mut e, "cwchanged");
    e.handle_key(Key::Escape);
    states.push(content(&e));
    for expected in states[..3].iter().rev() {
        keys(&mut e, "u");
        assert_eq!(&content(&e), expected);
        assert!(e.selection().is_none());
        let cursor = e.document.buffer.cursor();
        assert!(
            content(&e)
                .grapheme_indices(true)
                .any(|(byte, _)| { content(&e)[..byte].chars().count() == cursor })
                || cursor == content(&e).chars().count()
        );
    }
    keys(&mut e, "u");
    assert_eq!(content(&e), states[0]);
    for expected in &states[1..] {
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(&content(&e), expected);
    }
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(content(&e), states[3]);
    keys(&mut e, "2u");
    assert_eq!(content(&e), states[1]);
    keys(&mut e, "iX");
    e.handle_key(Key::Escape);
    let branched = content(&e);
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(content(&e), branched);
}
