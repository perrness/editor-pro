use editor_pro::{
    editor::{Editor, Mode, View},
    file::Document,
    input::Key,
    lsp::{self, Language, Phase, Stamp, Versioned},
    lsp_worker::read_frame,
    theme::Theme,
    ui,
};
use lsp_types::{Position, Range};
use ratatui::{Terminal, backend::TestBackend};
use ropey::Rope;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    directory: tempfile::TempDir,
    editor: Editor,
}
impl Fixture {
    fn new(scenario: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("pom.xml"), "<project/>").unwrap();
        fs::write(
            directory.path().join("Main.kt"),
            "bad value\r\n😀 e\u{301}界\r\n",
        )
        .unwrap();
        fs::write(directory.path().join("Other.kt"), "bad other\n").unwrap();
        let mut editor = Editor::new(Document::open(&directory.path().join("Main.kt")).unwrap());
        editor.lsp.config.commands.insert(
            Language::Kotlin,
            vec![
                "python3".into(),
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tools/lsp/fake_server.py")
                    .into_os_string(),
                "--log".into(),
                directory.path().join("protocol.jsonl").into_os_string(),
                "--scenario".into(),
                scenario.into(),
            ],
        );
        editor.lsp.config.initialize_timeout = Duration::from_secs(2);
        editor.lsp.config.request_timeout = Duration::from_millis(800);
        editor.lsp.config.restart_limit = 0;
        Self { directory, editor }
    }
    fn ready(&mut self) {
        until(&mut self.editor, |e| {
            e.lsp.servers.values().any(|s| s.capabilities.is_object())
        });
    }
    fn command(&mut self, command: &str) {
        self.editor.execute_command(command).unwrap();
        until(&mut self.editor, |e| !e.lsp.pending);
        assert!(!self.editor.message_is_error, "{}", self.editor.message);
    }
    fn notify(&self, frame: Value) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.directory.path().join("protocol.jsonl.notifications"))
            .unwrap();
        writeln!(file, "{frame}").unwrap();
    }
    fn progress(&self, token: Value, value: Value) {
        self.notify(json!({"method":"$/progress","params":{"token":token,"value":value}}));
    }
    fn log(&self) -> Vec<Value> {
        fs::read_to_string(self.directory.path().join("protocol.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
    fn wait_log(&mut self, method: &str, count: usize) {
        let start = Instant::now();
        loop {
            self.editor.tick();
            if self.log().iter().filter(|m| m["method"] == method).count() >= count {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(6),
                "Missing {method}: {:?}",
                self.log()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
fn until(editor: &mut Editor, condition: impl Fn(&Editor) -> bool) {
    let start = Instant::now();
    loop {
        editor.tick();
        if condition(editor) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(6),
            "timed out: {}; {:?}",
            editor.message,
            editor
                .lsp
                .servers
                .values()
                .map(|s| &s.status)
                .collect::<Vec<_>>()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn settle(editor: &mut Editor) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(350) {
        editor.tick();
        thread::sleep(Duration::from_millis(5));
    }
}
fn escape(editor: &mut Editor) {
    editor.handle_key(Key::Escape);
}
fn text_edit(start: u32, end: u32, text: &str) -> Value {
    json!({"range":Range::new(Position::new(0,start),Position::new(0,end)),"newText":text})
}
fn workspace(path: &Path, edits: Vec<Value>) -> Value {
    json!({"changes":{lsp::file_uri(path).unwrap().as_str():edits}})
}

fn current_versions(editor: &Editor) -> Vec<Versioned> {
    let path = editor.document.path().unwrap().to_path_buf();
    vec![Versioned {
        snapshot: lsp::Snapshot {
            stamp: editor.lsp_edit_guards()[&path].clone(),
            path: path.clone(),
            text: editor.document.buffer.text().clone(),
            saved: editor.document.save_generation(),
            language: Language::for_path(&path).unwrap(),
            virtual_source: None,
        },
        version: 1,
    }]
}

fn csharp_metadata_fixture(scenario: &str) -> Fixture {
    let mut f = Fixture::new(scenario);
    let command = f
        .editor
        .lsp
        .config
        .commands
        .remove(&Language::Kotlin)
        .unwrap();
    let path = f.directory.path().join("Main.cs");
    fs::write(&path, "class Main { Library value; }\r\n").unwrap();
    fs::write(f.directory.path().join("Probe.csproj"), "<Project/>").unwrap();
    let config = f.editor.lsp.config.clone();
    f.editor = Editor::new(Document::open(&path).unwrap());
    f.editor.lsp.config = config;
    f.editor
        .lsp
        .config
        .commands
        .insert(Language::CSharp, command);
    if scenario == "metadata-large" {
        f.editor.lsp.config.request_timeout = Duration::from_secs(5);
    }
    f.ready();
    f
}

fn gd(editor: &mut Editor) {
    editor.handle_key(Key::Char('g'));
    editor.handle_key(Key::Char('d'));
    until(editor, |e| !e.lsp.pending);
}

#[test]
fn csharp_generated_and_decompiled_definitions_are_read_only_and_keep_history() {
    for scenario in ["metadata-normal", "metadata-generated", "metadata-link"] {
        let mut f = csharp_metadata_fixture(scenario);
        let main = f.editor.document.path().unwrap().to_path_buf();
        f.editor.document.buffer.insert("// unsaved\r\n");
        f.editor.document.buffer.commit();
        let original = f.editor.document.buffer.text().to_string();
        let cursor = f.editor.document.buffer.cursor();
        gd(&mut f.editor);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert!(f.editor.document.path().is_none());
        let uri = f.editor.document.virtual_uri().unwrap().to_owned();
        assert!(uri.starts_with("csharp:"));
        assert_eq!(f.editor.document.buffer.position(), (0, 12));
        assert_eq!(f.editor.document.buffer.newline(), "\r\n");
        let source = f.editor.document.buffer.text().to_string();
        assert!(f.editor.document.buffer.is_read_only());
        assert!(f.editor.document.display_name().contains("Read only"));
        for key in ['i', 'a', 'o', 'x', 'd', 'c', 'p', 'u'] {
            f.editor.handle_key(Key::Char(key));
            assert!(f.editor.message.contains("read only"));
            assert_eq!(f.editor.mode, Mode::Normal);
        }
        // Core mutation entry points cannot bypass the document's protection.
        f.editor.document.buffer.insert("bad");
        f.editor.document.buffer.replace_range(0..1, "bad");
        f.editor.document.buffer.delete(true);
        f.editor.document.buffer.backspace();
        f.editor.document.buffer.open_line(false);
        assert!(!f.editor.document.buffer.undo());
        assert!(!f.editor.document.buffer.redo());
        assert_eq!(f.editor.document.buffer.text().to_string(), source);
        assert!(!f.editor.document.buffer.is_modified());
        for command in ["w", "Format", "Rename renamed", "CodeAction", "Complete"] {
            assert!(
                f.editor
                    .execute_command(command)
                    .unwrap_err()
                    .to_string()
                    .contains("read only")
            );
        }
        let destination = f.directory.path().join("must-not-save.cs");
        assert!(f.editor.document.save(Some(&destination)).is_err());
        assert!(!destination.exists());
        assert_eq!(f.editor.navigation.recent, vec![main.clone()]);
        f.editor.handle_key(Key::Ctrl('o'));
        assert_eq!(f.editor.document.path(), Some(main.as_path()));
        assert_eq!(f.editor.document.buffer.text().to_string(), original);
        assert_eq!(f.editor.document.buffer.cursor(), cursor);
        f.editor.handle_key(Key::Ctrl('i'));
        assert_eq!(f.editor.document.virtual_uri(), Some(uri.as_str()));
        f.editor.handle_key(Key::Ctrl('o'));
        gd(&mut f.editor);
        assert_eq!(f.editor.buffer_count(), 2);
        assert_eq!(f.editor.document.virtual_uri(), Some(uri.as_str()));
        assert_eq!(
            fs::read_to_string(&main).unwrap(),
            "class Main { Library value; }\r\n"
        );
        settle(&mut f.editor);
        assert!(
            f.log()
                .iter()
                .filter(|m| matches!(
                    m["method"].as_str(),
                    Some(
                        "textDocument/didOpen"
                            | "textDocument/didChange"
                            | "textDocument/didSave"
                            | "textDocument/didClose"
                    )
                ))
                .all(|m| m["params"]["textDocument"]["uri"]
                    .as_str()
                    .unwrap()
                    .starts_with("file:"))
        );
    }
}

#[test]
fn csharp_navigation_from_virtual_source_uses_original_uri_and_survives_source_close() {
    let mut f = csharp_metadata_fixture("metadata-normal");
    gd(&mut f.editor);
    let decompiled = f.editor.document.virtual_uri().unwrap().to_owned();
    gd(&mut f.editor);
    assert!(
        f.editor
            .document
            .virtual_uri()
            .unwrap()
            .contains("/generated/Generated%20Thing.g.cs")
    );
    let generated = f.editor.document.virtual_uri().unwrap().to_owned();
    gd(&mut f.editor);
    assert_eq!(
        f.editor.document.path().unwrap().file_name().unwrap(),
        "Main.cs"
    );
    f.editor.close_buffer(false).unwrap();
    f.editor.tick();
    assert!(f.editor.document.virtual_uri().is_some());
    gd(&mut f.editor);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    let uris: Vec<_> = f
        .log()
        .into_iter()
        .filter(|m| m["method"] == "textDocument/definition")
        .map(|m| {
            m["params"]["textDocument"]["uri"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(uris[1], decompiled);
    assert_eq!(uris[2], generated);
}

#[test]
fn csharp_virtual_results_work_in_explicit_and_multiple_location_lists() {
    for (scenario, command) in [
        ("metadata-normal", "Definition"),
        ("metadata-generated", "References"),
        ("metadata-multiple", "Definition"),
    ] {
        let mut f = csharp_metadata_fixture(scenario);
        f.command(command);
        assert_eq!(f.editor.view, View::Lsp);
        let panel = f.editor.lsp.panel.as_mut().unwrap();
        panel.selected = panel.rows.len() - 1;
        f.editor.handle_key(Key::Enter);
        until(&mut f.editor, |e| !e.lsp.pending);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert!(f.editor.document.virtual_uri().is_some());
        assert_eq!(f.editor.view, View::Editing);
    }
}

#[test]
fn csharp_metadata_failures_preserve_source_and_navigation() {
    for (scenario, message) in [
        ("metadata-null", "no source text"),
        ("metadata-nul", "NUL"),
        ("metadata-large", "2 MiB"),
        ("metadata-position", "outside"),
        ("metadata-error", "Cannot decompile"),
        ("metadata-timeout", "timed out"),
    ] {
        let mut f = csharp_metadata_fixture(scenario);
        let main = f.editor.document.path().unwrap().to_path_buf();
        let original = f.editor.document.buffer.text().to_string();
        gd(&mut f.editor);
        assert!(
            f.editor.message_is_error,
            "{scenario}: {}",
            f.editor.message
        );
        assert!(
            f.editor.message.contains(message),
            "{scenario}: {}",
            f.editor.message
        );
        assert_eq!(f.editor.document.path(), Some(main.as_path()));
        assert_eq!(f.editor.document.buffer.text().to_string(), original);
        assert_eq!(f.editor.buffer_count(), 1);
        assert!(f.editor.navigate_history(true).is_err());
    }
}

#[test]
fn late_csharp_metadata_cannot_navigate_after_context_changes() {
    for action in ["cancel", "move", "edit", "switch", "restart"] {
        let mut f = csharp_metadata_fixture("metadata-slow");
        f.editor.handle_key(Key::Char('g'));
        f.editor.handle_key(Key::Char('d'));
        f.wait_log("csharp/metadata", 1);
        match action {
            "cancel" => f.editor.handle_key(Key::Escape),
            "move" => f.editor.handle_key(Key::Char('l')),
            "edit" => f.editor.document.buffer.insert("edit"),
            "switch" => f.editor.new_buffer(),
            "restart" => f.editor.execute_command("LspRestart").unwrap(),
            _ => unreachable!(),
        }
        settle(&mut f.editor);
        assert!(f.editor.document.virtual_uri().is_none(), "{action}");
        assert!(
            f.editor
                .buffer_infos()
                .iter()
                .all(|b| !b.name.contains("csharp:"))
        );
    }
}

#[test]
fn csharp_virtual_source_refreshes_in_place_and_rejects_old_server_sessions() {
    let mut f = csharp_metadata_fixture("metadata-generated");
    gd(&mut f.editor);
    let id = f.editor.active_buffer_id;
    let old_instance = f.editor.document.buffer.instance();
    f.editor.handle_key(Key::Ctrl('o'));
    fs::write(
        f.directory.path().join("protocol.jsonl.source"),
        "/*😀*/ class Updated {}\r\n",
    )
    .unwrap();
    gd(&mut f.editor);
    assert_eq!(f.editor.active_buffer_id, id);
    assert_eq!(f.editor.buffer_count(), 2);
    assert_ne!(f.editor.document.buffer.instance(), old_instance);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .contains("class Updated")
    );
    let epoch = f.editor.lsp.servers.values().next().unwrap().epoch;
    f.editor.execute_command("LspRestart").unwrap();
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.epoch != epoch && s.capabilities.is_object())
    });
    gd(&mut f.editor);
    assert!(
        f.editor.message.contains("previous server session"),
        "{}",
        f.editor.message
    );
    assert_eq!(f.editor.active_buffer_id, id);
    f.editor.handle_key(Key::Ctrl('o'));
    gd(&mut f.editor);
    assert!(f.editor.document.virtual_uri().is_some());
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    assert_ne!(f.editor.active_buffer_id, id);
}

#[test]
fn help_defers_csharp_metadata_and_virtual_source_displays_read_only_status() {
    let mut f = csharp_metadata_fixture("metadata-slow");
    f.editor.handle_key(Key::Char('g'));
    f.editor.handle_key(Key::Char('d'));
    f.wait_log("csharp/metadata", 1);
    f.editor.handle_key(Key::F1);
    settle(&mut f.editor);
    assert!(f.editor.document.virtual_uri().is_none());
    assert!(f.editor.lsp.pending);
    f.editor.handle_key(Key::Escape);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(f.editor.document.virtual_uri().is_some());
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("[RO]"), "{screen}");
    for (w, h) in [(1, 1), (2, 2), (10, 3)] {
        terminal.backend_mut().resize(w, h);
        terminal
            .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
            .unwrap();
    }
}

#[test]
fn csharp_virtual_buffers_do_not_block_source_diagnostics_after_restart() {
    let mut f = csharp_metadata_fixture("metadata-pull");
    f.editor.document.buffer.insert("bad ");
    until(&mut f.editor, |e| {
        e.current_diagnostics().is_some_and(|d| !d.items.is_empty())
    });
    gd(&mut f.editor);
    assert!(f.editor.document.virtual_uri().is_some());
    f.editor.handle_key(Key::Ctrl('o'));
    until(&mut f.editor, |e| {
        e.current_diagnostics().is_some_and(|d| !d.items.is_empty())
    });
    let epoch = f.editor.lsp.servers.values().next().unwrap().epoch;
    f.editor.execute_command("LspRestart").unwrap();
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.epoch != epoch && s.capabilities.is_object())
            && e.current_diagnostics().is_some_and(|d| !d.items.is_empty())
    });
    assert_eq!(f.editor.buffer_count(), 2);
}

#[test]
fn server_cannot_edit_an_unsynchronized_open_dirty_buffer() {
    let mut f = Fixture::new("normal");
    let versions = current_versions(&f.editor);
    let other = f.directory.path().join("Other.java");
    fs::write(&other, "bad value\n").unwrap();
    f.editor.open_buffer(&other).unwrap();
    f.editor.document.buffer.insert("unsaved ");
    let value = workspace(&other, vec![text_edit(0, 3, "wrong")]);
    let error = f
        .editor
        .prepare_lsp_edit(
            &value,
            f.directory.path(),
            &versions,
            &f.editor.lsp_edit_guards(),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("did not synchronize"));
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("unsaved bad")
    );
}

#[test]
fn ai_context_uses_selected_symbol_ranges_references_and_labelled_diagnostics() {
    let mut f = Fixture::new("normal");
    f.ready();
    until(&mut f.editor, |e| e.current_diagnostics().is_some());
    f.command("Outline");
    let before = f.editor.document.buffer.text().to_string();
    let cursor = f.editor.document.buffer.cursor();
    f.editor.execute_command("ai context symbol").unwrap();
    assert_eq!(f.editor.ai.draft.context[0].text, "bad");
    f.editor.execute_command("ai add diagnostics").unwrap();
    assert!(
        f.editor.ai.draft.context[1]
            .label
            .contains("version matched")
    );
    assert!(!f.editor.ai.draft.context[1].text.is_empty());
    assert!(!f.editor.ai.pending);
    escape(&mut f.editor);
    assert_eq!(f.editor.view, View::Editing);
    f.command("References");
    f.editor.execute_command("ai add references").unwrap();
    let reference = f.editor.ai.draft.context.last().unwrap();
    assert_eq!(reference.text, "bad other\n");
    assert!(
        reference
            .source
            .as_ref()
            .unwrap()
            .path
            .ends_with("Other.kt")
    );
    assert_eq!(f.editor.document.buffer.text().to_string(), before);
    assert_eq!(f.editor.document.buffer.cursor(), cursor);
    escape(&mut f.editor);
    f.command("Outline");
    f.editor.document.buffer.insert("changed");
    assert!(f.editor.execute_command("ai context symbol").is_err());
}

#[test]
fn explain_shortcut_selects_containing_symbol_and_outline_selection_without_sending() {
    let mut f = Fixture::new("ai-symbols");
    f.ready();
    f.editor.document.buffer.insert("local ");
    f.editor.document.buffer.commit();
    f.editor.document.buffer.undo();
    f.editor.document.buffer.set_cursor(5, false);
    f.editor.viewport.left = 2;
    let before = f.editor.document.buffer.text().to_string();
    let cursor = f.editor.document.buffer.cursor();
    let viewport = f.editor.viewport.clone();
    f.editor.ai.config.executable = "/missing/codex".into();
    f.editor.handle_key(Key::Ctrl('g'));
    until(&mut f.editor, |e| !e.lsp.pending);
    assert_eq!(f.editor.view, View::Ai, "{}", f.editor.message);
    assert_eq!(f.editor.ai.draft.context[0].text, "value");
    assert_eq!(
        f.editor.ai.draft.context[0].source.as_ref().unwrap().start,
        4
    );
    assert_eq!(f.editor.ai.draft.question, editor_pro::ai::EXPLAIN_SOURCE);
    assert!(!f.editor.ai.pending);
    escape(&mut f.editor);
    assert_eq!(f.editor.view, View::Editing);
    assert_eq!(f.editor.document.buffer.cursor(), cursor);
    assert_eq!(f.editor.viewport, viewport);
    f.command("Outline");
    f.editor.lsp.panel.as_mut().unwrap().selected = 0;
    f.editor.handle_key(Key::Ctrl('g'));
    assert_eq!(f.editor.ai.draft.context[0].text, "bad value");
    assert_eq!(f.editor.document.buffer.text().to_string(), before);
    escape(&mut f.editor);
    f.editor
        .execute_command("ai explain What does this symbol return?")
        .unwrap();
    until(&mut f.editor, |e| !e.lsp.pending);
    assert_eq!(f.editor.ai.draft.question, "What does this symbol return?");
    assert_eq!(f.editor.ai.draft.context[0].text, "value");
    assert!(!f.editor.ai.pending);
}

#[test]
fn explain_symbol_drops_late_results_after_cancel_move_edit_or_buffer_switch() {
    for change in ["cancel", "move", "edit", "switch"] {
        let mut f = Fixture::new("comprehension-slow");
        f.ready();
        f.editor.handle_key(Key::Ctrl('g'));
        assert!(f.editor.lsp.pending);
        match change {
            "cancel" => f.editor.handle_key(Key::Escape),
            "move" => f.editor.handle_key(Key::Char('l')),
            "edit" => f.editor.document.buffer.insert("changed"),
            "switch" => f
                .editor
                .open_buffer(&f.directory.path().join("Other.kt"))
                .unwrap(),
            _ => unreachable!(),
        }
        let text = f.editor.document.buffer.text().to_string();
        let cursor = f.editor.document.buffer.cursor();
        settle(&mut f.editor);
        assert_eq!(f.editor.view, View::Editing, "{change}");
        assert!(!f.editor.lsp.pending);
        assert!(!f.editor.ai.pending);
        assert!(f.editor.ai.draft.context.is_empty());
        assert_eq!(f.editor.document.buffer.text().to_string(), text);
        assert_eq!(f.editor.document.buffer.cursor(), cursor);
    }
}

#[test]
fn explain_symbol_refuses_unrelated_cursor_and_stale_outline() {
    let mut f = Fixture::new("ai-symbols");
    f.ready();
    f.editor.document.buffer.goto_line(1, false);
    let cursor = f.editor.document.buffer.cursor();
    f.editor.handle_key(Key::Ctrl('g'));
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(f.editor.message.contains("No symbol contains the cursor"));
    assert_eq!(f.editor.view, View::Editing);
    assert_eq!(f.editor.document.buffer.cursor(), cursor);
    f.command("Outline");
    f.editor.document.buffer.insert("changed");
    f.editor.handle_key(Key::Ctrl('g'));
    assert!(f.editor.message.contains("stale"));
    assert_eq!(f.editor.view, View::Lsp);
    assert!(!f.editor.ai.pending);
    assert!(f.editor.ai.draft.context.is_empty());
}

#[test]
fn utf16_roundtrip_and_incremental_changes_preserve_exact_unicode_and_endings() {
    for text in ["a😀e\u{301}界\r\nnext\n", "a\rb\n", "", "😀", "\r\n"] {
        let rope = Rope::from_str(text);
        for scalar in 0..=rope.len_chars() {
            if let Ok(position) = lsp::position(&rope, scalar) {
                assert_eq!(lsp::scalar(&rope, position).unwrap(), scalar);
            }
        }
    }
    let rope = Rope::from_str("😀\r\n");
    assert!(lsp::scalar(&rope, Position::new(0, 1)).is_err());
    assert!(lsp::scalar(&rope, Position::new(0, 3)).is_err());
    assert!(lsp::scalar(&rope, Position::new(2, 0)).is_err());
    assert!(
        lsp::scalar_range(&rope, Range::new(Position::new(0, 2), Position::new(0, 0))).is_err()
    );
    for before in ["a\r\nx", "a\nx", "😀e\u{301}\n", "a\rb", ""] {
        for after in ["a\r\nx", "ab\r\nx", "a\nx", "😀e\u{301}\r\n", "a\rb", ""] {
            let mut rope = Rope::from_str(before);
            let changed = lsp::change(&rope, &Rope::from_str(after)).unwrap();
            let range = lsp::scalar_range(
                &rope,
                serde_json::from_value(changed["range"].clone()).unwrap(),
            )
            .unwrap();
            rope.remove(range.clone());
            rope.insert(range.start, changed["text"].as_str().unwrap());
            assert_eq!(rope.to_string(), after);
        }
    }
}

#[test]
fn roots_respect_gradle_maven_solution_project_and_repository_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("module/src")).unwrap();
    fs::write(root.join("settings.gradle.kts"), "").unwrap();
    fs::write(root.join("module/build.gradle.kts"), "").unwrap();
    assert_eq!(
        lsp::discover_root(&root.join("module/src/Main.kt"), Language::Kotlin).unwrap(),
        root
    );
    fs::remove_file(root.join("settings.gradle.kts")).unwrap();
    fs::write(root.join("module/pom.xml"), "").unwrap();
    assert_eq!(
        lsp::discover_root(&root.join("module/src/Main.java"), Language::Java).unwrap(),
        root.join("module")
    );
    fs::write(root.join("Project.sln"), "").unwrap();
    fs::write(root.join("module/Project.csproj"), "").unwrap();
    assert_eq!(
        lsp::discover_root(&root.join("module/src/Main.cs"), Language::CSharp).unwrap(),
        root
    );
    fs::remove_file(root.join("Project.sln")).unwrap();
    assert_eq!(
        lsp::discover_root(&root.join("module/src/Main.cs"), Language::CSharp).unwrap(),
        root.join("module")
    );
    fs::write(root.join("a.sln"), "").unwrap();
    fs::write(root.join("b.sln"), "").unwrap();
    assert!(lsp::discover_root(&root.join("module/src/Main.cs"), Language::CSharp).is_err());
}

#[test]
fn uri_roundtrip_escapes_names_and_rejects_remote_or_virtual_locations() {
    let path = Path::new("/tmp/space # % 界.kt");
    let uri = lsp::file_uri(path).unwrap();
    assert_eq!(lsp::uri_path(uri.as_str()).unwrap(), path);
    for uri in [
        "https://example.test/a",
        "jdt://contents/a",
        "file://remote/a",
        "file:///a#fragment",
        "file:///a?q=x",
    ] {
        assert!(lsp::uri_path(uri).is_err(), "{uri}");
    }
}

#[test]
fn framing_handles_fragmentation_and_rejects_bad_or_oversized_messages() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let value = json!({"jsonrpc":"2.0","id":1,"result":"界"});
        let bytes = serde_json::to_vec(&value).unwrap();
        let message = format!("Content-Length: {}\r\n\r\n", bytes.len())
            .into_bytes()
            .into_iter()
            .chain(bytes)
            .collect::<Vec<_>>();
        assert_eq!(read_frame(&mut message.as_slice()).await.unwrap(), value);
        for bytes in [
            b"Content-Length: 999999999\r\n\r\n".as_slice(),
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            b"Content-Length: 2\r\n\r\n{}",
            b"Content-Length: 20\r\n\r\n{}",
            b"wrong\r\n\r\n",
        ] {
            assert!(read_frame(&mut &bytes[..]).await.is_err());
        }
        assert!(read_frame(&mut vec![b'a'; 8193].as_slice()).await.is_err());
    });
}

#[test]
fn sync_open_edit_save_reload_save_as_close_and_full_sync() {
    for scenario in ["normal", "full"] {
        let mut f = Fixture::new(scenario);
        f.ready();
        f.wait_log("textDocument/didOpen", 1);
        f.editor.document.buffer.set_cursor(11, true);
        f.editor.document.buffer.insert("界😀\r\n");
        f.editor.document.buffer.commit();
        let expected = f.editor.document.buffer.text().to_string();
        f.editor.execute_command("w").unwrap();
        f.wait_log("textDocument/didSave", 1);
        let log = f.log();
        let changed = log.iter().rev().find(|v| v["event"] == "content").unwrap();
        assert_eq!(changed["text"], expected);
        assert_eq!(
            log.iter()
                .rev()
                .find(|v| v["method"] == "textDocument/didSave")
                .unwrap()["params"]["text"],
            expected
        );
        f.editor.execute_command("e!").unwrap();
        f.wait_log("textDocument/didOpen", 2);
        f.wait_log("textDocument/didClose", 1);
        let new_path = f.directory.path().join("Renamed.kt");
        f.editor
            .execute_command(&format!("w {}", new_path.display()))
            .unwrap();
        f.wait_log("textDocument/didOpen", 3);
        f.wait_log("textDocument/didClose", 2);
        f.editor.execute_command("bd").unwrap();
        until(&mut f.editor, |e| e.buffer_count() == 1);
        assert!(f.editor.document.path().is_none());
        assert_eq!(fs::read_to_string(new_path).unwrap(), expected);
    }
}

#[test]
fn document_versions_advance_independently_and_do_not_repeat_after_reopening() {
    let mut f = Fixture::new("normal");
    f.ready();
    f.wait_log("textDocument/didOpen", 1);
    let main = f.directory.path().join("Main.kt");
    let main_uri = lsp::file_uri(&main).unwrap();
    let first = f
        .log()
        .into_iter()
        .find(|m| m["method"] == "textDocument/didOpen")
        .unwrap();
    let first_version = first["params"]["textDocument"]["version"].as_i64().unwrap();
    f.editor
        .open_buffer(&f.directory.path().join("Other.kt"))
        .unwrap();
    f.wait_log("textDocument/didOpen", 2);
    f.editor.switch_buffer(1).unwrap();
    f.editor.document.buffer.insert("changed ");
    f.editor.document.buffer.commit();
    f.wait_log("textDocument/didChange", 1);
    let changed = f
        .log()
        .into_iter()
        .find(|m| m["method"] == "textDocument/didChange")
        .unwrap();
    assert_eq!(
        changed["params"]["textDocument"]["version"],
        first_version + 1
    );
    f.editor.close_buffer(true).unwrap();
    f.wait_log("textDocument/didClose", 1);
    f.editor.open_buffer(&main).unwrap();
    f.wait_log("textDocument/didOpen", 3);
    let reopened = f
        .log()
        .into_iter()
        .rev()
        .find(|m| {
            m["method"] == "textDocument/didOpen"
                && m["params"]["textDocument"]["uri"] == main_uri.as_str()
        })
        .unwrap();
    assert!(
        reopened["params"]["textDocument"]["version"]
            .as_i64()
            .unwrap()
            > first_version + 1
    );
}

#[test]
fn diagnostics_reject_old_versions_and_invalidate_immediately_on_edits() {
    let mut f = Fixture::new("normal");
    f.ready();
    until(&mut f.editor, |e| {
        e.current_diagnostics().is_some_and(|d| d.items.len() == 1)
    });
    f.editor.document.buffer.replace_range(0..3, "good");
    assert!(f.editor.current_diagnostics().is_none());
    until(&mut f.editor, |e| {
        e.current_diagnostics().is_some_and(|d| d.items.is_empty())
    });
    settle(&mut f.editor);
    assert!(f.editor.current_diagnostics().unwrap().items.is_empty());
}

#[test]
fn pull_and_unversioned_diagnostics_have_explicit_provenance() {
    for scenario in ["pull", "unversioned"] {
        let mut f = Fixture::new(scenario);
        f.ready();
        until(&mut f.editor, |e| e.current_diagnostics().is_some());
        assert_eq!(
            f.editor.current_diagnostics().unwrap().versioned,
            scenario == "pull"
        );
        f.editor.execute_command("Diagnostics").unwrap();
        assert_eq!(f.editor.view, View::Lsp);
        f.editor.handle_key(Key::Enter);
        assert_eq!(f.editor.view, View::Editing);
    }
}

#[test]
fn pull_diagnostics_refresh_after_indexing_without_a_document_edit() {
    let mut f = Fixture::new("pull-refresh");
    f.ready();
    until(&mut f.editor, |e| {
        e.current_diagnostics()
            .is_some_and(|d| d.versioned && d.items.iter().any(|d| d.message == "pull diagnostic"))
    });
    f.wait_log("textDocument/diagnostic", 2);
    assert!(
        f.log()
            .iter()
            .any(|m| { m["id"] == "diagnostic-refresh" && m.get("result") == Some(&Value::Null) })
    );
    assert!(!f.editor.document.buffer.is_modified());
}

#[test]
fn diagnostic_refresh_discards_in_flight_reports_for_unchanged_documents() {
    let mut f = Fixture::new("pull-refresh-race");
    f.ready();
    until(&mut f.editor, |e| e.current_diagnostics().is_some());
    settle(&mut f.editor);
    let items = &f.editor.current_diagnostics().unwrap().items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].message, "pull diagnostic");
    assert!(f.log().iter().any(|m| m["method"] == "$/cancelRequest"));
}

#[test]
fn pull_diagnostics_with_dependencies_refresh_when_another_document_changes() {
    let mut f = Fixture::new("pull-dependencies");
    f.editor
        .open_buffer(&f.directory.path().join("Other.kt"))
        .unwrap();
    f.editor.switch_buffer(1).unwrap();
    f.ready();
    until(&mut f.editor, |e| {
        e.current_diagnostics().is_some_and(|d| !d.items.is_empty())
    });
    let generation = f.editor.document.buffer.generation();
    f.editor.switch_buffer(2).unwrap();
    f.editor.document.buffer.insert("fixed");
    f.editor.document.buffer.commit();
    f.editor.switch_buffer(1).unwrap();
    f.editor.tick();
    assert!(f.editor.current_diagnostics().is_none());
    until(&mut f.editor, |e| {
        e.current_diagnostics().is_some_and(|d| d.items.is_empty())
    });
    assert_eq!(f.editor.document.buffer.generation(), generation);
}

#[test]
fn hover_signature_symbols_definitions_references_and_logs_use_contextual_views() {
    let mut f = Fixture::new("normal");
    f.ready();
    let original = f.editor.document.buffer.text().to_string();
    for command in [
        "Hover",
        "Signature",
        "Symbols",
        "LspInfo",
        "LspLog",
        "Doctor",
    ] {
        f.command(command);
        assert_eq!(f.editor.view, View::Lsp, "{command}");
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
            .unwrap();
        for (w, h) in [(1, 1), (2, 2), (10, 3)] {
            terminal.backend_mut().resize(w, h);
            terminal
                .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
                .unwrap();
        }
        escape(&mut f.editor);
        assert_eq!(f.editor.document.buffer.text().to_string(), original);
    }
    for command in ["Definition", "References"] {
        f.command(command);
        assert_eq!(f.editor.view, View::Lsp, "{command}");
        assert_eq!(f.editor.active_buffer_id, 1);
        f.editor.handle_key(Key::Enter);
        assert_eq!(
            f.editor.document.path().unwrap().file_name().unwrap(),
            "Other.kt"
        );
        assert_eq!(f.editor.document.buffer.cursor(), 0);
        f.editor.execute_command("b 1").unwrap();
    }
}

fn navigation_shortcut(editor: &mut Editor, key: char) {
    editor.handle_key(Key::Char('g'));
    editor.handle_key(Key::Char(key));
}

#[test]
fn help_defers_definition_results_without_losing_the_request_or_origin() {
    let mut f = Fixture::new("navigation-slow");
    f.ready();
    f.editor.document.buffer.set_cursor(4, false);
    let origin = f.editor.document.path().unwrap().to_path_buf();
    let viewport = f.editor.viewport.clone();
    navigation_shortcut(&mut f.editor, 'd');
    f.editor.handle_key(Key::F1);
    f.wait_log("textDocument/definition", 1);
    settle(&mut f.editor);
    assert!(f.editor.lsp.pending);
    assert!(f.editor.help.is_some());
    assert_eq!(f.editor.document.path(), Some(origin.as_path()));
    assert_eq!(f.editor.document.buffer.cursor(), 4);
    assert_eq!(f.editor.viewport, viewport);
    f.editor.handle_key(Key::Escape);
    assert_eq!(f.editor.document.path(), Some(origin.as_path()));
    assert_eq!(f.editor.document.buffer.cursor(), 4);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(f.editor.document.path().unwrap().ends_with("Other.kt"));
    f.editor.handle_key(Key::Ctrl('o'));
    assert_eq!(f.editor.document.path(), Some(origin.as_path()));
    assert_eq!(f.editor.document.buffer.cursor(), 4);
}

#[test]
fn navigation_shortcuts_jump_to_single_locations_and_preserve_history_and_dirty_buffers() {
    for scenario in ["normal", "navigation-location", "navigation-unicode"] {
        let mut f = Fixture::new(scenario);
        let source = f.directory.path().join("Main.kt");
        let target = f.directory.path().join("Other.kt");
        let target_text = "😀 e\u{301}界 target\r\n";
        fs::write(&target, target_text).unwrap();
        f.editor.open_buffer(&target).unwrap();
        f.editor.document.buffer.goto_line(1, true);
        f.editor.document.buffer.insert("unsaved");
        f.editor.document.buffer.commit();
        f.editor.switch_buffer(1).unwrap();
        f.editor.document.buffer.set_cursor(15, false);
        f.editor.viewport.left = 2;
        let viewport = f.editor.viewport.clone();
        f.ready();
        for key in ['d', 'r'] {
            // Counts must not leak into the next motion or history command.
            f.editor.handle_key(Key::Char('3'));
            navigation_shortcut(&mut f.editor, key);
            assert!(f.editor.pending_label().is_empty());
            assert!(f.editor.lsp.pending);
            until(&mut f.editor, |e| !e.lsp.pending);
            assert!(!f.editor.message_is_error, "{}", f.editor.message);
            assert_eq!(f.editor.view, View::Editing);
            assert_eq!(f.editor.mode, Mode::Normal);
            assert!(f.editor.lsp.panel.is_none());
            assert_eq!(f.editor.document.path(), Some(target.as_path()));
            assert_eq!(
                f.editor.document.buffer.cursor(),
                if scenario == "navigation-unicode" {
                    4
                } else {
                    0
                }
            );
            assert!(
                f.editor
                    .document
                    .buffer
                    .text()
                    .to_string()
                    .ends_with("unsaved")
            );
            assert_eq!(f.editor.buffer_count(), 2);
            f.editor.handle_key(Key::Ctrl('o'));
            assert_eq!(f.editor.document.path(), Some(source.as_path()));
            assert_eq!(f.editor.document.buffer.cursor(), 15);
            assert_eq!(f.editor.viewport, viewport);
            f.editor.handle_key(Key::Ctrl('i'));
            assert_eq!(f.editor.document.path(), Some(target.as_path()));
            f.editor.handle_key(Key::Ctrl('o'));
        }
        for command in ["Definition", "References"] {
            f.command(command);
            assert_eq!(f.editor.view, View::Lsp);
            assert_eq!(f.editor.active_buffer_id, 1);
            escape(&mut f.editor);
        }
        for method in ["textDocument/definition", "textDocument/references"] {
            let request = f.log().into_iter().find(|v| v["method"] == method).unwrap();
            assert_eq!(
                request["params"]["textDocument"]["uri"],
                json!(lsp::file_uri(&source).unwrap())
            );
            assert_eq!(
                request["params"]["position"],
                json!({"line": 1, "character": 5})
            );
        }
        assert_eq!(fs::read_to_string(target).unwrap(), target_text);
    }
}

#[test]
fn navigation_shortcuts_offer_multiple_locations_and_allow_cancel_or_selection() {
    let mut f = Fixture::new("navigation-multiple");
    f.ready();
    f.editor.document.buffer.set_cursor(7, false);
    let original = f.editor.document.buffer.text().to_string();
    let viewport = f.editor.viewport.clone();
    for (key, title) in [('d', "definition"), ('r', "references")] {
        for select in [false, true] {
            navigation_shortcut(&mut f.editor, key);
            until(&mut f.editor, |e| !e.lsp.pending);
            assert!(!f.editor.message_is_error, "{}", f.editor.message);
            assert_eq!(f.editor.view, View::Lsp);
            let panel = f.editor.lsp.panel.as_ref().unwrap();
            assert_eq!(panel.title, title);
            assert_eq!(panel.rows.len(), 2);
            assert!(panel.rows[0].label.contains("Main.kt"));
            assert!(panel.rows[1].label.contains("Other.kt"));
            for (w, h) in [(80, 24), (19, 7), (1, 1)] {
                Terminal::new(TestBackend::new(w, h))
                    .unwrap()
                    .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
                    .unwrap();
            }
            assert_eq!(f.editor.active_buffer_id, 1);
            assert_eq!(f.editor.document.buffer.cursor(), 7);
            assert_eq!(f.editor.viewport, viewport);
            f.editor.handle_key(Key::Down);
            f.editor.handle_key(Key::F1);
            f.editor.handle_key(Key::Down);
            f.editor.handle_key(Key::Enter);
            f.editor.handle_key(Key::Escape);
            assert_eq!(f.editor.view, View::Lsp);
            assert_eq!(f.editor.lsp.panel.as_ref().unwrap().selected, 1);
            if select {
                f.editor.handle_key(Key::Enter);
                assert_eq!(f.editor.active_buffer_id, 2);
                assert_eq!(f.editor.document.buffer.cursor(), 4);
                f.editor.handle_key(Key::Ctrl('o'));
            } else {
                escape(&mut f.editor);
            }
            assert_eq!(f.editor.view, View::Editing);
            assert_eq!(f.editor.active_buffer_id, 1);
            assert_eq!(f.editor.document.buffer.cursor(), 7);
            assert_eq!(f.editor.document.buffer.text().to_string(), original);
        }
    }
}

#[test]
fn navigation_shortcuts_handle_empty_unsupported_and_unavailable_targets() {
    for scenario in [
        "navigation-empty",
        "navigation-virtual",
        "navigation-missing",
        "unsupported",
    ] {
        let mut f = Fixture::new(scenario);
        f.ready();
        f.editor.document.buffer.set_cursor(4, false);
        let original = f.editor.document.buffer.text().to_string();
        for key in ['d', 'r'] {
            navigation_shortcut(&mut f.editor, key);
            until(&mut f.editor, |e| !e.lsp.pending);
            match scenario {
                "navigation-empty" => {
                    assert!(f.editor.message.contains("No language-server results"))
                }
                "navigation-virtual" => {
                    assert!(
                        f.editor.lsp.panel.as_ref().unwrap().rows[0]
                            .label
                            .contains("Unsupported location")
                    );
                    f.editor.handle_key(Key::Enter);
                    escape(&mut f.editor);
                }
                "navigation-missing" => {
                    assert!(f.editor.message_is_error);
                    assert!(f.editor.message.contains("target file is unavailable"));
                }
                "unsupported" => {
                    assert!(f.editor.message_is_error);
                    assert!(f.editor.message.contains("does not support"));
                }
                _ => unreachable!(),
            }
            assert_eq!(f.editor.view, View::Editing);
            assert_eq!(f.editor.active_buffer_id, 1);
            assert_eq!(f.editor.document.buffer.cursor(), 4);
            assert_eq!(f.editor.document.buffer.text().to_string(), original);
            assert!(f.editor.pending_label().is_empty());
            assert!(f.editor.navigate_history(true).is_err());
        }
    }
    let mut editor = Editor::new(Document::default());
    for key in ['d', 'r'] {
        navigation_shortcut(&mut editor, key);
        assert!(editor.message_is_error);
        assert!(editor.message.contains("Name a source file first"));
        assert!(!editor.lsp.pending);
        assert!(editor.pending_label().is_empty());
    }
}

#[test]
fn navigation_shortcuts_ignore_late_replies_after_cancellation_or_context_changes() {
    for key in ['d', 'r'] {
        for change in ["escape", "cursor", "edit", "switch", "mode"] {
            let mut f = Fixture::new("navigation-slow");
            f.ready();
            navigation_shortcut(&mut f.editor, key);
            assert!(f.editor.lsp.pending);
            f.wait_log(
                if key == 'd' {
                    "textDocument/definition"
                } else {
                    "textDocument/references"
                },
                1,
            );
            match change {
                "escape" => escape(&mut f.editor),
                "cursor" => f.editor.handle_key(Key::Right),
                "edit" => f.editor.handle_key(Key::Char('x')),
                "switch" => {
                    f.editor
                        .open_buffer(&f.directory.path().join("Other.kt"))
                        .unwrap();
                    f.editor.document.buffer.set_cursor(4, false);
                }
                "mode" => f.editor.handle_key(Key::Char('i')),
                _ => unreachable!(),
            }
            let id = f.editor.active_buffer_id;
            let cursor = f.editor.document.buffer.cursor();
            let text = f.editor.document.buffer.text().to_string();
            let mode = f.editor.mode;
            settle(&mut f.editor);
            assert!(!f.editor.lsp.pending);
            assert!(f.editor.lsp.panel.is_none());
            assert_eq!(f.editor.view, View::Editing);
            assert_eq!(f.editor.active_buffer_id, id);
            assert_eq!(f.editor.document.buffer.cursor(), cursor);
            assert_eq!(f.editor.document.buffer.text().to_string(), text);
            assert_eq!(f.editor.mode, mode);
        }
    }
}

#[test]
fn outline_and_workspace_symbols_preserve_source_and_record_navigation() {
    let mut f = Fixture::new("normal");
    f.ready();
    f.editor.document.buffer.set_cursor(4, false);
    f.editor.viewport.left = 2;
    let cursor = f.editor.document.buffer.cursor();
    let viewport = f.editor.viewport.clone();
    f.command("Outline");
    let panel = f.editor.lsp.panel.as_ref().unwrap();
    assert_eq!(panel.title, "Symbol outline");
    assert_eq!(panel.rows[0].label, "outer");
    assert_eq!(panel.rows[1].label, "  inner");
    escape(&mut f.editor);
    assert_eq!(f.editor.document.buffer.cursor(), cursor);
    assert_eq!(f.editor.viewport, viewport);
    f.command("WorkspaceSymbols other");
    assert!(
        f.editor.lsp.panel.as_ref().unwrap().rows[0]
            .label
            .contains("other")
    );
    f.editor.handle_key(Key::Enter);
    assert_eq!(
        f.editor.document.path().unwrap().file_name().unwrap(),
        "Other.kt"
    );
    assert_eq!(f.editor.document.buffer.cursor(), 4);
    f.editor.execute_command("back").unwrap();
    assert_eq!(
        f.editor.document.path().unwrap().file_name().unwrap(),
        "Main.kt"
    );
    assert_eq!(f.editor.document.buffer.cursor(), cursor);
    assert_eq!(f.editor.viewport, viewport);
    f.editor.execute_command("forward").unwrap();
    assert_eq!(f.editor.document.buffer.cursor(), 4);
    let request = f
        .log()
        .into_iter()
        .find(|v| v["method"] == "workspace/symbol")
        .unwrap();
    assert_eq!(request["params"], json!({"query":"other"}));
}

#[test]
fn lazy_workspace_symbols_resolve_opaque_data_and_reuse_dirty_buffers() {
    let mut f = Fixture::new("normal");
    let path = f.directory.path().join("Other.kt");
    f.editor.open_buffer(&path).unwrap();
    f.editor.document.buffer.set_cursor(9, true);
    f.editor.document.buffer.insert(" unsaved");
    f.editor.document.buffer.commit();
    f.editor.switch_buffer(1).unwrap();
    f.ready();
    f.command("WorkspaceSymbols lazy");
    f.editor.handle_key(Key::Enter);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    assert_eq!(f.editor.document.path(), Some(path.as_path()));
    assert_eq!(f.editor.document.buffer.cursor(), 4);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .contains("unsaved")
    );
    assert_eq!(f.editor.buffer_count(), 2);
    assert_eq!(fs::read_to_string(path).unwrap(), "bad other\n");
    let init = f
        .log()
        .into_iter()
        .find(|v| v["method"] == "initialize")
        .unwrap();
    assert_eq!(
        init["params"]["capabilities"]["workspace"]["symbol"]["resolveSupport"]["properties"],
        json!(["location.range"])
    );
}

#[test]
fn call_hierarchy_recurses_preserves_data_and_parent_selection_then_jumps_back() {
    let mut f = Fixture::new("normal");
    f.ready();
    f.editor.document.buffer.set_cursor(4, false);
    f.editor.viewport.left = 2;
    let viewport = f.editor.viewport.clone();
    f.command("Calls");
    assert_eq!(f.editor.lsp.panel.as_ref().unwrap().rows[0].label, "start");
    f.editor.handle_key(Key::Char('i'));
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    let panel = f.editor.lsp.panel.as_ref().unwrap();
    assert!(panel.title.contains("Incoming calls"));
    assert_eq!(panel.rows[0].label, "caller");
    assert!(panel.rows[0].detail.contains("2 call site(s)"));
    f.editor.handle_key(Key::Right);
    f.editor.handle_key(Key::Char('o'));
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(
        f.editor
            .lsp
            .panel
            .as_ref()
            .unwrap()
            .title
            .contains("Outgoing calls")
    );
    f.editor.handle_key(Key::Backspace);
    assert_eq!(f.editor.lsp.panel.as_ref().unwrap().left, 4);
    assert_eq!(f.editor.lsp.panel.as_ref().unwrap().rows[0].label, "caller");
    for (w, h) in [(1, 1), (2, 2), (24, 8), (100, 24)] {
        Terminal::new(TestBackend::new(w, h))
            .unwrap()
            .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
            .unwrap();
    }
    assert_eq!(f.editor.document.buffer.cursor(), 4);
    assert_eq!(f.editor.viewport, viewport);
    f.editor.handle_key(Key::Enter);
    assert_eq!(
        f.editor.document.path().unwrap().file_name().unwrap(),
        "Other.kt"
    );
    f.editor.execute_command("back").unwrap();
    assert_eq!(
        f.editor.document.path().unwrap().file_name().unwrap(),
        "Main.kt"
    );
    assert_eq!(f.editor.document.buffer.cursor(), 4);
    assert_eq!(f.editor.viewport, viewport);
    let requests: Vec<_> = f
        .log()
        .into_iter()
        .filter(|v| {
            v["method"] == "callHierarchy/incomingCalls"
                || v["method"] == "callHierarchy/outgoingCalls"
        })
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["params"]["item"]["name"], "caller");
}

#[test]
fn comprehension_reports_unsupported_empty_virtual_and_bounded_results() {
    let mut unsupported = Fixture::new("unsupported");
    unsupported.ready();
    for command in ["Outline", "WorkspaceSymbols foo", "Calls"] {
        assert!(
            unsupported
                .editor
                .execute_command(command)
                .unwrap_err()
                .to_string()
                .contains("does not support")
        );
    }
    let mut f = Fixture::new("calls-empty");
    f.ready();
    f.command("WorkspaceSymbols empty");
    assert_eq!(f.editor.view, View::Editing);
    assert!(f.editor.message.contains("No language-server results"));
    f.command("WorkspaceSymbols virtual");
    assert!(
        f.editor.lsp.panel.as_ref().unwrap().rows[0]
            .detail
            .contains("Unsupported virtual")
    );
    f.editor.handle_key(Key::Enter);
    assert_eq!(f.editor.active_buffer_id, 1);
    escape(&mut f.editor);
    f.command("WorkspaceSymbols many");
    assert_eq!(f.editor.lsp.panel.as_ref().unwrap().rows.len(), 4096);
    assert!(
        f.editor
            .lsp
            .panel
            .as_ref()
            .unwrap()
            .title
            .contains("result limit")
    );
    escape(&mut f.editor);
    f.command("Calls");
    f.editor.handle_key(Key::Char('o'));
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(
        f.editor.lsp.panel.as_ref().unwrap().rows[0]
            .label
            .contains("No calls")
    );
    f.editor.handle_key(Key::Backspace);
    assert_eq!(f.editor.lsp.panel.as_ref().unwrap().rows[0].label, "start");
}

#[test]
fn malformed_symbol_ranges_and_utf16_surrogates_never_move_source() {
    let mut f = Fixture::new("normal");
    fs::write(
        f.directory.path().join("Other.kt"),
        "bad other\r\n😀 e\u{301}\r\n",
    )
    .unwrap();
    f.ready();
    f.editor
        .execute_command("WorkspaceSymbols malformed")
        .unwrap();
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(f.editor.message_is_error);
    assert_eq!(f.editor.active_buffer_id, 1);
    f.command("WorkspaceSymbols surrogate");
    f.editor.handle_key(Key::Enter);
    assert!(f.editor.message_is_error);
    assert_eq!(f.editor.active_buffer_id, 1);
    assert_eq!(f.editor.buffer_count(), 1);
    assert_eq!(f.editor.document.buffer.cursor(), 0);
}

#[test]
fn comprehension_cancellation_and_document_guards_suppress_obsolete_results() {
    let mut f = Fixture::new("comprehension-slow");
    f.ready();
    f.editor.execute_command("Calls").unwrap();
    f.wait_log("textDocument/prepareCallHierarchy", 1);
    escape(&mut f.editor);
    settle(&mut f.editor);
    assert_eq!(f.editor.view, View::Editing);
    assert!(f.editor.lsp.panel.is_none());
    f.editor.execute_command("WorkspaceSymbols old").unwrap();
    f.wait_log("workspace/symbol", 1);
    f.editor.execute_command("WorkspaceSymbols new").unwrap();
    until(&mut f.editor, |e| !e.lsp.pending);
    assert_eq!(
        f.editor.lsp.panel.as_ref().unwrap().rows[0].label,
        "new Project"
    );
    escape(&mut f.editor);
    f.editor.execute_command("WorkspaceSymbols lazy").unwrap();
    until(&mut f.editor, |e| !e.lsp.pending);
    f.editor.handle_key(Key::Enter);
    f.wait_log("workspaceSymbol/resolve", 1);
    f.editor.document.buffer.insert("changed ");
    settle(&mut f.editor);
    assert_eq!(f.editor.active_buffer_id, 1);
    assert_eq!(f.editor.view, View::Editing);
    f.editor.document.buffer.set_cursor(0, false);
    f.command("Calls");
    f.editor.handle_key(Key::Char('i'));
    f.wait_log("callHierarchy/incomingCalls", 1);
    f.editor.handle_key(Key::Backspace);
    assert_eq!(f.editor.lsp.panel.as_ref().unwrap().rows[0].label, "start");
    settle(&mut f.editor);
    assert_eq!(f.editor.lsp.panel.as_ref().unwrap().rows[0].label, "start");
    f.editor.handle_key(Key::Char('o'));
    escape(&mut f.editor);
    f.editor.handle_key(Key::Char('i'));
    f.editor.paste("still editable ");
    escape(&mut f.editor);
    settle(&mut f.editor);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("still editable changed ")
    );
    assert_eq!(f.editor.view, View::Editing);
}

#[test]
fn call_hierarchy_depth_is_bounded_and_unsupported_file_types_are_explicit() {
    let mut f = Fixture::new("normal");
    f.ready();
    f.command("Calls");
    for _ in 0..16 {
        f.editor.handle_key(Key::Char('o'));
        until(&mut f.editor, |e| !e.lsp.pending);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
    }
    f.editor.handle_key(Key::Char('o'));
    assert!(f.editor.message.contains("depth limit"));
    assert!(!f.editor.lsp.pending);
    f.editor.handle_key(Key::Backspace);
    f.editor.handle_key(Key::Char('i'));
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    escape(&mut f.editor);
    f.editor
        .open_buffer(&f.directory.path().join("plain.txt"))
        .unwrap();
    assert!(
        f.editor
            .execute_command("WorkspaceSymbols")
            .unwrap_err()
            .to_string()
            .contains("No language server for this file type")
    );
}

#[test]
fn explicit_completion_accepts_with_enter_or_tab_after_optional_documentation() {
    for (accept, documentation) in [(Key::Enter, false), (Key::Tab, true)] {
        let mut f = Fixture::new("normal");
        f.ready();
        f.editor.handle_key(Key::Char('i'));
        f.editor.document.buffer.set_cursor(3, true);
        f.editor.handle_key(Key::Ctrl('n'));
        until(&mut f.editor, |e| e.view == View::Lsp && !e.lsp.pending);
        if documentation {
            f.editor.handle_key(Key::Char('d'));
            until(&mut f.editor, |e| !e.lsp.pending);
            assert!(
                f.editor.lsp.panel.as_ref().unwrap().rows[0]
                    .detail
                    .contains("Resolved")
            );
            assert!(
                f.editor
                    .document
                    .buffer
                    .text()
                    .to_string()
                    .starts_with("bad")
            );
        }
        f.editor.handle_key(accept);
        until(&mut f.editor, |e| !e.lsp.pending);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert_eq!(f.editor.mode, Mode::Insert);
        assert_eq!(f.editor.view, View::Editing);
        assert!(f.editor.lsp.panel.is_none());
        assert!(
            f.editor
                .document
                .buffer
                .text()
                .to_string()
                .starts_with("good")
        );
        assert_eq!(f.editor.document.buffer.cursor(), 5);
        assert!(f.editor.document.buffer.undo());
        assert!(
            f.editor
                .document
                .buffer
                .text()
                .to_string()
                .starts_with("bad")
        );
        assert!(
            fs::read_to_string(f.directory.path().join("Main.kt"))
                .unwrap()
                .starts_with("bad")
        );
    }
}

#[test]
fn explicit_completion_without_resolve_restores_normal_mode_and_selection() {
    for accept in [Key::Enter, Key::Tab] {
        let mut f = Fixture::new("auto-completion-no-resolve");
        f.ready();
        f.editor.document.buffer.set_cursor(3, false);
        f.command("Complete");
        let cursor = f.editor.document.buffer.cursor();
        f.editor.handle_key(Key::Down);
        f.editor.handle_key(Key::Down);
        f.editor.handle_key(Key::Up);
        assert_eq!(f.editor.document.buffer.cursor(), cursor);
        assert_eq!(f.editor.lsp.panel.as_ref().unwrap().selected, 1);
        assert_eq!(
            f.editor.lsp.panel.as_ref().unwrap().rows[1].label,
            "lengthSquared"
        );
        f.editor.handle_key(accept);
        assert_eq!(f.editor.view, View::Editing);
        assert_eq!(f.editor.mode, Mode::Normal);
        assert!(f.editor.lsp.panel.is_none());
        assert_eq!(
            f.editor.document.buffer.text().to_string(),
            "lengthSquared value\r\n😀 e\u{301}界\r\n"
        );
        f.editor.handle_key(Key::Char('u'));
        assert!(!f.editor.document.buffer.is_modified());
        assert!(
            !f.log()
                .iter()
                .any(|m| m["method"] == "completionItem/resolve")
        );
    }
}

fn auto_completion_fixture(scenario: &str, text: &str) -> Fixture {
    let mut f = Fixture::new(scenario);
    let path = f.directory.path().join("Main.kt");
    fs::write(&path, text).unwrap();
    f.editor.document = Document::open(&path).unwrap();
    f.ready();
    f.editor.handle_key(Key::Char('A'));
    f
}

fn type_completion(f: &mut Fixture, text: &str) {
    for ch in text.chars() {
        f.editor.handle_key(Key::Char(ch));
    }
}

fn wait_completion(f: &mut Fixture) {
    until(&mut f.editor, |e| e.lsp.completion.is_some());
    assert_eq!(f.editor.mode, Mode::Insert);
    assert_eq!(f.editor.view, View::Editing);
    assert!(f.editor.lsp.panel.is_none());
}

#[test]
fn automatic_completion_waits_for_startup_and_cancels_before_debounce() {
    let mut f = Fixture::new("auto-completion");
    f.editor.handle_key(Key::Char('A'));
    type_completion(&mut f, ".le");
    wait_completion(&mut f);
    assert_eq!(f.editor.lsp.completion.as_ref().unwrap().items.len(), 2);
    f.editor.handle_key(Key::Ctrl('e'));
    type_completion(&mut f, "n");
    f.editor.handle_key(Key::Escape);
    settle(&mut f.editor);
    assert!(f.editor.lsp.completion.is_none());
    assert_eq!(
        f.log()
            .iter()
            .filter(|m| m["method"] == "textDocument/completion")
            .count(),
        1
    );
}

#[test]
fn automatic_completion_debounces_filters_and_accepts_in_insert_with_undo() {
    for accept in [Key::Tab, Key::Enter] {
        let mut f = auto_completion_fixture("auto-completion", "value.\r\n");
        type_completion(&mut f, "le");
        f.editor.tick();
        assert!(!f.editor.lsp.pending);
        wait_completion(&mut f);
        let menu = f.editor.lsp.completion.as_ref().unwrap();
        assert_eq!(
            menu.items
                .iter()
                .map(|v| v["label"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["length", "lengthSquared"]
        );
        let requests: Vec<_> = f
            .log()
            .into_iter()
            .filter(|m| m["method"] == "textDocument/completion")
            .collect();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["params"]["context"]["triggerKind"], 1);
        let cursor = f.editor.document.buffer.cursor();
        for key in [Key::Down, Key::Ctrl('p'), Key::Ctrl('n'), Key::Up] {
            f.editor.handle_key(key);
        }
        assert_eq!(f.editor.document.buffer.cursor(), cursor);
        assert_eq!(f.editor.lsp.completion.as_ref().unwrap().selected, 0);
        f.editor.handle_key(accept);
        until(&mut f.editor, |e| !e.lsp.pending);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert_eq!(f.editor.mode, Mode::Insert);
        assert_eq!(f.editor.view, View::Editing);
        assert_eq!(
            f.editor.document.buffer.text().to_string(),
            "value.length \r\n"
        );
        assert_eq!(f.editor.document.buffer.cursor(), 13);
        assert!(f.editor.document.buffer.undo());
        assert_eq!(f.editor.document.buffer.text().to_string(), "value.le\r\n");
        assert_eq!(
            fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
            "value.\r\n"
        );
    }
}

#[test]
fn automatic_completion_preserves_auto_pairs_and_closer_overtyping() {
    let mut f = auto_completion_fixture("auto-completion", "\r\n");
    type_completion(&mut f, "(value.le");
    wait_completion(&mut f);
    assert_eq!(
        f.editor.document.buffer.text().to_string(),
        "(value.le)\r\n"
    );
    f.editor.handle_key(Key::Tab);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    assert_eq!(
        f.editor.document.buffer.text().to_string(),
        "(value.length)\r\n"
    );
    assert_eq!(f.editor.document.buffer.cursor(), 13);
    let generation = f.editor.document.buffer.generation();
    type_completion(&mut f, ")");
    assert_eq!(f.editor.document.buffer.cursor(), 14);
    assert_eq!(f.editor.document.buffer.generation(), generation);
    assert!(!f.editor.lsp.completion_scheduled());
    f.editor.handle_key(Key::Escape);
    f.editor.handle_key(Key::Char('u'));
    assert_eq!(
        f.editor.document.buffer.text().to_string(),
        "(value.le)\r\n"
    );
}

#[test]
fn automatic_completion_honors_triggers_incomplete_results_and_backspace() {
    let mut f = auto_completion_fixture("auto-completion", "value\n");
    type_completion(&mut f, ".");
    wait_completion(&mut f);
    assert!(
        f.editor
            .lsp
            .completion
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|v| v["insertTextFormat"] != 2)
    );
    let request = f
        .log()
        .into_iter()
        .find(|m| m["method"] == "textDocument/completion")
        .unwrap();
    assert_eq!(
        request["params"]["context"],
        json!({"triggerKind":2,"triggerCharacter":"."})
    );
    type_completion(&mut f, "le");
    wait_completion(&mut f);
    assert_eq!(f.editor.lsp.completion.as_ref().unwrap().items.len(), 2);
    let request = f
        .log()
        .into_iter()
        .rev()
        .find(|m| m["method"] == "textDocument/completion")
        .unwrap();
    assert_eq!(request["params"]["context"]["triggerKind"], 3);
    f.editor.handle_key(Key::Backspace);
    wait_completion(&mut f);
    assert_eq!(f.editor.lsp.completion.as_ref().unwrap().items.len(), 3);
    f.editor.handle_key(Key::Char(' '));
    settle(&mut f.editor);
    assert!(f.editor.lsp.completion.is_none());
}

#[test]
fn automatic_completion_uses_filter_text_and_works_without_resolve() {
    let mut f = auto_completion_fixture("auto-completion-no-resolve", "value.\n");
    type_completion(&mut f, "met");
    wait_completion(&mut f);
    assert_eq!(
        f.editor.lsp.completion.as_ref().unwrap().items[0]["label"],
        "call method(int value)"
    );
    f.editor.handle_key(Key::Tab);
    assert_eq!(
        f.editor.document.buffer.text().to_string(),
        "value.method \n"
    );
    assert!(
        !f.log()
            .iter()
            .any(|m| m["method"] == "completionItem/resolve")
    );
}

#[test]
fn automatic_completion_preserves_unicode_graphemes_utf16_and_crlf() {
    let mut f = auto_completion_fixture("auto-completion", "😀 \r\n");
    type_completion(&mut f, "e\u{301}");
    wait_completion(&mut f);
    let request = f
        .log()
        .into_iter()
        .find(|m| m["method"] == "textDocument/completion")
        .unwrap();
    assert_eq!(
        request["params"]["position"],
        json!({"line":0,"character":5})
    );
    f.editor.handle_key(Key::Tab);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert_eq!(
        f.editor.document.buffer.text().to_string(),
        "😀 e\u{301}clair \r\n"
    );
    assert_eq!(f.editor.document.buffer.cursor(), 10);
    assert!(f.editor.document.buffer.undo());
    assert_eq!(
        f.editor.document.buffer.text().to_string(),
        "😀 e\u{301}\r\n"
    );
}

#[test]
fn automatic_completion_applies_server_text_edit_over_identifier_suffix() {
    let mut f = auto_completion_fixture("auto-completion-text-edit", "value.lnxx\r\n");
    f.editor.document.buffer.set_cursor(7, true);
    type_completion(&mut f, "e");
    wait_completion(&mut f);
    f.editor.handle_key(Key::Enter);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert_eq!(
        f.editor.document.buffer.text().to_string(),
        "value.length \r\n"
    );
    assert_eq!(f.editor.document.buffer.cursor(), 13);
    assert_eq!(f.editor.view, View::Editing);
}

#[test]
fn completion_applies_utf16_text_and_insert_replace_edits_over_call_suffixes() {
    for scenario in [
        "auto-completion-call-edit",
        "auto-completion-insert-replace",
    ] {
        for accept in [Key::Enter, Key::Tab] {
            let original = "😀 value.l()tail\r\n";
            let mut f = auto_completion_fixture(scenario, original);
            f.editor.document.buffer.set_cursor(9, true);
            type_completion(&mut f, "e");
            wait_completion(&mut f);
            f.editor.handle_key(accept);
            until(&mut f.editor, |e| !e.lsp.pending);
            assert!(!f.editor.message_is_error, "{}", f.editor.message);
            assert_eq!(f.editor.view, View::Editing);
            assert_eq!(f.editor.mode, Mode::Insert);
            assert!(f.editor.lsp.panel.is_none());
            assert_eq!(
                f.editor.document.buffer.text().to_string(),
                "😀 value.length() tail\r\n"
            );
            assert_eq!(f.editor.document.buffer.cursor(), 17);
            f.editor.handle_key(Key::Escape);
            f.editor.handle_key(Key::Char('u'));
            assert_eq!(
                f.editor.document.buffer.text().to_string(),
                "😀 value.le()tail\r\n"
            );
            f.editor.handle_key(Key::Ctrl('r'));
            assert_eq!(
                f.editor.document.buffer.text().to_string(),
                "😀 value.length() tail\r\n"
            );
            assert_eq!(
                fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
                original
            );
        }
    }
}

#[test]
fn completion_escape_dismisses_without_editing_and_restores_enter_tab_fallback() {
    for explicit in [false, true] {
        let mut f = auto_completion_fixture("auto-completion", "value.\r\n");
        type_completion(&mut f, "le");
        if explicit {
            f.editor.handle_key(Key::Ctrl('n'));
            until(&mut f.editor, |e| e.view == View::Lsp && !e.lsp.pending);
        } else {
            wait_completion(&mut f);
        }
        let cursor = f.editor.document.buffer.cursor();
        let generation = f.editor.document.buffer.generation();
        f.editor.handle_key(Key::Escape);
        settle(&mut f.editor);
        assert_eq!(f.editor.view, View::Editing);
        assert_eq!(f.editor.mode, Mode::Insert);
        assert!(f.editor.lsp.completion.is_none());
        assert!(f.editor.lsp.panel.is_none());
        assert_eq!(f.editor.document.buffer.cursor(), cursor);
        assert_eq!(f.editor.document.buffer.generation(), generation);
        assert_eq!(f.editor.document.buffer.text().to_string(), "value.le\r\n");
        f.editor.handle_key(Key::Tab);
        f.editor.handle_key(Key::Enter);
        assert_eq!(
            f.editor.document.buffer.text().to_string(),
            "value.le\t\r\n\r\n"
        );
        f.editor.handle_key(Key::Escape);
        assert_eq!(f.editor.mode, Mode::Normal);
        assert!(
            !f.log()
                .iter()
                .any(|m| m["method"] == "completionItem/resolve")
        );
    }
}

#[test]
fn automatic_completion_cancels_on_movement_escape_dismissal_paste_and_buffer_switch() {
    for action in 0..5 {
        let mut f = auto_completion_fixture("auto-completion-slow", "value.\n");
        type_completion(&mut f, "le");
        f.wait_log("textDocument/completion", 1);
        match action {
            0 => f.editor.handle_key(Key::Left),
            1 => f.editor.handle_key(Key::Escape),
            2 => f.editor.handle_key(Key::Ctrl('e')),
            3 => f.editor.paste("more"),
            _ => f.editor.new_buffer(),
        }
        settle(&mut f.editor);
        assert!(f.editor.lsp.completion.is_none());
        assert!(!f.editor.lsp.pending);
        assert_eq!(f.editor.view, View::Editing);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert_eq!(
            f.log()
                .iter()
                .filter(|m| m["method"] == "textDocument/completion")
                .count(),
            1
        );
    }
}

#[test]
fn automatic_completion_discards_old_results_after_more_typing() {
    let mut f = auto_completion_fixture("auto-completion-slow", "value.\n");
    type_completion(&mut f, "l");
    f.wait_log("textDocument/completion", 1);
    type_completion(&mut f, "e");
    wait_completion(&mut f);
    assert_eq!(f.editor.lsp.completion.as_ref().unwrap().items.len(), 2);
    assert!(f.log().iter().any(|m| m["method"] == "$/cancelRequest"));
    assert_eq!(f.editor.document.buffer.text().to_string(), "value.le\n");
}

#[test]
fn completion_cancels_resolve_after_typing_buffer_switch_reload_or_document_changes() {
    for explicit in [false, true] {
        for action in 0..4 {
            let mut f = auto_completion_fixture("auto-completion-resolve-slow", "value.\n");
            let main = f.editor.active_buffer_id;
            f.editor
                .open_buffer(&f.directory.path().join("Other.kt"))
                .unwrap();
            let other = f.editor.active_buffer_id;
            f.editor.switch_buffer(main).unwrap();
            f.editor.handle_key(Key::Char('A'));
            type_completion(&mut f, "le");
            if explicit {
                f.editor.handle_key(Key::Ctrl('n'));
                until(&mut f.editor, |e| e.view == View::Lsp && !e.lsp.pending);
            } else {
                wait_completion(&mut f);
            }
            f.editor.handle_key(Key::Tab);
            f.wait_log("completionItem/resolve", 1);
            match action {
                0 => {
                    if explicit {
                        f.editor.handle_key(Key::Escape);
                    }
                    type_completion(&mut f, "x");
                }
                1 => f.editor.switch_buffer(other).unwrap(),
                2 => {
                    f.editor.document = Document::open(&f.directory.path().join("Main.kt")).unwrap()
                }
                _ => f.editor.document.buffer.insert("changed"),
            }
            let expected = f.editor.document.buffer.text().to_string();
            settle(&mut f.editor);
            assert_eq!(f.editor.document.buffer.text().to_string(), expected);
            assert!(f.editor.lsp.completion.is_none());
            assert!(f.editor.lsp.panel.is_none());
            assert!(!f.editor.lsp.pending);
            assert!(!f.editor.message_is_error, "{}", f.editor.message);
            assert_eq!(
                fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
                "value.\n"
            );
        }
    }
}

#[test]
fn completion_rechecks_visible_candidates_before_acceptance() {
    for accept in [Key::Enter, Key::Tab] {
        let mut f = auto_completion_fixture("auto-completion", "value.\r\n");
        type_completion(&mut f, "le");
        wait_completion(&mut f);
        // Invalidate the candidate without polling between the change and key.
        f.editor.document.buffer.insert("x");
        f.editor.handle_key(accept);
        settle(&mut f.editor);
        assert_eq!(f.editor.view, View::Editing);
        assert!(f.editor.lsp.completion.is_none());
        assert_eq!(
            f.editor.document.buffer.text().to_string(),
            if accept == Key::Tab {
                "value.lex\t\r\n"
            } else {
                "value.lex\r\n\r\n"
            }
        );
        assert!(
            !f.log()
                .iter()
                .any(|m| m["method"] == "completionItem/resolve")
        );
    }
}

#[test]
fn completion_accepts_additional_edits_and_multiline_text_without_a_preview() {
    for explicit in [false, true] {
        for (scenario, expected) in [
            (
                "auto-completion-extra",
                "import sample\r\nvalue.length \r\n",
            ),
            ("auto-completion-multiline", "value.length\r\nnext \r\n"),
        ] {
            for accept in [Key::Tab, Key::Enter] {
                let mut f = auto_completion_fixture(scenario, "value.\r\n");
                type_completion(&mut f, "le");
                if explicit {
                    f.editor.handle_key(Key::Ctrl('n'));
                    until(&mut f.editor, |e| e.view == View::Lsp && !e.lsp.pending);
                } else {
                    wait_completion(&mut f);
                }
                f.editor.handle_key(accept);
                until(&mut f.editor, |e| !e.lsp.pending);
                assert!(!f.editor.message_is_error, "{}", f.editor.message);
                assert_eq!(f.editor.view, View::Editing);
                assert!(f.editor.lsp.panel.is_none());
                assert!(f.editor.lsp.completion.is_none());
                assert_eq!(f.editor.document.buffer.text().to_string(), expected);
                assert_eq!(
                    f.editor.document.buffer.cursor(),
                    expected.chars().count() - 2
                );
                assert!(
                    !f.log()
                        .iter()
                        .any(|m| m["method"] == "workspace/executeCommand")
                );
                assert!(f.editor.document.buffer.undo());
                assert_eq!(f.editor.document.buffer.text().to_string(), "value.le\r\n");
                assert!(f.editor.document.buffer.redo());
                assert_eq!(f.editor.document.buffer.text().to_string(), expected);
                assert_eq!(
                    fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
                    "value.\r\n"
                );
                assert_eq!(f.editor.mode, Mode::Insert);
            }
        }
    }
}

#[test]
fn completion_adds_or_reuses_one_space_and_leaves_insert_cursor_after_it() {
    for scenario in ["auto-completion", "auto-completion-space"] {
        for (suffix, separator) in [("", " "), (" next", " "), ("\tnext", " \t"), (";", " ")] {
            for accept in [Key::Tab, Key::Enter] {
                let original = format!("😀 value.{suffix}\r\n");
                let mut f = auto_completion_fixture(scenario, &original);
                f.editor.document.buffer.set_cursor(8, true);
                type_completion(&mut f, "le");
                wait_completion(&mut f);
                f.editor.handle_key(accept);
                until(&mut f.editor, |e| !e.lsp.pending);
                assert!(!f.editor.message_is_error, "{}", f.editor.message);
                let expected = if scenario == "auto-completion-space" && suffix.starts_with(' ') {
                    // Existing source whitespace is never removed.
                    format!("😀 value.length {suffix}\r\n")
                } else {
                    format!(
                        "😀 value.length{separator}{}\r\n",
                        suffix.trim_start_matches([' ', '\t'])
                    )
                };
                assert_eq!(f.editor.document.buffer.text().to_string(), expected);
                assert_eq!(f.editor.document.buffer.cursor(), 15);
                assert_eq!(f.editor.view, View::Editing);
                assert_eq!(f.editor.mode, Mode::Insert);
                assert!(f.editor.document.buffer.undo());
                assert_eq!(
                    f.editor.document.buffer.text().to_string(),
                    format!("😀 value.le{suffix}\r\n")
                );
            }
        }
    }
}

#[test]
fn completion_commands_from_both_lists_accept_inline_with_space_and_guarded_followup() {
    for explicit in [false, true] {
        for accept in [Key::Tab, Key::Enter] {
            for scenario in [
                "auto-completion-command",
                "auto-completion-command-primary",
                "auto-completion-command-other",
            ] {
                let mut f = auto_completion_fixture(scenario, "😀 value.\r\n");
                type_completion(&mut f, "le");
                if explicit {
                    f.editor.handle_key(Key::Ctrl('n'));
                    until(&mut f.editor, |e| e.view == View::Lsp && !e.lsp.pending);
                } else {
                    wait_completion(&mut f);
                }
                f.editor.handle_key(accept);
                until(&mut f.editor, |e| !e.lsp.pending);
                assert!(!f.editor.message_is_error, "{}", f.editor.message);
                assert_eq!(f.editor.view, View::Editing);
                assert_eq!(f.editor.mode, Mode::Insert);
                assert!(f.editor.lsp.panel.is_none());
                assert_eq!(
                    f.editor.document.buffer.text().to_string(),
                    "😀 value.length() \r\n"
                );
                assert_eq!(f.editor.document.buffer.cursor(), 17);
                assert!(f.editor.document.buffer.undo());
                if scenario == "auto-completion-command-primary" {
                    assert_eq!(
                        f.editor.document.buffer.text().to_string(),
                        "😀 value.length\r\n"
                    );
                    assert!(f.editor.document.buffer.undo());
                }
                assert_eq!(
                    f.editor.document.buffer.text().to_string(),
                    "😀 value.le\r\n"
                );
                assert_eq!(
                    fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
                    "😀 value.\r\n"
                );
                if scenario == "auto-completion-command-other" {
                    f.editor
                        .open_buffer(&f.directory.path().join("Other.kt"))
                        .unwrap();
                    assert_eq!(f.editor.document.buffer.text().to_string(), "good other\n");
                    assert!(f.editor.document.buffer.undo());
                    assert!(!f.editor.document.buffer.is_modified());
                }
            }
        }
    }
}

#[test]
fn completion_commands_without_a_cursor_edit_still_finish_with_a_space() {
    for (scenario, expected) in [
        ("auto-completion-command-no-edit", "value.length \r\n"),
        (
            "auto-completion-command-import",
            "import sample\r\nvalue.length \r\n",
        ),
        ("auto-completion-command-other-only", "value.length \r\n"),
    ] {
        let mut f = auto_completion_fixture(scenario, "value.\r\n");
        type_completion(&mut f, "le");
        wait_completion(&mut f);
        f.editor.handle_key(Key::Enter);
        until(&mut f.editor, |e| !e.lsp.pending);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert_eq!(f.editor.view, View::Editing);
        assert_eq!(f.editor.document.buffer.text().to_string(), expected);
        assert_eq!(
            f.editor.document.buffer.cursor(),
            expected.chars().count() - 2
        );
        assert!(f.editor.document.buffer.undo());
        assert_eq!(
            f.editor.document.buffer.text().to_string(),
            "value.length\r\n"
        );
        assert!(f.editor.document.buffer.undo());
        assert_eq!(f.editor.document.buffer.text().to_string(), "value.le\r\n");
        if scenario == "auto-completion-command-other-only" {
            f.editor
                .open_buffer(&f.directory.path().join("Other.kt"))
                .unwrap();
            assert_eq!(f.editor.document.buffer.text().to_string(), "good other\n");
            assert!(f.editor.document.buffer.undo());
            assert!(!f.editor.document.buffer.is_modified());
        }
    }
}

#[test]
fn completion_command_followups_reject_cancellation_conflicts_and_invalid_edits() {
    for change in [
        "typing", "move", "escape", "dismiss", "switch", "disk", "invalid", "error",
    ] {
        let scenario = match change {
            "invalid" => "auto-completion-command-invalid",
            "error" => "auto-completion-command-error",
            _ => "auto-completion-command-slow",
        };
        let mut f = auto_completion_fixture(scenario, "value.\n");
        type_completion(&mut f, "le");
        wait_completion(&mut f);
        f.editor.handle_key(Key::Tab);
        f.wait_log("workspace/executeCommand", 1);
        match change {
            "typing" => type_completion(&mut f, "x"),
            "move" => f.editor.handle_key(Key::Left),
            "escape" => f.editor.handle_key(Key::Escape),
            "dismiss" => f.editor.handle_key(Key::Ctrl('e')),
            "switch" => f
                .editor
                .open_buffer(&f.directory.path().join("Other.kt"))
                .unwrap(),
            "disk" => fs::write(f.directory.path().join("Main.kt"), "external\n").unwrap(),
            _ => (),
        }
        settle(&mut f.editor);
        assert_eq!(f.editor.view, View::Editing);
        assert!(f.editor.lsp.panel.is_none());
        assert!(
            !f.log()
                .iter()
                .any(|m| m["id"] == 777 && m["result"]["applied"] == true)
        );
        if change == "switch" {
            f.editor
                .open_buffer(&f.directory.path().join("Main.kt"))
                .unwrap();
        }
        assert_eq!(
            f.editor.document.buffer.text().to_string(),
            if change == "typing" {
                "value.lex\n"
            } else {
                "value.le\n"
            }
        );
        if matches!(change, "disk" | "invalid" | "error") {
            assert!(f.editor.message_is_error, "{change}: {}", f.editor.message);
        }
    }
}

#[test]
fn completion_rejects_malformed_additional_edits_without_applying_the_main_edit() {
    let mut f = auto_completion_fixture("auto-completion-invalid-extra", "value.\n");
    type_completion(&mut f, "le");
    wait_completion(&mut f);
    f.editor.handle_key(Key::Enter);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(f.editor.message_is_error);
    assert!(f.editor.message.contains("Invalid additional"));
    assert_eq!(f.editor.document.buffer.text().to_string(), "value.le\n");
}

#[test]
fn automatic_completion_checks_disk_conflicts_before_applying() {
    let mut f = auto_completion_fixture("auto-completion", "value.\n");
    type_completion(&mut f, "le");
    wait_completion(&mut f);
    fs::write(f.directory.path().join("Main.kt"), "external\n").unwrap();
    f.editor.handle_key(Key::Tab);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(f.editor.message_is_error);
    assert!(f.editor.message.contains("changed"));
    assert_eq!(f.editor.document.buffer.text().to_string(), "value.le\n");
}

#[test]
fn automatic_completion_errors_and_unsupported_servers_do_not_interrupt_typing() {
    for scenario in [
        "unsupported",
        "auto-completion-error",
        "auto-completion-timeout",
    ] {
        let mut f = auto_completion_fixture(scenario, "value.\n");
        f.editor.message = "typing".into();
        type_completion(&mut f, "le");
        if scenario != "unsupported" {
            f.wait_log("textDocument/completion", 1);
            until(&mut f.editor, |e| !e.lsp.pending);
        }
        settle(&mut f.editor);
        assert!(!f.editor.message_is_error);
        assert_eq!(f.editor.message, "typing");
        assert_eq!(f.editor.mode, Mode::Insert);
        assert!(f.editor.lsp.completion.is_none());
    }
}

#[test]
fn automatic_completion_popup_preserves_source_cursor_and_handles_small_screens() {
    let mut f = auto_completion_fixture("auto-completion-many", "\t😀 \n");
    type_completion(&mut f, "i");
    wait_completion(&mut f);
    for _ in 0..12 {
        f.editor.handle_key(Key::Down);
    }
    for (width, height) in [(1, 1), (1, 3), (2, 2), (8, 4), (80, 8)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
            .unwrap();
        if height > 2 {
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < width && cursor.y < height - 2);
        }
        if width == 80 {
            let output: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(output.contains("item12"));
            assert!(output.contains("INSERT"));
            assert!(output.contains("Tab/Enter accept"));
            assert_eq!(terminal.get_cursor_position().unwrap().y, 0);
        }
    }
    f.editor.handle_key(Key::Ctrl('e'));
    assert_eq!(f.editor.mode, Mode::Insert);
    f.editor.handle_key(Key::Enter);
    assert_eq!(f.editor.document.buffer.text().to_string(), "\t😀 i\n\n");

    let mut f = auto_completion_fixture("auto-completion-many", "first\nsecond\nthird\nlast ");
    let end = f.editor.document.buffer.text().len_chars();
    f.editor.document.buffer.set_cursor(end, true);
    type_completion(&mut f, "i");
    wait_completion(&mut f);
    let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
    terminal
        .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
        .unwrap();
    assert_eq!(terminal.get_cursor_position().unwrap().y, 3);
    let output: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(output.contains("item00"));
    assert!(output.contains("last i"));
}

#[test]
fn automatic_completion_popup_covers_source_text_and_restores_it_on_dismissal() {
    let source_lines = format!("// {}\r\n", "界e\u{301}x ".repeat(24)).repeat(3);
    for above in [false, true] {
        let text = if above {
            format!("{source_lines}value.")
        } else {
            format!("value.\r\n{source_lines}")
        };
        let mut f = auto_completion_fixture("auto-completion", &text);
        if above {
            let end = f.editor.document.buffer.text().len_chars();
            f.editor.document.buffer.set_cursor(end, true);
        }
        type_completion(&mut f, "le");
        wait_completion(&mut f);

        let theme = Theme::night();
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        let menu = f.editor.lsp.completion.take();
        let source = terminal
            .draw(|frame| ui::render(frame, &mut f.editor, &theme))
            .unwrap()
            .buffer
            .clone();
        let cursor = terminal.get_cursor_position().unwrap();
        f.editor.lsp.completion = menu;

        let top = if above { 0 } else { 1 };
        for selected in 0..2 {
            terminal
                .draw(|frame| ui::render(frame, &mut f.editor, &theme))
                .unwrap();
            assert_eq!(terminal.get_cursor_position().unwrap(), cursor);
            let cells = terminal.backend().buffer();
            let hint = format!(
                " {}/2 Tab/Enter accept  ↑↓ select  Esc/Ctrl-e dismiss",
                selected + 1
            );
            for (row, label) in [" length", " lengthSquared", &hint].iter().enumerate() {
                let y = top + row as u16;
                let rendered: String = (11..75).map(|x| cells[(x, y)].symbol()).collect();
                assert_eq!(rendered, format!("{label:<64}"), "above={above}, row={row}");
                for x in 11..75 {
                    assert_eq!(
                        cells[(x, y)].bg,
                        if row == selected {
                            theme.selection
                        } else {
                            theme.status
                        }
                    );
                }
            }
            f.editor.handle_key(Key::Down);
        }

        f.editor.handle_key(Key::Ctrl('e'));
        let restored = terminal
            .draw(|frame| ui::render(frame, &mut f.editor, &theme))
            .unwrap();
        assert_eq!(restored.buffer, &source);
        assert_eq!(terminal.get_cursor_position().unwrap(), cursor);
    }
}

#[test]
fn completion_commands_accept_once_and_apply_their_guarded_edit_inline() {
    for accept in [Key::Tab, Key::Enter] {
        let mut f = Fixture::new("completion-command");
        f.ready();
        let before = f.editor.document.buffer.text().to_string();
        f.command("Complete");
        escape(&mut f.editor);
        assert!(
            !f.log()
                .iter()
                .any(|m| m["method"] == "workspace/executeCommand")
        );
        f.command("Complete");
        f.editor.handle_key(accept);
        until(&mut f.editor, |e| !e.lsp.pending);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert_eq!(f.editor.view, View::Editing);
        assert_eq!(f.editor.mode, Mode::Normal);
        assert!(f.editor.lsp.panel.is_none());
        assert_eq!(
            f.editor.document.buffer.text().to_string(),
            before.replacen("bad", "good", 1)
        );
        assert_eq!(f.editor.document.buffer.cursor(), 5);
        settle(&mut f.editor);
        assert_eq!(
            f.log()
                .iter()
                .filter(|m| m["method"] == "workspace/executeCommand")
                .count(),
            1
        );
        assert!(
            f.log()
                .iter()
                .any(|m| m["id"] == 777 && m["result"]["applied"] == true)
        );
        assert!(f.editor.document.buffer.undo());
        assert_eq!(f.editor.document.buffer.text().to_string(), before);
        assert_eq!(
            fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
            before
        );
    }
}

#[test]
fn unadvertised_completion_commands_are_rejected_before_any_edit() {
    let mut f = Fixture::new("completion-command-unadvertised");
    f.ready();
    f.command("Complete");
    f.editor.handle_key(Key::Enter);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(
        f.editor.message.contains("did not advertise"),
        "{}",
        f.editor.message
    );
    assert!(!f.editor.document.buffer.is_modified());
    assert!(
        !f.log()
            .iter()
            .any(|m| m["method"] == "workspace/executeCommand")
    );
}

#[test]
fn completion_prefix_includes_combining_graphemes() {
    let mut f = Fixture::new("normal");
    f.ready();
    f.editor.document.buffer.replace_range(0..3, "e\u{301}");
    f.editor.document.buffer.commit();
    f.editor.mode = Mode::Insert;
    f.editor.document.buffer.set_cursor(2, true);
    f.editor.handle_key(Key::Ctrl('n'));
    until(&mut f.editor, |e| e.view == View::Lsp && !e.lsp.pending);
    f.editor.handle_key(Key::Enter);
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("good value")
    );
}

#[test]
fn rename_edits_multiple_unsaved_buffers_atomically_and_preserves_histories() {
    let mut f = Fixture::new("normal");
    f.editor
        .open_buffer(&f.directory.path().join("Other.kt"))
        .unwrap();
    f.editor.document.buffer.insert("x");
    f.editor.document.buffer.commit();
    f.editor.switch_buffer(1).unwrap();
    f.ready();
    f.command("Rename named");
    let before = fs::read(f.directory.path().join("Other.kt")).unwrap();
    f.editor.handle_key(Key::Enter);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("named")
    );
    assert_eq!(f.editor.dirty_buffer_count(), 2);
    assert!(f.editor.execute_command("q").is_err());
    f.editor.switch_buffer(2).unwrap();
    assert!(f.editor.document.buffer.undo());
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("xbad")
    );
    assert!(f.editor.document.buffer.undo());
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("bad")
    );
    assert_eq!(
        fs::read(f.directory.path().join("Other.kt")).unwrap(),
        before
    );
}

#[test]
fn formatting_quick_fixes_resolved_actions_and_command_edits_need_explicit_apply() {
    for action in 0..4 {
        let mut f = Fixture::new("normal");
        f.ready();
        if action == 0 {
            f.command("Format");
        } else {
            f.command("CodeAction");
            for _ in 1..action {
                f.editor.handle_key(Key::Down);
            }
            f.editor.handle_key(Key::Enter);
            until(&mut f.editor, |e| {
                e.lsp
                    .panel
                    .as_ref()
                    .is_some_and(|p| p.title.starts_with("Apply"))
            });
        }
        assert!(
            f.editor
                .document
                .buffer
                .text()
                .to_string()
                .starts_with("bad")
        );
        f.editor.handle_key(Key::Enter);
        assert!(!f.editor.message_is_error, "{}", f.editor.message);
        assert!(
            f.editor
                .document
                .buffer
                .text()
                .to_string()
                .starts_with("good")
        );
        assert!(
            fs::read_to_string(f.directory.path().join("Main.kt"))
                .unwrap()
                .starts_with("bad")
        );
        if action == 2 {
            let start = Instant::now();
            while !f
                .log()
                .iter()
                .any(|v| v["id"] == 777 && v["result"]["applied"] == true)
            {
                f.editor.tick();
                thread::sleep(Duration::from_millis(5));
                assert!(start.elapsed() < Duration::from_secs(3));
            }
        }
    }
}

#[test]
fn stale_hover_after_edit_switch_reload_cursor_or_new_request_never_opens_old_results() {
    for change in 0..5 {
        let mut f = Fixture::new("slow");
        f.ready();
        f.editor.execute_command("Hover").unwrap();
        f.wait_log("textDocument/hover", 1);
        match change {
            0 => f.editor.document.buffer.insert("new"),
            1 => f.editor.new_buffer(),
            2 => f.editor.execute_command("e!").unwrap(),
            3 => f.editor.document.buffer.move_right(false),
            _ => {
                f.editor.execute_command("Signature").unwrap();
            }
        }
        settle(&mut f.editor);
        if change == 4 {
            assert_eq!(f.editor.lsp.panel.as_ref().unwrap().title, "signatureHelp");
        } else {
            assert_eq!(f.editor.view, View::Editing);
        }
    }
}

#[test]
fn delayed_initialization_and_overlapping_imports_keep_editing_and_elapsed_status_live() {
    let mut f = Fixture::new("lifecycle-held");
    f.editor.lsp.config.initialize_timeout = Duration::from_secs(6);
    f.wait_log("initialize", 1);
    assert_eq!(
        f.editor.lsp.servers.values().next().unwrap().status.phase,
        Phase::Starting
    );
    f.progress(json!(1), json!({"kind":"begin","title":"Gradle import"}));
    f.progress(
        json!("1"),
        json!({"kind":"begin","title":"Maven dependencies"}),
    );
    until(&mut f.editor, |e| {
        e.lsp.servers.values().any(|s| s.status.progress.len() == 2)
    });
    f.progress(json!(1), json!({"kind":"end"}));
    f.progress(json!("unrelated"), json!({"kind":"end"}));
    f.progress(json!("partial-results"), json!([{"name":"symbol"}]));
    f.progress(
        json!("1"),
        json!({"kind":"report","message":"Resolving project JDK"}),
    );
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.message.contains("Resolving project JDK"))
    });
    let server = f.editor.lsp.servers.values().next().unwrap();
    assert_eq!(server.status.phase, Phase::Indexing);
    assert_eq!(server.status.progress.len(), 1);
    assert!(server.status.progress[0].contains("Maven dependencies"));
    assert!(!server.status.progress[0].contains('%'));
    assert!(server.capabilities.is_null());
    f.progress(json!("1"), json!({"kind":"end"}));
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Starting)
    });
    f.progress(
        json!("index"),
        json!({"kind":"begin","title":"Index project"}),
    );
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Indexing)
    });
    let since = f.editor.lsp.servers.values().next().unwrap().status.since;
    f.editor.handle_key(Key::Char('i'));
    f.editor.paste("unsaved 😀\r\n");
    escape(&mut f.editor);
    let expected = f.editor.document.buffer.text().to_string();
    f.editor.execute_command("LspInfo").unwrap();
    until(&mut f.editor, |e| {
        e.lsp
            .panel
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.label == "Status: Indexing 1s")
    });
    assert!(
        f.editor
            .lsp
            .panel
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.label.contains(":Doctor"))
    );
    fs::write(
        f.directory.path().join("protocol.jsonl.initialize"),
        "ready",
    )
    .unwrap();
    f.ready();
    assert_eq!(
        f.editor.lsp.servers.values().next().unwrap().status.phase,
        Phase::Indexing
    );
    f.progress(
        json!("index"),
        json!({"kind":"report","message":"Indexing sources","percentage":37}),
    );
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.message.contains("37%"))
    });
    assert_eq!(
        f.editor.lsp.servers.values().next().unwrap().status.since,
        since
    );
    f.progress(json!("index"), json!({"kind":"end"}));
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Ready)
    });
    f.wait_log("textDocument/didOpen", 1);
    assert!(f.log().iter().any(|m| m["method"] == "textDocument/didOpen"
        && m["params"]["textDocument"]["text"] == expected));
    escape(&mut f.editor);
    assert!(f.editor.document.buffer.undo());
    assert_eq!(
        fs::read(f.directory.path().join("Main.kt")).unwrap(),
        "bad value\r\n😀 e\u{301}界\r\n".as_bytes()
    );
}

#[test]
fn import_errors_remain_actionable_through_progress_and_recover_on_explicit_restart() {
    for (method, kind) in [
        ("window/logMessage", 1),
        ("window/showMessage", 2),
        ("window/showMessageRequest", 1),
    ] {
        let mut f = Fixture::new("lifecycle");
        f.ready();
        until(&mut f.editor, |e| e.current_diagnostics().is_some());
        f.progress(
            json!("import"),
            json!({"kind":"begin","title":"Gradle import"}),
        );
        let mut frame = json!({"method":method,"params":{"type":kind,"message":"Gradle import failed: project JDK 21 missing"}});
        if method == "window/showMessageRequest" {
            frame["id"] = json!("setup-error");
        }
        f.notify(frame);
        f.progress(json!("import"), json!({"kind":"end"}));
        until(&mut f.editor, |e| {
            e.lsp
                .servers
                .values()
                .any(|s| s.status.phase == Phase::Degraded && s.status.progress.is_empty())
        });
        assert!(f.editor.lsp_summary().contains("Degraded"));
        assert!(f.editor.lsp_summary().contains(":LspInfo"));
        f.editor.execute_command("LspInfo").unwrap();
        let text = f
            .editor
            .lsp
            .panel
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .map(|r| r.label.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for message in [
            "project JDK 21 missing",
            ":LspLog",
            ":Doctor",
            ":LspRestart",
        ] {
            assert!(text.contains(message), "{text}");
        }
        f.editor.execute_command("LspLog").unwrap();
        f.notify(json!({"method":"window/logMessage","params":{"type":3,"message":"Maven repository authentication details"}}));
        until(&mut f.editor, |e| {
            e.lsp
                .panel
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.label.contains("authentication details"))
        });
        escape(&mut f.editor);
        f.editor.document.buffer.insert("unsaved ");
        f.editor.document.buffer.commit();
        settle(&mut f.editor);
        assert_eq!(f.log().iter().filter(|m| m["event"] == "start").count(), 1);
        f.editor.execute_command("LspRestart").unwrap();
        assert!(f.editor.current_diagnostics().is_none());
        f.wait_log("textDocument/didOpen", 2);
        until(&mut f.editor, |e| {
            e.lsp
                .servers
                .values()
                .any(|s| s.status.phase == Phase::Ready)
        });
        assert!(
            f.editor
                .document
                .buffer
                .text()
                .to_string()
                .starts_with("unsaved ")
        );
        assert!(f.editor.document.buffer.undo());
    }
}

#[test]
fn java_startup_notifications_do_not_hide_import_errors_or_overlapping_work() {
    let mut f = Fixture::new("lifecycle");
    let command = f
        .editor
        .lsp
        .config
        .commands
        .remove(&Language::Kotlin)
        .unwrap();
    let path = f.directory.path().join("Main.java");
    fs::write(&path, "bad value\n").unwrap();
    f.editor = Editor::new(Document::open(&path).unwrap());
    f.editor.lsp.config.commands.insert(Language::Java, command);
    f.ready();
    f.notify(json!({"method":"language/status","params":{"type":"Starting","message":"Importing Maven projects"}}));
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Indexing)
    });
    f.progress(
        json!("java"),
        json!({"kind":"begin","title":"Resolve dependencies"}),
    );
    f.notify(
        json!({"method":"language/status","params":{"type":"Started","message":"Service started"}}),
    );
    until(&mut f.editor, |e| {
        e.lsp.servers.values().any(|s| s.status.progress.len() == 1)
    });
    f.notify(json!({"method":"language/status","params":{"type":"Error","message":"Maven import failed: unresolved parent POM"}}));
    f.progress(json!("java"), json!({"kind":"end"}));
    f.notify(json!({"method":"language/status","params":{"type":"ServiceReady","message":"Service ready"}}));
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Degraded && s.status.progress.is_empty())
    });
    assert!(
        f.editor
            .lsp
            .servers
            .values()
            .next()
            .unwrap()
            .status
            .message
            .contains("unresolved parent POM")
    );
}

#[test]
fn crash_retry_limit_invalidates_diagnostics_and_never_loops_on_edits() {
    let mut f = Fixture::new("crash-always");
    f.editor.lsp.config.restart_limit = 1;
    f.ready();
    until(&mut f.editor, |e| e.current_diagnostics().is_some());
    f.editor.execute_command("Hover").unwrap();
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.message.contains("restarting (retry"))
    });
    assert!(f.editor.current_diagnostics().is_none());
    f.editor.handle_key(Key::Char('i'));
    f.editor.paste("unsaved ");
    escape(&mut f.editor);
    f.wait_log("textDocument/didOpen", 2);
    f.ready();
    f.editor.execute_command("Hover").unwrap();
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Failed)
    });
    assert!(f.editor.current_diagnostics().is_none());
    assert!(f.editor.lsp_summary().contains("Failed"));
    f.editor.document.buffer.insert("more ");
    settle(&mut f.editor);
    assert_eq!(f.log().iter().filter(|m| m["event"] == "start").count(), 2);
    f.editor.execute_command("LspRestart").unwrap();
    f.wait_log("textDocument/didOpen", 3);
    f.ready();
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .contains("unsaved")
    );
    assert_eq!(
        fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
        "bad value\r\n😀 e\u{301}界\r\n"
    );
}

#[test]
fn progress_queue_pressure_keeps_final_failure_and_does_not_restart() {
    let mut f = Fixture::new("lifecycle");
    f.ready();
    f.editor.execute_command("help LSP").unwrap();
    f.progress(
        json!("import"),
        json!({"kind":"begin","title":"Gradle import"}),
    );
    for i in 0..200 {
        f.progress(
            json!("import"),
            json!({"kind":"report","message":format!("Resolving dependency {i}")}),
        );
    }
    f.notify(json!({"method":"window/showMessage","params":{"type":1,"message":"Gradle dependency resolution failed"}}));
    f.progress(json!("import"), json!({"kind":"end"}));
    thread::sleep(Duration::from_millis(300));
    escape(&mut f.editor);
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Degraded && s.status.progress.is_empty())
    });
    assert_eq!(f.log().iter().filter(|m| m["event"] == "start").count(), 1);
    assert!(
        f.editor
            .lsp
            .servers
            .values()
            .next()
            .unwrap()
            .status
            .message
            .contains("dependency resolution failed")
    );
}

#[test]
fn malformed_or_excessive_progress_is_bounded_and_cannot_claim_readiness() {
    let mut f = Fixture::new("lifecycle");
    f.ready();
    f.progress(json!(true), json!({"kind":"begin","title":"invalid token"}));
    for i in 0..33 {
        f.progress(json!(i), json!({"kind":"begin","title":"index".repeat(300),"message":"source".repeat(300),"percentage":101}));
    }
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Degraded)
    });
    let status = &f.editor.lsp.servers.values().next().unwrap().status;
    assert_eq!(status.progress.len(), 32);
    assert!(
        status
            .progress
            .iter()
            .all(|task| task.chars().count() <= 1026 && !task.contains('%'))
    );
    for i in 0..33 {
        f.progress(json!(i), json!({"kind":"end"}));
    }
    until(&mut f.editor, |e| {
        e.lsp.servers.values().any(|s| s.status.progress.is_empty())
    });
    assert_eq!(
        f.editor.lsp.servers.values().next().unwrap().status.phase,
        Phase::Degraded
    );
    f.progress(
        json!("x".repeat(1025)),
        json!({"kind":"begin","title":"oversized token"}),
    );
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.message.contains("1 KiB"))
    });
}

#[test]
fn lsp_status_and_troubleshooting_are_rendered_safely_at_narrow_sizes() {
    let mut f = Fixture::new("lifecycle");
    f.ready();
    f.notify(json!({"method":"window/showMessage","params":{"type":1,"message":"Missing JDK\u{1b}[2J\t界"}}));
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.phase == Phase::Degraded)
    });
    for width in [0, 1, 20, 70, 100] {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
            .unwrap();
        if width >= 70 {
            let cells = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(cells.contains("LSP Degraded :LspInfo"), "{cells}");
        }
    }
    f.editor.execute_command("LspInfo").unwrap();
    for width in [0, 1, 20, 100] {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal
            .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
            .unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .all(|c| !c.symbol().contains('\u{1b}'))
        );
    }
}

#[test]
fn crash_restart_resynchronizes_current_unsaved_content_and_reaps_old_process() {
    let mut f = Fixture::new("crash");
    f.editor.lsp.config.restart_limit = 1;
    f.ready();
    f.editor.document.buffer.insert("unsaved ");
    f.editor.document.buffer.commit();
    f.editor.tick();
    let expected = f.editor.document.buffer.text().to_string();
    f.editor.execute_command("Hover").unwrap();
    f.wait_log("textDocument/didOpen", 2);
    until(&mut f.editor, |e| {
        e.lsp.servers.values().any(|s| s.capabilities.is_object())
    });
    let opens: Vec<_> = f
        .log()
        .into_iter()
        .filter(|v| v["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(
        opens.last().unwrap()["params"]["textDocument"]["text"],
        expected
    );
    let starts: Vec<_> = f
        .log()
        .into_iter()
        .filter(|v| v["event"] == "start")
        .collect();
    assert_eq!(starts.len(), 2);
    #[cfg(target_os = "linux")]
    assert!(!Path::new(&format!("/proc/{}", starts[0]["pid"].as_u64().unwrap())).exists());
    f.editor.execute_command("LspRestart").unwrap();
    f.wait_log("textDocument/didOpen", 3);
    assert_eq!(f.editor.document.buffer.text().to_string(), expected);
    assert!(f.editor.document.buffer.undo());
}

#[test]
fn unsupported_missing_malformed_encoding_and_timeout_fail_without_losing_text() {
    for scenario in [
        "unsupported",
        "malformed",
        "encoding",
        "initialize-timeout",
        "timeout",
    ] {
        let mut f = Fixture::new(scenario);
        if scenario == "initialize-timeout" {
            f.editor.lsp.config.initialize_timeout = Duration::from_millis(150);
        }
        if ["unsupported", "timeout"].contains(&scenario) {
            f.ready();
            let result = f.editor.execute_command("Hover");
            if scenario == "unsupported" {
                assert!(result.is_err());
            } else {
                result.unwrap();
                until(&mut f.editor, |e| e.message_is_error && !e.lsp.pending);
            }
        } else {
            until(&mut f.editor, |e| {
                e.lsp
                    .servers
                    .values()
                    .any(|s| s.status.message.contains("stopped"))
            });
        }
        assert!(
            f.editor
                .document
                .buffer
                .text()
                .to_string()
                .starts_with("bad")
        );
        assert!(!f.editor.document.buffer.is_modified());
    }
    let mut f = Fixture::new("normal");
    f.editor
        .lsp
        .config
        .commands
        .insert(Language::Kotlin, vec!["/missing/editor-pro-lsp".into()]);
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.message.contains("Launch failed"))
    });
    assert_eq!(
        f.editor.lsp.servers.values().next().unwrap().status.phase,
        Phase::Failed
    );
    assert!(f.editor.lsp_summary().contains(":LspInfo"));
    assert!(f.editor.execute_command("Hover").is_err());
}

#[test]
fn unsolicited_server_edits_are_rejected_and_cancelled_preview_does_not_edit() {
    let mut f = Fixture::new("unsolicited");
    f.ready();
    settle(&mut f.editor);
    assert!(
        f.log()
            .iter()
            .any(|m| m["id"] == 999 && m["result"]["applied"] == false)
    );
    f.command("Format");
    escape(&mut f.editor);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("bad")
    );
    assert!(!f.editor.document.buffer.is_modified());
}

#[test]
fn code_action_preview_can_outlive_the_network_request_timeout() {
    let mut f = Fixture::new("normal");
    f.editor.lsp.config.request_timeout = Duration::from_millis(120);
    f.ready();
    f.command("CodeAction");
    f.editor.handle_key(Key::Down);
    f.editor.handle_key(Key::Enter);
    until(&mut f.editor, |e| {
        e.lsp
            .panel
            .as_ref()
            .is_some_and(|p| p.title.starts_with("Apply"))
    });
    settle(&mut f.editor);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    f.editor.handle_key(Key::Enter);
    assert!(!f.editor.message_is_error, "{}", f.editor.message);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("good")
    );
}

#[test]
fn explicit_root_handles_ambiguous_solutions_and_rejects_outside_sources() {
    let mut f = Fixture::new("normal");
    let root = f.directory.path().to_path_buf();
    fs::write(root.join("a.sln"), "").unwrap();
    fs::write(root.join("b.sln"), "").unwrap();
    fs::write(root.join("Main.cs"), "bad csharp\n").unwrap();
    f.editor.lsp.config.root = Some(root.clone());
    let mut command = f.editor.lsp.config.commands[&Language::Kotlin].clone();
    command[3] = root.join("csharp.jsonl").into_os_string();
    f.editor
        .lsp
        .config
        .commands
        .insert(Language::CSharp, command);
    f.editor.open_buffer(&root.join("Main.cs")).unwrap();
    until(&mut f.editor, |e| {
        e.lsp
            .servers
            .values()
            .filter(|s| s.capabilities.is_object())
            .count()
            == 2
    });
    assert_eq!(f.editor.lsp.routes[&root.join("Main.cs")].root, root);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("Other.kt"), "bad other\n").unwrap();
    f.editor
        .open_buffer(&outside.path().join("Other.kt"))
        .unwrap();
    until(&mut f.editor, |e| !e.lsp.unavailable.is_empty());
    assert!(
        f.editor
            .execute_command("Hover")
            .unwrap_err()
            .to_string()
            .contains("outside")
    );
}

#[test]
fn edit_validation_rejects_overlap_versions_resource_ops_and_disk_or_buffer_races() {
    let mut f = Fixture::new("normal");
    let path = f.directory.path().join("Main.kt");
    let root = f.directory.path();
    let guards = f.editor.lsp_edit_guards();
    for value in [
        workspace(&path, vec![text_edit(0, 2, "a"), text_edit(1, 3, "b")]),
        workspace(&path, vec![text_edit(0, 0, "a"), text_edit(0, 0, "b")]),
        workspace(&path, vec![text_edit(0, 999, "a")]),
        json!({"documentChanges":[{"kind":"delete","uri":lsp::file_uri(&path).unwrap()}]}),
        json!({"documentChanges":[{"textDocument":{"uri":lsp::file_uri(&path).unwrap(),"version":42},"edits":[text_edit(0,3,"a")]}]}),
    ] {
        assert!(
            f.editor
                .prepare_lsp_edit(&value, root, &current_versions(&f.editor), &guards)
                .is_err()
        );
    }
    let good = workspace(&path, vec![text_edit(0, 3, "good")]);
    let plan = f
        .editor
        .prepare_lsp_edit(&good, root, &current_versions(&f.editor), &guards)
        .unwrap();
    fs::write(&path, "external\n").unwrap();
    assert!(f.editor.apply_lsp_edit(plan).is_err());
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("bad")
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "external\n");
}

#[test]
fn multi_file_preview_validation_is_all_or_nothing_and_closed_targets_become_dirty_buffers() {
    let mut f = Fixture::new("normal");
    let root = f.directory.path();
    let main = root.join("Main.kt");
    let other = root.join("Other.kt");
    let guards = f.editor.lsp_edit_guards();
    let value = json!({"changes":{
        lsp::file_uri(&main).unwrap().as_str():[text_edit(0,3,"good")],
        lsp::file_uri(&other).unwrap().as_str():[text_edit(0,3,"good")]
    }});
    let plan = f
        .editor
        .prepare_lsp_edit(&value, root, &current_versions(&f.editor), &guards)
        .unwrap();
    f.editor.document.buffer.insert("x");
    assert!(f.editor.apply_lsp_edit(plan).is_err());
    assert_eq!(f.editor.buffer_count(), 1);
    f.editor.document.buffer.undo();
    let guards = f.editor.lsp_edit_guards();
    let plan = f
        .editor
        .prepare_lsp_edit(&value, root, &current_versions(&f.editor), &guards)
        .unwrap();
    f.editor.apply_lsp_edit(plan).unwrap();
    assert_eq!(f.editor.buffer_count(), 2);
    assert_eq!(f.editor.dirty_buffer_count(), 2);
    assert!(fs::read_to_string(other).unwrap().starts_with("bad"));
}

#[test]
fn versioned_utf16_edits_handle_emoji_crlf_and_reject_surrogate_splits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unicode.kt");
    fs::write(&path, "😀e\u{301}界\r\nnext").unwrap();
    let mut editor = Editor::new(Document::open(&path).unwrap());
    let guards = editor.lsp_edit_guards();
    let stamp: Stamp = guards[&path].clone();
    let versions = vec![Versioned {
        snapshot: lsp::Snapshot {
            stamp,
            path: path.clone(),
            text: editor.document.buffer.text().clone(),
            saved: 0,
            language: Language::Kotlin,
            virtual_source: None,
        },
        version: 7,
    }];
    let value = json!({"documentChanges":[{"textDocument":{"uri":lsp::file_uri(&path).unwrap(),"version":7},"edits":[text_edit(2,4,"é")]}]});
    let plan = editor
        .prepare_lsp_edit(&value, dir.path(), &versions, &guards)
        .unwrap();
    editor.apply_lsp_edit(plan).unwrap();
    assert_eq!(editor.document.buffer.text().to_string(), "😀é界\r\nnext");
    let split = workspace(&path, vec![text_edit(1, 2, "bad")]);
    assert!(
        editor
            .prepare_lsp_edit(
                &split,
                dir.path(),
                &current_versions(&editor),
                &editor.lsp_edit_guards()
            )
            .is_err()
    );
}

#[test]
fn shutdown_is_graceful_and_reaps_server_even_with_outstanding_request() {
    let mut f = Fixture::new("slow");
    f.ready();
    f.editor.execute_command("Hover").unwrap();
    let path = f.directory.path().join("protocol.jsonl");
    drop(f.editor);
    let log = fs::read_to_string(path).unwrap();
    assert!(log.contains("\"method\": \"shutdown\""));
    assert!(log.contains("\"method\": \"exit\""));
}

#[test]
fn shutdown_closes_stdin_so_servers_can_finish_their_transport_loop() {
    let mut f = Fixture::new("exit-eof");
    f.ready();
    let directory = f.directory;
    drop(f.editor);
    let log = fs::read_to_string(directory.path().join("protocol.jsonl")).unwrap();
    assert!(
        log.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|v| v["event"] == "stdin-closed")
    );
}

#[test]
fn java_signature_help_is_enabled_in_initialization_and_workspace_settings() {
    let mut f = Fixture::new("java-settings");
    let path = f.directory.path().join("Main.java");
    fs::write(&path, "class Main {}\n").unwrap();
    f.editor.document = Document::open(&path).unwrap();
    let command = f.editor.lsp.config.commands[&Language::Kotlin].clone();
    f.editor.lsp.config.commands.insert(Language::Java, command);
    f.ready();
    f.command("Signature");
    let configuration = f
        .log()
        .into_iter()
        .find(|m| m["id"] == "java-config")
        .unwrap();
    assert_eq!(
        configuration["result"],
        json!([{"signatureHelp":{"enabled":true}},true,null])
    );
    assert!(
        f.editor
            .lsp
            .panel
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.label.contains("greet"))
    );
}

#[test]
fn java_csharp_and_multiple_roots_get_independent_sessions() {
    let mut f = Fixture::new("normal");
    let command = f.editor.lsp.config.commands[&Language::Kotlin].clone();
    for (language, name) in [(Language::Java, "Main.java"), (Language::CSharp, "Main.cs")] {
        let root = f.directory.path().join(language.id());
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(name), "bad value\n").unwrap();
        fs::write(
            root.join(if language == Language::Java {
                "pom.xml"
            } else {
                "app.csproj"
            }),
            "",
        )
        .unwrap();
        let mut command = command.clone();
        command[3] = root.join("protocol.jsonl").into_os_string();
        f.editor.lsp.config.commands.insert(language, command);
        f.editor.open_buffer(&root.join(name)).unwrap();
    }
    f.editor.switch_buffer(1).unwrap();
    until(&mut f.editor, |e| {
        e.lsp.servers.len() == 3 && e.lsp.servers.values().all(|s| s.capabilities.is_object())
    });
    for id in [1, 2, 3] {
        f.editor.switch_buffer(id).unwrap();
        f.command("Hover");
        assert_eq!(f.editor.view, View::Lsp);
        escape(&mut f.editor);
    }
    let java_log = fs::read_to_string(f.directory.path().join("java/protocol.jsonl")).unwrap();
    assert!(java_log.contains("\"languageId\": \"java\""));
    let csharp_log = fs::read_to_string(f.directory.path().join("csharp/protocol.jsonl")).unwrap();
    assert!(csharp_log.contains("\"languageId\": \"csharp\""));
}

#[test]
fn closing_last_document_sends_didclose_and_save_as_keeps_session_usable() {
    let mut f = Fixture::new("normal");
    f.ready();
    let renamed = f.directory.path().join("Moved.kt");
    f.editor
        .execute_command(&format!("w {}", renamed.display()))
        .unwrap();
    f.wait_log("textDocument/didOpen", 2);
    f.command("Hover");
    escape(&mut f.editor);
    f.editor.execute_command("bd").unwrap();
    f.wait_log("textDocument/didClose", 2);
    assert!(f.editor.lsp.servers.is_empty());
}

#[test]
fn slow_import_does_not_block_edits_and_pull_diagnostics_follow_latest_changes() {
    let mut f = Fixture::new("initialize-timeout");
    f.editor.tick();
    for _ in 0..20 {
        f.editor.handle_key(Key::Char('i'));
        f.editor.handle_key(Key::Char('x'));
        escape(&mut f.editor);
        f.editor.tick();
    }
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with(&"x".repeat(20))
    );
    drop(f);
    let mut f = Fixture::new("pull");
    f.ready();
    until(&mut f.editor, |e| e.current_diagnostics().is_some());
    f.editor.document.buffer.replace_range(0..3, "good");
    f.editor.execute_command("Hover").unwrap();
    until(&mut f.editor, |e| {
        e.current_diagnostics().is_some_and(|d| d.items.is_empty())
    });
}

#[test]
fn review_note_never_replaces_source_in_lsp_synchronization() {
    let mut f = Fixture::new("normal");
    for arguments in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["add", "Main.kt"],
        vec!["commit", "-m", "baseline"],
    ] {
        let out = std::process::Command::new("git")
            .current_dir(f.directory.path())
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    f.ready();
    f.editor.execute_command("diff").unwrap();
    until(&mut f.editor, |e| {
        e.review_diff().is_some() && e.review.store.is_some()
    });
    f.editor.execute_command("note file").unwrap();
    f.editor.handle_key(Key::Char('i'));
    f.editor.paste("PRIVATE NOTE TEXT");
    settle(&mut f.editor);
    assert!(
        !f.log()
            .iter()
            .any(|v| v.to_string().contains("PRIVATE NOTE"))
    );
    assert!(f.editor.execute_command("Hover").is_err());
    escape(&mut f.editor);
    f.editor.execute_command("note cancel").unwrap();
    escape(&mut f.editor);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("bad")
    );
    f.command("Hover");
}
