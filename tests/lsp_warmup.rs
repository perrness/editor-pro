use editor_pro::{editor::Editor, file::Document, input::Key, lsp::Language, ui};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

struct Project {
    editor: Editor,
    directory: tempfile::TempDir,
}

impl Project {
    fn new(scenario: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(".git"), "").unwrap();
        fs::write(
            directory.path().join("pom.xml"),
            "<artifactId>kotlin-maven-plugin</artifactId>",
        )
        .unwrap();
        fs::write(
            directory.path().join("Main.kt"),
            "bad value\r\n😀 e\u{301}界\r\n",
        )
        .unwrap();
        let mut editor = Editor::new(Document::default());
        editor.lsp.config.initialize_timeout = Duration::from_secs(5);
        editor.lsp.config.restart_limit = 0;
        for language in [Language::Kotlin, Language::Java, Language::CSharp] {
            editor.lsp.config.commands.insert(
                language,
                vec![
                    "python3".into(),
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tools/lsp/fake_server.py")
                        .into_os_string(),
                    "--log".into(),
                    directory
                        .path()
                        .join(format!("{}.jsonl", language.id()))
                        .into_os_string(),
                    "--scenario".into(),
                    scenario.into(),
                ],
            );
        }
        Self { editor, directory }
    }
    fn root(&self) -> PathBuf {
        self.directory.path().canonicalize().unwrap()
    }
    fn prepare(&mut self) {
        self.editor.prepare_lsp(Some(self.root())).unwrap();
    }
    fn log(&self, language: Language) -> Vec<Value> {
        log(&self.root().join(format!("{}.jsonl", language.id())))
    }
    fn ready(&mut self) {
        until(&mut self.editor, |e| {
            e.lsp.servers.values().any(|s| s.capabilities.is_object())
        });
    }
    fn open(&mut self) {
        self.editor
            .open_buffer(&self.root().join("Main.kt"))
            .unwrap();
    }
}

fn log(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn until(editor: &mut Editor, condition: impl Fn(&Editor) -> bool) {
    let start = Instant::now();
    loop {
        editor.tick();
        if condition(editor) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Timed out: {:?}",
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

#[test]
fn warm_session_has_no_document_and_reuses_initialization_with_latest_unsaved_text() {
    let mut p = Project::new("initialize-held");
    let root = p.root();
    p.prepare();
    until(&mut p.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.contains("fake import"))
    });
    assert!(
        !p.log(Language::Kotlin)
            .iter()
            .any(|m| m["method"] == "textDocument/didOpen")
    );
    assert!(p.editor.lsp_summary().contains("Indexing"));
    p.editor.execute_command("LspInfo").unwrap();
    assert!(
        p.editor
            .lsp
            .panel
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.label.contains("Prepared kotlin"))
    );
    p.editor.handle_key(Key::Escape);

    // Initialization cannot finish until this test releases it. Editing and
    // rendering must still finish, including narrow terminals and Unicode.
    p.open();
    p.editor.handle_key(Key::Char('i'));
    p.editor.paste("unsaved 😀\r\n");
    let expected = p.editor.document.buffer.text().to_string();
    let mut terminal = Terminal::new(TestBackend::new(24, 6)).unwrap();
    let theme = p.editor.theme.palette();
    for _ in 0..10 {
        p.editor.tick();
        terminal
            .draw(|frame| ui::render(frame, &mut p.editor, &theme))
            .unwrap();
    }
    fs::write(root.join("kotlin.jsonl.initialize"), "ready").unwrap();
    p.ready();
    until(&mut p.editor, |e| e.current_diagnostics().is_some());
    let messages = p.log(Language::Kotlin);
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["method"] == "initialize")
            .count(),
        1
    );
    let opened: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0]["params"]["textDocument"]["text"], expected);
    assert!(p.log(Language::Java).is_empty());
    assert!(p.log(Language::CSharp).is_empty());
    let epoch = p.editor.lsp.servers.values().next().unwrap().epoch;
    p.editor.prepare_lsp(None).unwrap();
    p.editor.handle_key(Key::Escape);
    p.editor.execute_command("Hover").unwrap();
    until(&mut p.editor, |e| !e.lsp.pending);
    assert!(!p.editor.message_is_error, "{}", p.editor.message);
    assert_eq!(p.editor.lsp.servers.values().next().unwrap().epoch, epoch);
    assert_eq!(
        fs::read_to_string(root.join("Main.kt")).unwrap(),
        "bad value\r\n😀 e\u{301}界\r\n"
    );
}

#[test]
fn cancelling_during_initialize_reaps_unused_server_and_keeps_editing_responsive() {
    let mut p = Project::new("initialize-timeout");
    p.prepare();
    let log_path = p.root().join("kotlin.jsonl");
    until(&mut p.editor, |_| {
        log(&log_path).iter().any(|m| m["method"] == "initialize")
    });
    p.editor.prepare_lsp(None).unwrap();
    p.editor.handle_key(Key::Char('i'));
    p.editor.paste("still editable");
    until(&mut p.editor, |e| {
        !e.lsp.discovering && e.lsp.servers.is_empty()
    });
    assert_eq!(
        p.editor.document.buffer.text().to_string(),
        "still editable"
    );
    assert!(
        p.log(Language::Kotlin)
            .iter()
            .any(|m| m["method"] == "shutdown")
    );
    // Opening later follows the normal demand path, not a stale cancelled job.
    p.open();
    until(&mut p.editor, |_| {
        log(&log_path)
            .iter()
            .filter(|m| m["method"] == "initialize")
            .count()
            == 2
    });
}

#[test]
fn replacing_preparation_closes_old_session_and_shutdown_closes_documentless_server() {
    let mut p = Project::new("normal");
    p.prepare();
    p.ready();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join(".git"), "").unwrap();
    fs::write(other.path().join("Project.csproj"), "").unwrap();
    p.editor.prepare_lsp(Some(other.path().into())).unwrap();
    until(&mut p.editor, |e| {
        e.lsp.servers.len() == 1
            && e.lsp.servers.iter().all(|(key, info)| {
                key.language == Language::CSharp && info.capabilities.is_object()
            })
    });
    assert!(
        p.log(Language::Kotlin)
            .iter()
            .any(|m| m["method"] == "shutdown")
    );
    let root = p.root();
    drop(p.editor);
    let messages = log(&root.join("csharp.jsonl"));
    assert!(messages.iter().any(|m| m["method"] == "shutdown"));
    assert!(messages.iter().any(|m| m["method"] == "exit"));
    assert!(
        !messages
            .iter()
            .any(|m| m["method"] == "textDocument/didOpen")
    );
}

#[test]
fn missing_warm_server_remains_quiet_and_opening_it_does_not_retry_in_a_loop() {
    let mut p = Project::new("normal");
    p.editor.lsp.config.commands.insert(
        Language::Kotlin,
        vec![p.root().join("missing-server").into_os_string()],
    );
    p.prepare();
    until(&mut p.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.contains("Launch failed"))
    });
    assert!(!p.editor.message_is_error);
    let epoch = p.editor.lsp.servers.values().next().unwrap().epoch;
    p.open();
    until(&mut p.editor, |e| !e.lsp.routes.is_empty());
    assert_eq!(p.editor.lsp.servers.values().next().unwrap().epoch, epoch);
    assert!(p.editor.execute_command("Hover").is_err());
    p.editor.execute_command("LspRestart").unwrap();
    until(&mut p.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.epoch != epoch && s.status.contains("Launch failed"))
    });
}

#[test]
fn warm_session_crash_recovers_with_current_unsaved_document() {
    let mut p = Project::new("crash");
    p.editor.lsp.config.restart_limit = 1;
    p.prepare();
    p.ready();
    p.open();
    p.editor.document.buffer.insert("unsaved ");
    until(&mut p.editor, |e| e.current_diagnostics().is_some());
    let epoch = p.editor.lsp.servers.values().next().unwrap().epoch;
    p.editor.execute_command("Hover").unwrap();
    until(&mut p.editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.epoch != epoch && s.capabilities.is_object())
            && e.current_diagnostics().is_some()
    });
    let messages = p.log(Language::Kotlin);
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["method"] == "initialize")
            .count(),
        2
    );
    for opened in messages
        .iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
    {
        assert!(
            opened["params"]["textDocument"]["text"]
                .as_str()
                .unwrap()
                .starts_with("unsaved ")
        );
    }
}

#[test]
fn open_documents_evict_unused_warm_sessions_at_the_existing_limit() {
    let mut p = Project::new("normal");
    fs::write(p.root().join("Main.java"), "class Main {}").unwrap();
    p.prepare();
    until(&mut p.editor, |e| {
        e.lsp.servers.len() == 2 && e.lsp.servers.values().all(|s| s.capabilities.is_object())
    });
    for index in 0..8 {
        let root = p.root().join(format!("project-{index}"));
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".git"), "").unwrap();
        fs::write(root.join("Project.csproj"), "").unwrap();
        fs::write(root.join("Main.cs"), "class Main {}\n").unwrap();
        p.editor.open_buffer(&root.join("Main.cs")).unwrap();
    }
    until(&mut p.editor, |e| {
        e.lsp.servers.len() == 8
            && e.lsp.servers.iter().all(|(key, info)| {
                key.language == Language::CSharp && info.capabilities.is_object()
            })
    });
    assert!(p.editor.lsp.unavailable.is_empty());
    assert!(
        p.log(Language::Kotlin)
            .iter()
            .any(|m| m["method"] == "shutdown")
    );
    assert!(
        p.log(Language::Java)
            .iter()
            .any(|m| m["method"] == "shutdown")
    );
}

#[test]
fn opening_during_detection_and_repeated_preparation_never_duplicate_a_session() {
    let mut p = Project::new("normal");
    p.prepare();
    p.open();
    p.ready();
    p.prepare();
    until(&mut p.editor, |e| {
        !e.lsp.discovering && e.current_diagnostics().is_some()
    });
    assert_eq!(
        p.log(Language::Kotlin)
            .iter()
            .filter(|m| m["method"] == "initialize")
            .count(),
        1
    );
    p.editor.execute_command("bd!").unwrap();
    let log_path = p.root().join("kotlin.jsonl");
    until(&mut p.editor, |e| {
        e.lsp.servers.is_empty()
            && log(&log_path)
                .iter()
                .any(|m| m["method"] == "textDocument/didClose")
    });
    assert!(
        p.log(Language::Kotlin)
            .iter()
            .any(|m| m["method"] == "textDocument/didClose")
    );
}
