use editor_pro::{
    buffer::Buffer,
    editor::{Editor, Mode, View},
    file::Document,
    input::{self, Key, Mouse, MouseKind},
    mouse::SourceArea,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

fn editor(text: &str) -> Editor {
    let mut document = Document::default();
    document.buffer = Buffer::new(text);
    Editor::new(document)
}

fn draw(editor: &mut Editor, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, editor, &Theme::night()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn event(kind: MouseKind, column: u16, row: u16) -> Mouse {
    Mouse {
        kind,
        column,
        row,
        shift: false,
        control: false,
    }
}

fn wait(editor: &mut Editor, ready: impl Fn(&Editor) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !ready(editor) {
        editor.tick();
        assert!(Instant::now() < deadline, "{}", editor.message);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn terminal_decoding_ignores_unsupported_buttons_and_modifiers() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let mut raw = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 7,
        row: 2,
        modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    };
    let decoded = input::decode_mouse(raw).unwrap();
    assert!(decoded.control && decoded.shift);
    assert_eq!(
        (decoded.column, decoded.row, decoded.kind),
        (7, 2, MouseKind::Down)
    );
    raw.kind = MouseEventKind::Down(MouseButton::Middle);
    assert!(input::decode_mouse(raw).is_none());
    raw.kind = MouseEventKind::Drag(MouseButton::Right);
    assert!(input::decode_mouse(raw).is_none());
    raw.kind = MouseEventKind::Moved;
    raw.modifiers = KeyModifiers::ALT;
    assert!(input::decode_mouse(raw).is_none());
    raw.modifiers = KeyModifiers::NONE;
    for (kind, expected) in [
        (MouseEventKind::Moved, MouseKind::Move),
        (MouseEventKind::ScrollLeft, MouseKind::ScrollLeft),
        (MouseEventKind::ScrollRight, MouseKind::ScrollRight),
        (
            MouseEventKind::Down(MouseButton::Right),
            MouseKind::RightDown,
        ),
    ] {
        raw.kind = kind;
        assert_eq!(input::decode_mouse(raw).unwrap().kind, expected);
    }
}

#[test]
fn coordinate_mapping_distinguishes_cells_scalars_graphemes_and_crlf() {
    let buffer = Buffer::new("first\r\n\tαe\u{301}界😀Z\r\nlast");
    let mut area = SourceArea {
        area: Rect::new(5, 7, 30, 5),
        gutter: 3,
        top: 1,
        left: 0,
    };
    let start = buffer.text().line_to_char(1);
    for (cell, scalar) in [
        (0, 0),
        (3, 0),
        (4, 1),
        (5, 2),
        (6, 4),
        (7, 4),
        (8, 5),
        (9, 5),
        (10, 6),
        (11, 7),
    ] {
        assert_eq!(
            area.position(&buffer, 8 + cell, 7, false),
            Some(start + scalar)
        );
    }
    assert_eq!(area.position(&buffer, 5, 7, false), Some(start));
    assert_eq!(area.position(&buffer, 5, 7, true), None);
    assert_eq!(area.position(&buffer, 34, 7, true), None);
    assert_eq!(area.position(&buffer, 35, 7, false), None);
    assert_eq!(area.position(&buffer, 8, 11, true), None);
    area.left = 7;
    assert_eq!(area.position(&buffer, 8, 7, false), Some(start + 4));
    area.area = Rect::default();
    assert_eq!(area.position(&buffer, 0, 0, false), None);
}

#[test]
fn clicks_preserve_insert_and_split_undo_groups_and_cancel_operators() {
    let mut e = editor("abc\r\ndef");
    e.settings.relative_line_numbers = true;
    e.handle_key(Key::Char('d'));
    draw(&mut e, 40, 8);
    assert!(e.handle_mouse(event(MouseKind::Down, 4, 1)));
    assert_eq!(e.document.buffer.position(), (1, 1));
    assert!(e.pending_label().is_empty());
    e.handle_key(Key::Char('l'));
    assert_eq!(e.document.buffer.text().to_string(), "abc\r\ndef");
    e.handle_key(Key::Char('i'));
    e.handle_key(Key::Char('X'));
    draw(&mut e, 40, 8);
    e.handle_mouse(event(MouseKind::Down, 39, 0));
    assert_eq!(e.mode, Mode::Insert);
    assert_eq!(e.document.buffer.position(), (0, 3));
    e.handle_key(Key::Char('Y'));
    e.handle_key(Key::Escape);
    e.handle_key(Key::Char('u'));
    assert_eq!(e.document.buffer.text().to_string(), "abc\r\ndeXf");
    e.handle_key(Key::Char('u'));
    assert_eq!(e.document.buffer.text().to_string(), "abc\r\ndef");
}

#[test]
fn dragging_multiline_unicode_uses_visual_yank_and_linewise_selection() {
    let mut e = editor("\tαe\u{301}界\r\nsecond\r\nthird");
    draw(&mut e, 40, 8);
    e.handle_mouse(event(MouseKind::Down, 7, 0));
    e.handle_mouse(event(MouseKind::Drag, 5, 1));
    e.handle_mouse(event(MouseKind::Up, 5, 1));
    assert_eq!(e.mode, Mode::Visual);
    assert_eq!(e.document.buffer.position(), (1, 2));
    e.handle_key(Key::Char('y'));
    assert_eq!(e.register.text, "αe\u{301}界\r\nsec");
    assert!(!e.document.buffer.is_modified());
    e.handle_key(Key::Char('V'));
    draw(&mut e, 40, 8);
    e.handle_mouse(event(MouseKind::Down, 8, 0));
    e.handle_mouse(event(MouseKind::Drag, 4, 2));
    assert_eq!(e.mode, Mode::VisualLine);
    e.handle_key(Key::Char('y'));
    assert!(e.register.linewise);
    assert_eq!(e.register.text, "\tαe\u{301}界\r\nsecond\r\nthird\r\n");
}

#[test]
fn shift_click_extends_from_the_cursor_and_prompts_ignore_source_clicks() {
    let mut e = editor("abc\ndef");
    draw(&mut e, 30, 8);
    e.handle_mouse(Mouse {
        shift: true,
        ..event(MouseKind::Down, 4, 1)
    });
    assert_eq!(e.mode, Mode::Visual);
    assert_eq!(e.selection().unwrap().chars, 0..6);
    e.handle_key(Key::Escape);
    e.handle_key(Key::Char(':'));
    e.handle_key(Key::Char('w'));
    draw(&mut e, 30, 8);
    let cursor = e.document.buffer.cursor();
    assert!(!e.handle_mouse(event(MouseKind::Down, 3, 0)));
    assert_eq!(e.command, "w");
    assert_eq!(e.document.buffer.cursor(), cursor);
    e.handle_mouse(event(MouseKind::RightDown, 3, 0));
    assert_eq!(e.mode, Mode::Normal);
}

#[test]
fn wheel_scrolls_viewport_without_moving_cursor_and_keyboard_follows_again() {
    let source = (0..100)
        .map(|i| format!("{i:03} {}\n", "long ".repeat(15)))
        .collect::<String>();
    let mut e = editor(&source);
    draw(&mut e, 30, 8);
    e.handle_mouse(event(MouseKind::ScrollDown, 10, 2));
    draw(&mut e, 30, 8);
    assert_eq!(e.viewport.top, 3);
    assert_eq!(e.document.buffer.cursor(), 0);
    e.handle_mouse(event(MouseKind::ScrollRight, 10, 2));
    draw(&mut e, 30, 8);
    assert_eq!(e.viewport.left, 4);
    e.handle_mouse(event(MouseKind::Down, 7, 1));
    assert_eq!(e.document.buffer.position(), (4, 6));
    e.handle_mouse(event(MouseKind::Up, 7, 1));
    e.handle_key(Key::Char('g'));
    e.handle_key(Key::Char('g'));
    draw(&mut e, 30, 8);
    assert_eq!(e.viewport.top, 0);
    assert_eq!(e.viewport.left, 0);
    for _ in 0..50 {
        e.handle_mouse(event(MouseKind::ScrollDown, 10, 2));
        draw(&mut e, 30, 8);
    }
    assert_eq!(e.viewport.top, 95);
    assert_eq!(e.document.buffer.cursor(), 0);
}

#[test]
fn held_drag_scrolls_at_viewport_edge_and_release_stops_it() {
    let mut e = editor(&"abcdef\n".repeat(40));
    draw(&mut e, 30, 8);
    e.handle_mouse(event(MouseKind::Down, 4, 1));
    let anchor = e.document.buffer.cursor();
    e.handle_mouse(event(MouseKind::Drag, 8, 7));
    thread::sleep(Duration::from_millis(60));
    e.tick();
    assert_eq!(e.viewport.top, 1);
    assert_eq!(e.selection_anchor, Some(anchor));
    assert_eq!(e.mode, Mode::Visual);
    draw(&mut e, 30, 8);
    e.handle_mouse(event(MouseKind::Up, 8, 7));
    let viewport = e.viewport.clone();
    thread::sleep(Duration::from_millis(60));
    e.tick();
    assert_eq!(e.viewport, viewport);
    assert!(!e.mouse.dragging());
}

#[test]
fn help_preserves_a_mouse_scrolled_source_viewport_until_source_input_resumes() {
    let mut e = editor(&"source\n".repeat(30));
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::ScrollDown, 8, 2));
    draw(&mut e, 40, 10);
    let viewport = e.viewport.clone();
    e.handle_key(Key::F1);
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::ScrollDown, 8, 2));
    e.paste("ignored");
    e.handle_key(Key::Escape);
    draw(&mut e, 40, 10);
    assert_eq!(e.viewport, viewport);
    assert_eq!(e.document.buffer.cursor(), 0);
    e.handle_key(Key::Char('l'));
    draw(&mut e, 40, 10);
    assert_eq!(e.viewport.top, 0);
}

#[test]
fn tree_clicks_fold_open_and_leave_file_prompts_unsubmitted() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("folder")).unwrap();
    fs::write(dir.path().join("folder/source.txt"), "source\r\n").unwrap();
    let mut e = editor("unsaved");
    e.toggle_tree(Some(dir.path())).unwrap();
    wait(&mut e, |e| e.tree.entries.len() == 1);
    draw(&mut e, 40, 10);
    assert!(!e.handle_mouse(event(MouseKind::ScrollRight, 5, 1)));
    assert_eq!(e.tree.entries.len(), 1);
    e.handle_mouse(event(MouseKind::Down, 5, 1));
    wait(&mut e, |e| e.tree.entries.len() == 2);
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::Down, 5, 1));
    assert_eq!(e.tree.entries.len(), 1);
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::Down, 5, 1));
    wait(&mut e, |e| e.tree.entries.len() == 2);
    e.handle_key(Key::Char('a'));
    for ch in "accidental.txt".chars() {
        e.handle_key(Key::Char(ch));
    }
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::Down, 5, 2));
    assert!(!dir.path().join("folder/accidental.txt").exists());
    assert_eq!(e.view, View::Tree);
    e.handle_mouse(event(MouseKind::RightDown, 5, 2));
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::Down, 5, 2));
    assert_eq!(e.view, View::Editing);
    assert!(e.document.path().unwrap().ends_with("source.txt"));
    assert_eq!(e.document.buffer.text().to_string(), "source\r\n");
}

#[test]
fn buffer_and_file_lists_use_visible_rows_and_popup_dismissal_is_local() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a").unwrap();
    fs::write(dir.path().join("b.txt"), "b").unwrap();
    let mut e = editor("original");
    e.open_buffer(&dir.path().join("b.txt")).unwrap();
    e.execute_command("ls").unwrap();
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::Down, 4, 1));
    assert_eq!(e.active_buffer_id, 1);
    e.open_picker(dir.path()).unwrap();
    wait(&mut e, |e| {
        e.picker.as_ref().is_some_and(|p| p.match_count() == 2)
    });
    draw(&mut e, 40, 10);
    let path = e
        .picker
        .as_ref()
        .unwrap()
        .candidate(1)
        .unwrap()
        .path
        .clone();
    e.handle_mouse(event(MouseKind::Down, 4, 3));
    assert_eq!(e.document.path(), Some(dir.path().join(path).as_path()));
    let cursor = e.document.buffer.cursor();
    e.open_help("");
    draw(&mut e, 40, 10);
    e.handle_mouse(event(MouseKind::Down, 1, 9));
    assert!(e.help.is_none());
    assert_eq!(e.document.buffer.cursor(), cursor);
}

#[test]
fn disabled_mouse_and_tiny_or_invalid_regions_never_change_source() {
    let mut e = editor("a界\nb");
    e.mouse.settings.enabled = false;
    draw(&mut e, 30, 8);
    for kind in [
        MouseKind::Down,
        MouseKind::Drag,
        MouseKind::Up,
        MouseKind::RightDown,
        MouseKind::Move,
        MouseKind::ScrollDown,
    ] {
        assert!(!e.handle_mouse(event(kind, 8, 1)));
    }
    assert_eq!(e.document.buffer.cursor(), 0);
    e.mouse.settings.enabled = true;
    for (width, height) in [(0, 0), (1, 1), (1, 2), (2, 3), (30, 8)] {
        draw(&mut e, width, height);
        e.handle_mouse(event(MouseKind::Down, u16::MAX, u16::MAX));
        e.handle_mouse(event(MouseKind::Drag, 0, 0));
        assert_eq!(e.document.buffer.cursor(), 0);
        assert_eq!(e.document.buffer.text().to_string(), "a界\nb");
    }
    assert!(!e.handle_mouse(event(MouseKind::Move, 3, 0)));
    assert!(!e.handle_mouse(event(MouseKind::Move, 3, 0)));
    e.handle_key(Key::Char('i'));
    e.handle_key(Key::Char('X'));
    assert_eq!(e.document.buffer.text().to_string(), "Xa界\nb");
}
