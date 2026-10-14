use editor_pro::{
    editor::{Editor, Mode, View},
    file::Document,
    input::Key,
    lsp::{self, Language, Stamp, Versioned},
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
        },
        version: 1,
    }]
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
        f.editor.handle_key(Key::Enter);
        assert_eq!(
            f.editor.document.path().unwrap().file_name().unwrap(),
            "Other.kt"
        );
        assert_eq!(f.editor.document.buffer.cursor(), 0);
        f.editor.execute_command("b 1").unwrap();
    }
}

#[test]
fn completion_resolves_documentation_and_applies_undoable_preview_in_insert() {
    let mut f = Fixture::new("normal");
    f.ready();
    f.editor.handle_key(Key::Char('i'));
    f.editor.document.buffer.set_cursor(3, true);
    f.editor.handle_key(Key::Ctrl('n'));
    until(&mut f.editor, |e| e.view == View::Lsp && !e.lsp.pending);
    f.editor.handle_key(Key::Char('d'));
    until(&mut f.editor, |e| !e.lsp.pending);
    assert!(
        f.editor.lsp.panel.as_ref().unwrap().rows[0]
            .detail
            .contains("Resolved")
    );
    f.editor.handle_key(Key::Enter);
    assert!(f.editor.lsp.panel.as_ref().unwrap().title.contains("Apply"));
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
    assert_eq!(f.editor.mode, Mode::Insert);
    assert!(
        f.editor
            .document
            .buffer
            .text()
            .to_string()
            .starts_with("good")
    );
    assert_eq!(f.editor.document.buffer.cursor(), 4);
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

#[test]
fn completion_commands_require_selection_approval_and_a_separate_server_edit_preview() {
    let mut f = Fixture::new("completion-command");
    f.ready();
    let before = f.editor.document.buffer.text().to_string();
    for apply_command in [false, true] {
        f.command("Complete");
        f.editor.handle_key(Key::Enter);
        until(&mut f.editor, |e| {
            e.lsp
                .panel
                .as_ref()
                .is_some_and(|p| p.title.starts_with("Apply"))
        });
        assert!(
            f.editor
                .lsp
                .panel
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.label.contains("Then run server command: fake.fix"))
        );
        assert!(
            !f.log()
                .iter()
                .any(|m| m["method"] == "workspace/executeCommand")
        );
        if !apply_command {
            escape(&mut f.editor);
            continue;
        }
        f.editor.handle_key(Key::Enter);
        until(&mut f.editor, |e| {
            e.lsp
                .panel
                .as_ref()
                .is_some_and(|p| p.title.starts_with("Apply"))
        });
        assert_eq!(f.editor.document.buffer.text().to_string(), before);
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
        assert!(f.editor.document.buffer.undo());
        assert_eq!(f.editor.document.buffer.text().to_string(), before);
    }
    assert_eq!(
        fs::read_to_string(f.directory.path().join("Main.kt")).unwrap(),
        before
    );
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
    f.editor.handle_key(Key::Enter);
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
        f.editor.lsp.config.initialize_timeout = Duration::from_millis(150);
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
                e.lsp.servers.values().any(|s| s.status.contains("stopped"))
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
            .any(|s| s.status.contains("Launch failed"))
    });
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
