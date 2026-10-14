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
