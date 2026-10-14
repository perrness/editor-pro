use editor_pro::{buffer::Buffer, editor::Editor, file::Document, theme::Theme, ui};
use ratatui::{Terminal, backend::TestBackend};

#[test]
fn render_shows_text_mode_filename_and_dirty_marker() {
    let mut document = Document::default();
    document.buffer.insert("fn main() {}\n界");
    document.buffer.commit();
    document.buffer.goto_line(0, false);
    let mut editor = Editor::new(document);
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut editor, &Theme::night()))
        .unwrap();
    let output: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    for expected in ["fn main() {}", "NORMAL", "[No Name]", "[+]", "UTF-8"] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
}

#[test]
fn tiny_terminals_and_long_lines_do_not_panic_or_lose_cursor() {
    for (width, height) in [(1, 1), (1, 3), (2, 2), (8, 4), (80, 24)] {
        let mut document = Document::default();
        document.buffer = Buffer::new(&format!("{}界\nshort\n", "\t界".repeat(100)));
        document.buffer.line_end(false);
        let mut editor = Editor::new(document);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &mut editor, &Theme::night()))
            .unwrap();
        if height > 2 {
            let position = terminal.get_cursor_position().unwrap();
            assert!(position.x < width && position.y < height);
        }
    }
}

#[test]
fn vertical_viewport_tracks_cursor_and_resize() {
    let mut document = Document::default();
    document.buffer = Buffer::new(&"line\n".repeat(100));
    document.buffer.goto_line(90, false);
    let mut editor = Editor::new(document);
    let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut editor, &Theme::night()))
        .unwrap();
    assert!(editor.viewport.top <= 90);
    assert!(editor.viewport.top + editor.viewport.height > 90);
    terminal.backend_mut().resize(12, 3);
    terminal
        .draw(|frame| ui::render(frame, &mut editor, &Theme::night()))
        .unwrap();
    assert_eq!(editor.viewport.height, 1);
    assert_eq!(editor.viewport.top, 90);
    editor.document.buffer.goto_line(0, false);
    terminal
        .draw(|frame| ui::render(frame, &mut editor, &Theme::night()))
        .unwrap();
    assert_eq!(editor.viewport.top, 0);
}

#[test]
fn visual_selection_colors_complete_wide_and_combining_graphemes() {
    use editor_pro::input::Key;
    let mut document = Document::default();
    document.buffer = Buffer::new("e\u{301}界z\r\nnext");
    let mut editor = Editor::new(document);
    editor.handle_key(Key::Char('v'));
    editor.handle_key(Key::Char('l'));
    let theme = Theme::night();
    let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut editor, &theme))
        .unwrap();
    let cells = terminal.backend().buffer();
    assert_eq!(cells[(3, 0)].symbol(), "e\u{301}");
    assert_eq!(cells[(3, 0)].bg, theme.selection);
    assert_eq!(cells[(4, 0)].symbol(), "界");
    assert_eq!(cells[(4, 0)].bg, theme.selection);
    assert_ne!(cells[(6, 0)].bg, theme.selection);
    editor.handle_key(Key::Char('V'));
    editor.handle_key(Key::Char('j'));
    terminal
        .draw(|frame| ui::render(frame, &mut editor, &theme))
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(20, 1)].bg, theme.selection);
}

#[test]
fn search_highlight_and_prompt_preserve_selection_and_narrow_cursor() {
    use editor_pro::input::Key;
    let mut document = Document::default();
    document.buffer = Buffer::new("one one\nnext");
    let mut editor = Editor::new(document);
    let theme = Theme::night();
    for ch in "/one".chars() {
        editor.handle_key(Key::Char(ch));
    }
    editor.handle_key(Key::Enter);
    let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
    terminal
        .draw(|f| ui::render(f, &mut editor, &theme))
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(3, 0)].bg, theme.search);
    editor.handle_key(Key::Char('V'));
    editor.handle_key(Key::Char('/'));
    assert!(editor.selection().unwrap().linewise);
    for ch in "a very long query 界".chars() {
        editor.handle_key(Key::Char(ch));
    }
    terminal.backend_mut().resize(8, 3);
    terminal
        .draw(|f| ui::render(f, &mut editor, &theme))
        .unwrap();
    let cursor = terminal.get_cursor_position().unwrap();
    assert!(cursor.x < 8 && cursor.y == 2);
}

#[test]
fn buffer_list_renders_dirty_inactive_documents_in_tiny_terminals() {
    use editor_pro::input::Key;
    let mut editor = Editor::new(Document::default());
    editor.document.buffer.insert("dirty");
    editor.new_buffer();
    editor.execute_command("ls").unwrap();
    editor.handle_key(Key::Up);
    for (width, height) in [(1, 1), (1, 3), (40, 6)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| ui::render(f, &mut editor, &Theme::night()))
            .unwrap();
        if width == 40 {
            let output: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(output.contains("[No Name]"));
            assert!(output.contains('+'));
            assert!(output.contains("Buffers:"));
        }
    }
}

#[test]
fn file_picker_prompt_renders_safely_in_tiny_terminals() {
    use editor_pro::input::Key;
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("界 file.rs"), "").unwrap();
    let mut editor = Editor::new(Document::default());
    editor.open_picker(directory.path()).unwrap();
    for c in "long query 界e\u{301}".chars() {
        editor.handle_key(Key::Char(c));
    }
    for (width, height) in [(1, 1), (1, 3), (2, 4), (40, 10)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| ui::render(f, &mut editor, &Theme::night()))
            .unwrap();
        if height > 2 {
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < width && cursor.y < height - 2);
        }
    }
}
