use editor_pro::{editor::Editor, file::Document, input::Key, theme::ThemeName, ui};
use ratatui::{Terminal, backend::TestBackend};
use std::fs;
#[test]
fn theme_commands_change_the_full_palette_without_changing_edit_history() {
    let mut editor = Editor::new(Document::default());
    editor.document.buffer.insert("keep");
    editor.document.buffer.commit();
    let mut backgrounds = Vec::new();
    for name in ["tokyo-night", "catppuccin", "gruvbox"] {
        editor.execute_command(&format!("theme {name}")).unwrap();
        assert_eq!(editor.theme.name(), name);
        let theme = editor.theme.palette();
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &mut editor, &theme))
            .unwrap();
        backgrounds.push(terminal.backend().buffer()[(30, 1)].bg);
        assert_eq!(terminal.backend().buffer()[(0, 4)].bg, theme.normal);
        assert_eq!(editor.document.buffer.text().to_string(), "keep");
    }
    assert_ne!(backgrounds[0], backgrounds[1]);
    assert_ne!(backgrounds[1], backgrounds[2]);
    assert_ne!(backgrounds[0], backgrounds[2]);
    assert!(editor.execute_command("theme missing").is_err());
    assert_eq!(editor.theme, ThemeName::Gruvbox);
    editor.document.buffer.undo();
    assert_eq!(editor.document.buffer.text().to_string(), "");
}
#[test]
fn each_theme_colors_syntax_search_and_selection_and_reports_buffer_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("example.rs");
    fs::write(&path, "fn main() {}\r\n// comment\r\n").unwrap();
    let mut editor = Editor::new(Document::open(&path).unwrap());
    editor.new_buffer();
    editor.document.buffer.insert("dirty");
    editor.switch_buffer(1).unwrap();
    editor.search.set("fn", false);
    for name in [
        ThemeName::TokyoNight,
        ThemeName::Catppuccin,
        ThemeName::Gruvbox,
    ] {
        let theme = name.palette();
        let mut terminal = Terminal::new(TestBackend::new(120, 7)).unwrap();
        terminal
            .draw(|f| ui::render(f, &mut editor, &theme))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(3, 0)].fg, theme.keyword);
        assert_eq!(terminal.backend().buffer()[(3, 0)].bg, theme.search);
        assert_eq!(terminal.backend().buffer()[(3, 1)].fg, theme.comment);
        let output: String = (0..120)
            .map(|x| terminal.backend().buffer()[(x, 5)].symbol())
            .collect();
        for word in [
            "NORMAL", "[B1]", "2buf", "1dirty", "Rust", "UTF-8", "CRLF", "1:1",
        ] {
            assert!(output.contains(word), "{output}");
        }
        editor.handle_key(Key::Char('v'));
        terminal
            .draw(|f| ui::render(f, &mut editor, &theme))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(3, 0)].bg, theme.selection);
        editor.handle_key(Key::Escape);
    }
}
#[test]
fn status_shows_pending_operators_and_plain_fallback_without_panicking_when_narrow() {
    let mut editor = Editor::new(Document::default());
    editor.document.syntax.fallback = Some("test fallback".into());
    for c in "2d3".chars() {
        editor.handle_key(Key::Char(c));
    }
    for width in [1, 8, 20, 40, 80, 120] {
        let mut terminal = Terminal::new(TestBackend::new(width, 4)).unwrap();
        terminal
            .draw(|f| ui::render(f, &mut editor, &ThemeName::Gruvbox.palette()))
            .unwrap();
        if width >= 80 {
            let status: String = (0..width)
                .map(|x| terminal.backend().buffer()[(x, 2)].symbol())
                .collect();
            assert!(status.contains("2d3"));
            assert!(status.contains("[plain]"));
        }
    }
}
