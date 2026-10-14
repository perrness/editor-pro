use editor_pro::{
    buffer::Buffer,
    editor::{Editor, Mode},
    file::Document,
    input::Key,
};
use unicode_segmentation::UnicodeSegmentation;
fn editor(text: &str) -> Editor {
    let mut document = Document::default();
    document.buffer = Buffer::new(text);
    Editor::new(document)
}
fn keys(editor: &mut Editor, keys: &str) {
    for c in keys.chars() {
        editor.handle_key(Key::Char(c));
    }
}
fn text(editor: &Editor) -> String {
    editor.document.buffer.text().to_string()
}
fn escape(editor: &mut Editor) {
    editor.handle_key(Key::Escape);
}

#[test]
fn counts_multiply_and_one_operator_is_one_undo_step() {
    let mut e = editor("one two three four five six seven");
    keys(&mut e, "2d3w");
    assert_eq!(text(&e), "seven");
    assert_eq!(e.register.text, "one two three four five six ");
    keys(&mut e, "u");
    assert_eq!(text(&e), "one two three four five six seven");
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(text(&e), "seven");
}
#[test]
fn motions_distinguish_words_punctuation_and_big_words() {
    let mut e = editor("café.name 界字 👩‍💻 end");
    keys(&mut e, "w");
    assert_eq!(e.document.buffer.cursor(), 4);
    keys(&mut e, "w");
    assert_eq!(e.document.buffer.cursor(), 5);
    keys(&mut e, "b");
    assert_eq!(e.document.buffer.cursor(), 4);
    keys(&mut e, "0W");
    assert_eq!(e.document.buffer.cursor(), 10);
    keys(&mut e, "e");
    assert_eq!(e.document.buffer.cursor(), 11);
    keys(&mut e, "B");
    assert_eq!(e.document.buffer.cursor(), 10);
    keys(&mut e, "0E");
    assert_eq!(e.document.buffer.cursor(), 8);
}
#[test]
fn change_word_preserves_space_and_groups_replacement_with_delete() {
    for (source, command, expected) in [
        ("one two", "cwnew", "new two"),
        ("a two", "cwnew", "new two"),
        ("one two three", "c2wnew", "new three"),
    ] {
        let mut e = editor(source);
        keys(&mut e, command);
        assert_eq!(e.mode, Mode::Insert);
        escape(&mut e);
        assert_eq!(text(&e), expected);
        keys(&mut e, "u");
        assert_eq!(text(&e), source);
    }
}
#[test]
fn delete_char_motion_is_exclusive_and_end_is_inclusive() {
    for (keys_in, expected) in [
        ("dl", "bc"),
        ("ldh", "bc"),
        ("$dl", "ab"),
        ("ld$", "a"),
        ("d0", "abc"),
    ] {
        let mut e = editor("abc");
        keys(&mut e, keys_in);
        assert_eq!(text(&e), expected, "{keys_in}");
    }
}
#[test]
fn cancelled_and_invalid_operators_do_not_leak_counts() {
    let mut e = editor("abcdef");
    keys(&mut e, "3d");
    escape(&mut e);
    keys(&mut e, "x");
    assert_eq!(text(&e), "bcdef");
    keys(&mut e, "dqiZ");
    escape(&mut e);
    assert_eq!(text(&e), "Zbcdef");
}
#[test]
fn unicode_visual_selection_deletes_whole_clusters_in_either_direction() {
    for movement in ["vld", "lvhd"] {
        let mut e = editor("e\u{301}👩‍💻界z");
        keys(&mut e, movement);
        assert_eq!(text(&e), "界z");
        assert_eq!(e.register.text, "e\u{301}👩‍💻");
        keys(&mut e, "u");
        assert_eq!(text(&e), "e\u{301}👩‍💻界z");
    }
}
#[test]
fn visual_multiline_delete_and_change_preserve_crlf_pairs() {
    let mut e = editor("e\u{301}界\r\n👩‍💻end\r\n");
    keys(&mut e, "lvjd");
    assert_eq!(text(&e), "e\u{301}end\r\n");
    keys(&mut e, "uggVjcnew");
    escape(&mut e);
    assert_eq!(text(&e), "new\r\n");
    keys(&mut e, "u");
    assert_eq!(text(&e), "e\u{301}界\r\n👩‍💻end\r\n");
}
#[test]
fn linewise_delete_handles_final_lines_with_and_without_terminator() {
    for (source, commands, expected) in [
        ("a\r\nb\r\nc", "jdd", "a\r\nc"),
        ("a\r\nb", "jdd", "a"),
        ("a\r\nb\r\n", "jdd", "a\r\n"),
        ("a\r\n", "Gdd", "a"),
        ("a\r\nb", "2dd", ""),
    ] {
        let mut e = editor(source);
        keys(&mut e, commands);
        assert_eq!(text(&e), expected, "{source:?}: {commands}");
        keys(&mut e, "u");
        assert_eq!(text(&e), source);
    }
}
#[test]
fn vertical_and_file_motions_compose_linewise() {
    let mut e = editor("a\nb\nc\nd");
    keys(&mut e, "dj");
    assert_eq!(text(&e), "c\nd");
    keys(&mut e, "u3ggdG");
    assert_eq!(text(&e), "a\nb");
    keys(&mut e, "uGdgg");
    assert_eq!(text(&e), "");
}
#[test]
fn yank_and_put_work_for_characters_and_full_lines() {
    let mut e = editor("界e\u{301}z");
    keys(&mut e, "vly$p");
    assert_eq!(text(&e), "界e\u{301}z界e\u{301}");
    keys(&mut e, "u");
    assert_eq!(text(&e), "界e\u{301}z");
    for (source, commands, expected) in [
        ("a\r\nb", "yyp", "a\r\na\r\nb"),
        ("a\r\nb", "GyyP", "a\r\nb\r\nb"),
        ("a", "yy2p", "a\na\na"),
        ("a\n", "yyp", "a\na\n"),
    ] {
        let mut e = editor(source);
        keys(&mut e, commands);
        assert_eq!(text(&e), expected, "{commands}");
        assert!(e.register.linewise);
        keys(&mut e, "u");
        assert_eq!(text(&e), source);
    }
}
#[test]
fn linewise_change_preserves_one_blank_line_and_is_atomic() {
    let mut e = editor("one\r\ntwo\r\nthree");
    keys(&mut e, "jccnew");
    escape(&mut e);
    assert_eq!(text(&e), "one\r\nnew\r\nthree");
    keys(&mut e, "u");
    assert_eq!(text(&e), "one\r\ntwo\r\nthree");
}
#[test]
fn word_objects_preserve_graphemes_and_around_space() {
    let mut e = editor("a e\u{301}cole  z");
    keys(&mut e, "wdiw");
    assert_eq!(text(&e), "a   z");
    keys(&mut e, "uwwdaw");
    assert_eq!(text(&e), "a e\u{301}cole");
    let mut e = editor("one.two three");
    keys(&mut e, "diW");
    assert_eq!(text(&e), " three");
}
#[test]
fn nested_multiline_pairs_and_escaped_quotes_are_text_objects() {
    let mut e = editor("outer(a, (界\r\n👩‍💻), z)");
    keys(&mut e, "f界di(");
    assert_eq!(text(&e), "outer(a, (), z)");
    keys(&mut e, "u2di(");
    assert_eq!(text(&e), "outer()");
    let mut e = editor("say(\"a\\\"界\")");
    keys(&mut e, "f界ci\"new");
    escape(&mut e);
    assert_eq!(text(&e), "say(\"new\")");
    keys(&mut e, "u");
    assert_eq!(text(&e), "say(\"a\\\"界\")");
}
#[test]
fn visual_objects_and_anchor_swap_work() {
    let mut e = editor("one two three");
    keys(&mut e, "wviwy");
    assert_eq!(e.register.text, "two");
    keys(&mut e, "vloe");
    assert_eq!(e.mode, Mode::Visual);
    escape(&mut e);
    assert!(e.selection_anchor.is_none());
}
#[test]
fn find_till_repeat_and_matching_pairs_are_composable() {
    let mut e = editor("a界b界c");
    keys(&mut e, "f界;");
    assert_eq!(e.document.buffer.cursor(), 3);
    keys(&mut e, ",");
    assert_eq!(e.document.buffer.cursor(), 1);
    keys(&mut e, "0dt界");
    assert_eq!(text(&e), "界b界c");
    let mut e = editor("(a(界)b) end");
    keys(&mut e, "d%");
    assert_eq!(text(&e), " end");
}
#[test]
fn missing_objects_and_find_targets_leave_text_and_history_intact() {
    let mut e = editor("one two");
    keys(&mut e, "di(f界");
    assert_eq!(text(&e), "one two");
    assert!(!e.document.buffer.undo());
    assert_eq!(e.mode, Mode::Normal);
}
#[test]
fn mixed_modal_sequences_keep_cursor_and_selection_on_grapheme_boundaries() {
    let mut e = editor("a\u{301} 👩‍💻 (界\r\nxyz)\r\n");
    let actions: Vec<_> = "vwbedcyipP()1230$hljkguWBEf界"
        .chars()
        .map(Key::Char)
        .chain([Key::Escape, Key::Ctrl('r'), Key::Backspace])
        .collect();
    let mut random = 41_u64;
    for step in 0..2_000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        e.handle_key(actions[(random >> 32) as usize % actions.len()]);
        if e.document.buffer.text().len_chars() > 2000 {
            escape(&mut e);
            keys(&mut e, "ggdG");
        }
        let source = text(&e);
        let mut boundaries = vec![0];
        for g in source.graphemes(true) {
            boundaries.push(boundaries.last().unwrap() + g.chars().count());
        }
        assert!(
            boundaries.contains(&e.document.buffer.cursor()),
            "step {step}: {source:?}"
        );
        if let Some(s) = e.selection() {
            assert!(boundaries.contains(&s.chars.start));
            assert!(boundaries.contains(&s.chars.end));
        }
    }
}

#[test]
fn empty_line_end_operator_does_not_join_lines_and_backward_find_is_inclusive() {
    let mut e = editor("\r\nnext");
    keys(&mut e, "d$");
    assert_eq!(text(&e), "\r\nnext");
    let mut e = editor("abcde");
    keys(&mut e, "$dFb");
    assert_eq!(text(&e), "a");
    keys(&mut e, "u$dTb");
    assert_eq!(text(&e), "ab");
}

#[test]
fn repeated_till_search_skips_the_adjacent_previous_target() {
    let mut e = editor("a,b,c,d");
    keys(&mut e, "t,;;");
    assert_eq!(e.document.buffer.cursor(), 4);
    let mut e = editor("a,b,c,d");
    keys(&mut e, "$T,;;");
    assert_eq!(e.document.buffer.cursor(), 2);
}
#[test]
fn linewise_register_can_put_an_empty_line_into_an_empty_buffer() {
    let mut e = editor("");
    keys(&mut e, "yyp");
    assert_eq!(text(&e), "\n");
    keys(&mut e, "u");
    assert_eq!(text(&e), "");
}
#[test]
fn large_horizontal_counts_are_grapheme_based_and_clamped() {
    let mut e = editor(&"界".repeat(12_000));
    keys(&mut e, "10000l");
    assert_eq!(e.document.buffer.cursor(), 10_000);
    keys(&mut e, "10000x");
    assert_eq!(e.document.buffer.text().len_chars(), 10_000);
    keys(&mut e, "u");
    assert_eq!(e.document.buffer.text().len_chars(), 12_000);
}
