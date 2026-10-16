use editor_pro::{
    buffer::Buffer,
    editor::{Editor, Mode, View},
    file::Document,
    input::{self, Key},
    theme::Theme,
    ui,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
};

fn editor() -> Editor {
    let mut document = Document::default();
    document.buffer = Buffer::new(&"e\u{301}界👩‍💻\t source\r\n".repeat(50));
    Editor::new(document)
}

fn keys(editor: &mut Editor, text: &str) {
    for ch in text.chars() {
        editor.handle_key(Key::Char(ch));
    }
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
        .chunks(usize::from(width).max(1))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn pages(editor: &mut Editor, width: u16, height: u16) -> String {
    let mut output = String::new();
    for _ in 0..1000 {
        output.push_str(&draw(editor, width, height));
        let before = editor.help.as_ref().unwrap().scroll();
        editor.handle_key(Key::PageDown);
        if editor.help.as_ref().unwrap().scroll() == before {
            return output;
        }
    }
    panic!("Help did not reach the last page");
}

#[test]
fn help_discovers_definition_git_review_and_codex_with_actual_bindings() {
    let mut editor = editor();
    for (query, expected) in [
        (
            "definition",
            vec!["gd", ":Definition", "ready language server"],
        ),
        ("references", vec!["gr", ":References"]),
        (
            "git",
            vec![
                "Space g d",
                "Space g c",
                "]c / [c",
                "]f / [f",
                ":diff staged",
                "Diff",
            ],
        ),
        ("diagnostics", vec![":Diagnostics / :Dnext / :Dprev"]),
        (
            "review",
            vec![
                "F / R / A",
                ":note file|hunk|old|new",
                ":note save",
                ":note cancel",
            ],
        ),
        (
            "CODEX",
            vec![
                ":ai context file",
                ":ai ask QUESTION",
                ":ai send",
                "Ctrl-g",
                "authenticated",
                "Codex CLI",
            ],
        ),
        ("visual", vec!["v / V", "Visual", "Ctrl-g"]),
    ] {
        keys(&mut editor, &format!(":help {query}"));
        editor.handle_key(Key::Enter);
        let output = pages(&mut editor, 100, 24);
        for binding in expected {
            assert!(
                output.contains(binding),
                "Missing {binding} for {query}: {output}"
            );
        }
        editor.handle_key(Key::Escape);
        assert_eq!(editor.mode, Mode::Normal);
    }
    for (command, expected) in [
        ("ai help", ":ai ask QUESTION"),
        ("review help", ":note save"),
    ] {
        editor.execute_command(command).unwrap();
        assert!(pages(&mut editor, 100, 24).contains(expected));
        editor.handle_key(Key::Escape);
    }
}

#[test]
fn help_search_categories_and_scrolling_are_keyboard_accessible() {
    let mut editor = editor();
    editor.handle_key(Key::F1);
    assert!(draw(&mut editor, 80, 8).contains("Help | All"));
    editor.handle_key(Key::PageDown);
    assert!(editor.help.as_ref().unwrap().scroll() > 0);
    editor.handle_key(Key::Home);
    assert_eq!(editor.help.as_ref().unwrap().scroll(), 0);
    editor.handle_key(Key::End);
    assert!(draw(&mut editor, 80, 8).contains("load!/new!"));
    editor.handle_key(Key::PageUp);
    let scroll = editor.help.as_ref().unwrap().scroll();
    editor.handle_key(Key::Up);
    assert_eq!(editor.help.as_ref().unwrap().scroll(), scroll - 1);
    editor.handle_key(Key::Ctrl('n'));
    assert_eq!(editor.help.as_ref().unwrap().scroll(), scroll);
    editor.handle_key(Key::BackTab);
    assert_eq!(editor.help.as_ref().unwrap().category(), "AI / Codex");
    keys(&mut editor, "aSk quESTion");
    let output = draw(&mut editor, 100, 24);
    assert!(output.contains(":ai ask QUESTION"));
    assert!(!output.contains("Space g d"));
    editor.handle_key(Key::Tab);
    assert_eq!(editor.help.as_ref().unwrap().category(), "All");
    assert_eq!(editor.help.as_ref().unwrap().query(), "aSk quESTion");
    keys(&mut editor, "not-a-command");
    assert!(draw(&mut editor, 40, 8).contains("No shortcuts match"));
    editor.handle_key(Key::Ctrl('u'));
    assert_eq!(editor.help.as_ref().unwrap().query(), "");
    keys(&mut editor, "界e\u{301}👩‍💻");
    for expected in ["界e\u{301}", "界", ""] {
        editor.handle_key(Key::Backspace);
        assert_eq!(editor.help.as_ref().unwrap().query(), expected);
    }
    keys(&mut editor, &"界".repeat(300));
    assert!(editor.help.as_ref().unwrap().query().len() <= 256);
    editor.handle_key(Key::Ctrl('['));
    assert!(editor.help.is_none());
}

#[test]
fn help_preserves_modes_pending_input_selection_search_and_viewport() {
    for input in [
        "",
        "iunsaved",
        "vll",
        "Vj",
        ":w somewhere",
        "/source",
        "2d",
        "g",
        " g",
        "f",
    ] {
        let mut editor = editor();
        editor.document.buffer.goto_line(30, false);
        editor.search.set("source", true);
        editor.search.refresh(&editor.document.buffer);
        editor.register.text = "register\r\n".into();
        editor.register.linewise = true;
        keys(&mut editor, input);
        editor.viewport.top = 20;
        editor.viewport.left = 3;
        editor.viewport.height = 18;
        let mode = editor.mode;
        let cursor = editor.document.buffer.cursor();
        let text = editor.document.buffer.text().clone();
        let generation = editor.document.buffer.generation();
        let viewport = editor.viewport.clone();
        let pending = editor.pending_label();
        let selection = editor.selection_anchor;
        let command = editor.command.clone();
        let matches = editor.search.matches.clone();
        let message = editor.message.clone();
        editor.handle_key(Key::F1);
        keys(&mut editor, ":q! :w :ai send dd i");
        editor.handle_key(Key::Enter);
        editor.handle_key(Key::Delete);
        editor.handle_key(Key::Ctrl('r'));
        editor.paste("\x1b:w\r\nPASTED");
        assert!(!editor.tick());
        assert!(editor.execute_command("q!").is_err());
        for (width, height) in [(80, 24), (20, 7), (1, 1), (1, 3), (2, 2), (0, 0), (120, 30)] {
            draw(&mut editor, width, height);
            editor.handle_key(Key::PageDown);
            assert_eq!(editor.viewport, viewport);
            assert_eq!(editor.document.buffer.cursor(), cursor);
        }
        editor.handle_key(Key::Escape);
        assert!(editor.help.is_none());
        assert_eq!(editor.view, View::Editing);
        assert_eq!(editor.mode, mode, "{input}");
        assert_eq!(editor.document.buffer.text(), &text);
        assert_eq!(editor.document.buffer.generation(), generation);
        assert_eq!(editor.document.buffer.cursor(), cursor);
        assert_eq!(editor.selection_anchor, selection);
        assert_eq!(editor.pending_label(), pending);
        assert_eq!(editor.command, command);
        assert_eq!(editor.search.query, "source");
        assert_eq!(editor.search.matches, matches);
        assert!(editor.search.backward && editor.search.visible);
        assert_eq!(editor.register.text, "register\r\n");
        assert!(editor.register.linewise);
        assert_eq!(editor.message, message);
        assert!(!editor.quit);
        if input == "2d" {
            editor.handle_key(Key::Char('d'));
            assert!(editor.document.buffer.text().len_chars() < text.len_chars());
            editor.handle_key(Key::Char('u'));
            assert_eq!(editor.document.buffer.text(), &text);
        }
    }
}

#[test]
fn help_does_not_split_insert_undo_groups_or_accept_pastes() {
    let mut editor = editor();
    let original = editor.document.buffer.text().clone();
    keys(&mut editor, "ie\u{301}");
    editor.handle_key(Key::F1);
    editor.paste("SHOULD NOT INSERT\r\n");
    editor.handle_key(Key::F1);
    assert_eq!(editor.mode, Mode::Insert);
    keys(&mut editor, "界👩‍💻");
    editor.handle_key(Key::Escape);
    let edited = editor.document.buffer.text().clone();
    editor.handle_key(Key::Char('u'));
    assert_eq!(editor.document.buffer.text(), &original);
    editor.handle_key(Key::Ctrl('r'));
    assert_eq!(editor.document.buffer.text(), &edited);
}

#[test]
fn narrow_help_wraps_long_commands_and_keeps_every_line_reachable() {
    let mut editor = editor();
    editor.execute_command("help LspInstallCancel").unwrap();
    let output = pages(&mut editor, 18, 7);
    assert!(output.contains(":LspInstallCancel"));
    assert!(output.contains("installation"));
    assert!(output.contains("Esc back"));
    editor.execute_command("help gd").unwrap();
    let output = pages(&mut editor, 1, 1);
    assert!(
        output.contains("gd"),
        "one-column rows remain scrollable: {output}"
    );
    editor.execute_command("help 界👩‍💻é").unwrap();
    for (width, height) in [(1, 4), (4, 4), (18, 7), (80, 24)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &mut editor, &Theme::night()))
            .unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        assert!(cursor.x < width && cursor.y < height);
    }
    editor.handle_key(Key::Escape);
    assert!(editor.help.is_none());
}

#[test]
fn f1_and_category_keys_decode_without_intercepting_modified_or_released_input() {
    assert_eq!(
        input::decode(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
        Some(Key::F1)
    );
    assert_eq!(
        input::decode(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        Some(Key::BackTab)
    );
    assert_eq!(
        input::decode(KeyEvent::new(KeyCode::F(1), KeyModifiers::ALT)),
        None
    );
    assert_eq!(
        input::decode(KeyEvent::new(KeyCode::F(1), KeyModifiers::CONTROL)),
        None
    );
    let mut release = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    assert_eq!(input::decode(release), None);
}

#[test]
fn help_returns_to_buffer_and_file_pickers_with_their_selections_and_prompts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source.txt"), "on disk\r\n").unwrap();
    let mut editor = editor();
    editor.new_buffer();
    editor.execute_command("ls").unwrap();
    editor.handle_key(Key::Up);
    let active = editor.active_buffer_id;
    editor.handle_key(Key::F1);
    editor.handle_key(Key::Down);
    editor.handle_key(Key::Enter);
    editor.handle_key(Key::Escape);
    assert_eq!(editor.view, View::Buffers { selected: 0 });
    assert_eq!(editor.active_buffer_id, active);
    editor.handle_key(Key::Escape);
    editor.open_picker(dir.path()).unwrap();
    keys(&mut editor, "source");
    editor.handle_key(Key::F1);
    keys(&mut editor, "overwrite");
    editor.handle_key(Key::Enter);
    draw(&mut editor, 10, 5);
    editor.handle_key(Key::Escape);
    assert_eq!(editor.view, View::Files);
    assert_eq!(editor.picker.as_ref().unwrap().query, "source");
    assert_eq!(editor.active_buffer_id, active);
    editor.handle_key(Key::Escape);
    editor.toggle_tree(Some(dir.path())).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while editor.tree.entries.is_empty() {
        assert!(std::time::Instant::now() < deadline);
        editor.tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    editor.handle_key(Key::Char('a'));
    keys(&mut editor, "draft");
    editor.handle_key(Key::F1);
    keys(&mut editor, "tree");
    editor.handle_key(Key::Enter);
    editor.handle_key(Key::Escape);
    assert_eq!(editor.view, View::Tree);
    keys(&mut editor, ".txt");
    editor.handle_key(Key::Enter);
    while !dir.path().join("draft.txt").exists() {
        assert!(std::time::Instant::now() < deadline);
        editor.tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        std::fs::read(dir.path().join("source.txt")).unwrap(),
        b"on disk\r\n"
    );
}
