//! Opt-in actual Codex CLI acceptance; protocol doubles are separate evidence.
use editor_pro::{ai::Config, editor::Editor, file::Document, input::Key};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn finish(editor: &mut Editor) {
    let started = Instant::now();
    let mut status = String::new();
    while editor.ai.pending {
        editor.tick();
        if status != editor.ai.status {
            status.clone_from(&editor.ai.status);
            eprintln!("AI status after {:?}: {status}", started.elapsed());
        }
        assert!(
            started.elapsed() < Duration::from_secs(200),
            "{}",
            editor.message
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!editor.message_is_error, "{}", editor.message);
}

#[test]
#[ignore = "Requires EDITOR_PRO_REAL_CODEX, authenticated Codex CLI and provider network access"]
fn real_codex_context_revisions_and_persistent_followup() {
    let executable = PathBuf::from(
        std::env::var_os("EDITOR_PRO_REAL_CODEX")
            .expect("Set EDITOR_PRO_REAL_CODEX to the installed Codex executable"),
    );
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"ai-fixture\"\n",
    )
    .unwrap();
    let source = root.join("greeting.txt");
    let original = "fn greeting() -> &'static str { \"Hello World\" }\r\n";
    fs::write(&source, original).unwrap();
    let mut editor = Editor::new(Document::open(&source).unwrap());
    editor.ai.config = Config {
        executable: executable.clone(),
        ..Config::default()
    };
    editor.document.buffer.insert("// unsaved fixture\r\n");
    editor.document.buffer.commit();
    let unsaved = editor.document.buffer.text().to_string();
    editor.execute_command("ai context file").unwrap();
    editor.execute_command("ai ask Explain the greeting with one source citation. Suggest exactly one edit replacing World with Reader. Do not change anything else. Distinguish inferred intent from what the code says.").unwrap();
    editor.execute_command("ai send").unwrap();
    finish(&mut editor);
    let answer = editor.ai.conversation.as_ref().unwrap().turns[0]
        .answer
        .as_ref()
        .unwrap();
    assert!(answer.observations.iter().any(|o| !o.citations.is_empty()));
    assert!(!answer.summary.is_empty());
    assert_eq!(answer.edits.len(), 1);
    editor.execute_command("ai edits").unwrap();
    editor.execute_command("ai apply").unwrap();
    assert_eq!(
        editor.document.buffer.text().to_string(),
        unsaved.replace("World", "Reader")
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), original);
    editor.handle_key(Key::Escape);
    editor.handle_key(Key::Char('u'));
    assert_eq!(editor.document.buffer.text().to_string(), unsaved);
    let saved = dir.path().join("conversation.json");
    editor
        .execute_command(&format!("ai save {}", saved.display()))
        .unwrap();
    finish(&mut editor);
    drop(editor);
    let mut editor = Editor::new(Document::open(&source).unwrap());
    editor.ai.config = Config {
        executable,
        ..Config::default()
    };
    editor
        .execute_command(&format!("ai load {}", saved.display()))
        .unwrap();
    finish(&mut editor);
    assert!(editor.prepare_ai_edit().is_err());
    editor.execute_command("ai context file").unwrap();
    editor.execute_command("ai ask What replacement name did we discuss previously? Explain without proposing any edits.").unwrap();
    editor.execute_command("ai send").unwrap();
    finish(&mut editor);
    let answer = editor.ai.conversation.as_ref().unwrap().turns[1]
        .answer
        .as_ref()
        .unwrap();
    let text = serde_json::to_string(answer).unwrap();
    assert!(text.contains("Reader"), "{text}");
    assert!(answer.edits.is_empty());
    assert_eq!(fs::read_to_string(source).unwrap(), original);
}
