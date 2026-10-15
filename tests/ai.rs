use editor_pro::{
    ai::{self, Answer, Context, Draft, Replacement, Status},
    ai_store,
    editor::{Editor, Mode, View},
    file::Document,
    input::Key,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    dir: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    editor: Editor,
}
impl Fixture {
    fn new(text: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        let source = root.join("source.txt");
        fs::write(&source, text).unwrap();
        let mut editor = Editor::new(Document::open(&source).unwrap());
        editor.ai.config.executable =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/ai/fake_codex.py");
        editor.ai.config.model = Some("edit".into());
        Self {
            dir,
            root,
            source,
            editor,
        }
    }
    fn request(&mut self, variant: &str) {
        self.editor.ai.config.model = Some(variant.into());
        self.editor.execute_command("ai context file").unwrap();
        self.editor
            .execute_command("ai ask Explain and suggest the requested revision")
            .unwrap();
        self.editor.execute_command("ai send").unwrap();
    }
    fn finish(&mut self) {
        wait(&mut self.editor, |e| !e.ai.pending);
    }
    fn text(&self) -> String {
        self.editor.document.buffer.text().to_string()
    }
}
fn wait(e: &mut Editor, done: impl Fn(&Editor) -> bool) {
    let start = Instant::now();
    loop {
        e.tick();
        if done(e) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "AI timed out: {}",
            e.ai.status
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn answer(e: &Editor) -> &Answer {
    e.ai.conversation
        .as_ref()
        .unwrap()
        .turns
        .last()
        .unwrap()
        .answer
        .as_ref()
        .unwrap()
}

#[test]
fn context_preview_is_local_and_preserves_unsaved_unicode_source_and_viewport() {
    let mut f = Fixture::new("old\r\n界👩‍💻e\u{301}\r\n");
    f.editor.document.buffer.insert("local ");
    f.editor.document.buffer.commit();
    f.editor.document.buffer.goto_line(1, false);
    f.editor.viewport.top = 1;
    f.editor.viewport.left = 2;
    let cursor = f.editor.document.buffer.cursor();
    let viewport = f.editor.viewport.clone();
    f.editor.ai.config.executable = "/missing/codex".into();
    f.editor.execute_command("ai context file").unwrap();
    assert!(!f.editor.ai.pending);
    assert_eq!(f.editor.ai.draft.context[0].text, f.text());
    assert!(
        f.editor.ai.draft.context[0]
            .source
            .as_ref()
            .unwrap()
            .unsaved
    );
    assert!(f.editor.execute_command("w").is_err());
    for (width, height) in [(1, 1), (2, 2), (18, 5), (100, 24)] {
        Terminal::new(TestBackend::new(width, height))
            .unwrap()
            .draw(|frame| ui::render(frame, &mut f.editor, &Theme::night()))
            .unwrap();
    }
    assert_eq!(f.editor.viewport, viewport);
    assert_eq!(f.editor.document.buffer.cursor(), cursor);
    f.editor.handle_key(Key::Escape);
    assert_eq!(f.editor.view, View::Editing);
    assert!(f.text().starts_with("local "));
    assert_eq!(
        fs::read_to_string(&f.source).unwrap(),
        "old\r\n界👩‍💻e\u{301}\r\n"
    );
}

#[test]
fn visual_character_and_line_context_preserve_graphemes_and_crlf() {
    let mut f = Fixture::new("界👩‍💻e\u{301}\r\nsecond\r\n");
    f.editor.handle_key(Key::Char('v'));
    f.editor.handle_key(Key::Char('l'));
    f.editor.handle_key(Key::Ctrl('g'));
    assert_eq!(f.editor.ai.draft.context[0].text, "界👩‍💻");
    f.editor.handle_key(Key::Escape);
    f.editor.document.buffer.set_cursor(0, false);
    f.editor.handle_key(Key::Char('V'));
    f.editor.handle_key(Key::Ctrl('g'));
    assert_eq!(f.editor.ai.draft.context[0].text, "界👩‍💻e\u{301}\r\n");
    assert_eq!(f.editor.mode, Mode::Normal);
}

#[test]
fn context_and_prompt_limits_refuse_without_truncating_prior_draft() {
    let mut f = Fixture::new("old");
    f.editor.execute_command("ai context file").unwrap();
    let file = f.root.join("huge.txt");
    fs::write(&file, "x".repeat(ai::MAX_ATTACHMENT + 1)).unwrap();
    assert!(
        f.editor
            .execute_command(&format!("ai add tests {}", file.display()))
            .is_err()
    );
    assert_eq!(f.editor.ai.draft.context.len(), 1);
    assert!(
        f.editor
            .execute_command(&format!("ai ask {}", "x".repeat(8193)))
            .is_err()
    );
    assert!(f.editor.execute_command("ai send").is_err());
    assert!(f.editor.execute_command("ai context symbol").is_err());
    assert!(f.editor.execute_command("ai add references").is_err());
    assert!(f.editor.execute_command("ai add diagnostics").is_err());
    assert!(f.editor.execute_command("ai context diff").is_err());
}

#[test]
fn codex_answer_requires_preview_approval_and_changes_only_undoable_buffers() {
    let mut f = Fixture::new("old 界👩‍💻\r\n");
    f.editor.document.buffer.insert("local ");
    f.editor.document.buffer.commit();
    f.request("edit");
    f.finish();
    assert_eq!(answer(&f.editor).edits.len(), 1);
    assert_eq!(f.text(), "local old 界👩‍💻\r\n");
    assert!(f.editor.execute_command("ai apply").is_err());
    f.editor.execute_command("ai edits").unwrap();
    assert!(f.editor.ai.rows.iter().any(|r| r.text == "- old"));
    assert!(f.editor.ai.rows.iter().any(|r| r.text == "+ new"));
    f.editor.execute_command("ai apply").unwrap();
    assert_eq!(f.text(), "local new 界👩‍💻\r\n");
    assert_eq!(fs::read_to_string(&f.source).unwrap(), "old 界👩‍💻\r\n");
    f.editor.handle_key(Key::Escape);
    f.editor.handle_key(Key::Char('u'));
    assert_eq!(f.text(), "local old 界👩‍💻\r\n");
    f.editor.handle_key(Key::Char('u'));
    assert_eq!(f.text(), "old 界👩‍💻\r\n");
}

#[test]
fn changed_reloaded_closed_and_externally_modified_sources_reject_edits() {
    for change in ["edit", "reload", "close", "disk"] {
        let mut f = Fixture::new("old\n");
        f.request("slow");
        f.editor.handle_key(Key::Escape);
        match change {
            "edit" => {
                f.editor.document.buffer.insert("local");
                f.editor.document.buffer.commit();
            }
            "reload" => f.editor.execute_command("e!").unwrap(),
            "close" => f.editor.execute_command("bd").unwrap(),
            "disk" => {
                fs::write(&f.source, "external\n").unwrap();
            }
            _ => unreachable!(),
        }
        f.finish();
        f.editor.execute_command("ai").unwrap();
        if change == "disk" {
            f.editor.execute_command("ai edits").unwrap();
            assert!(f.editor.execute_command("ai apply").is_err());
            assert_eq!(f.text(), "old\n");
        } else {
            assert!(f.editor.execute_command("ai edits").is_err(), "{change}");
        }
    }
}

#[test]
fn preview_is_rechecked_before_send_and_before_apply() {
    let mut f = Fixture::new("old\n");
    f.editor.execute_command("ai context file").unwrap();
    f.editor.execute_command("ai ask Revise").unwrap();
    f.editor.document.buffer.insert("local");
    f.editor.document.buffer.commit();
    assert!(f.editor.execute_command("ai send").is_err());
    assert!(!f.editor.ai.pending);
    f.editor.document.buffer.undo();
    f.request("edit");
    f.finish();
    f.editor.execute_command("ai edits").unwrap();
    f.editor.document.buffer.insert("later");
    f.editor.document.buffer.commit();
    assert!(f.editor.execute_command("ai apply").is_err());
    assert!(f.text().starts_with("later"));
}

#[test]
fn multi_file_edits_validate_every_target_before_any_splice_and_preserve_undo() {
    let mut f = Fixture::new("old\r\n");
    let second = f.root.join("second.txt");
    fs::write(&second, "second\r\n").unwrap();
    f.editor.execute_command("ai context file").unwrap();
    f.editor
        .execute_command(&format!("ai add tests {}", second.display()))
        .unwrap();
    f.editor.ai.config.model = Some("multi".into());
    f.editor.execute_command("ai ask Revise both").unwrap();
    f.editor.execute_command("ai send").unwrap();
    f.finish();
    f.editor.execute_command("ai edits").unwrap();
    fs::write(&second, "external\r\n").unwrap();
    assert!(f.editor.execute_command("ai apply").is_err());
    assert_eq!(f.text(), "old\r\n");
    assert_eq!(f.editor.buffer_count(), 1);
    fs::write(&second, "second\r\n").unwrap();
    f.editor.execute_command("ai edits").unwrap();
    f.editor.execute_command("ai apply").unwrap();
    assert_eq!(f.text(), "new\r\n");
    assert_eq!(f.editor.buffer_count(), 2);
    f.editor.open_buffer(&second).unwrap();
    assert_eq!(f.text(), "revised\r\n");
    f.editor.handle_key(Key::Char('u'));
    assert_eq!(f.text(), "second\r\n");
    assert_eq!(fs::read_to_string(&second).unwrap(), "second\r\n");
}

#[test]
fn unsupported_output_errors_timeouts_and_missing_cli_leave_core_editable() {
    for variant in [
        "malformed",
        "oversize",
        "error",
        "tools",
        "invalid",
        "nonzero",
        "no-completion",
        "update-only",
        "hang",
        "missing",
    ] {
        let mut f = Fixture::new("old\n");
        f.editor.ai.config.timeout = Duration::from_millis(200);
        if variant == "missing" {
            f.editor.ai.config.executable = "/missing/codex".into();
        }
        f.request(variant);
        f.finish();
        assert!(f.editor.message_is_error, "{variant}: {}", f.editor.message);
        assert_eq!(
            f.editor.ai.conversation.as_ref().unwrap().turns[0].status,
            Status::Failed
        );
        assert!(f.editor.prepare_ai_edit().is_err());
        f.editor.handle_key(Key::Escape);
        f.editor.handle_key(Key::Char('i'));
        f.editor.paste("local");
        f.editor.handle_key(Key::Escape);
        assert!(f.text().starts_with("local"));
        assert_eq!(fs::read_to_string(&f.source).unwrap(), "old\n");
    }
}

#[test]
fn partial_updates_are_visible_but_never_actionable_until_successful_completion() {
    let mut f = Fixture::new("old\n");
    f.request("partial");
    wait(&mut f.editor, |e| {
        e.ai.rows
            .iter()
            .any(|r| r.text.contains("Captured text received"))
    });
    assert!(f.editor.ai.pending);
    assert!(f.editor.prepare_ai_edit().is_err());
    f.finish();
    assert_eq!(answer(&f.editor).summary, "Captured text received");
}

#[test]
fn cancellation_suppresses_late_results_and_allows_an_immediate_followup() {
    let mut f = Fixture::new("old\n");
    let pid_path = f.dir.path().join("pid");
    f.request(&format!("hang:{}", pid_path.display()));
    wait(&mut f.editor, |_| pid_path.exists());
    let start = Instant::now();
    f.editor.execute_command("ai cancel").unwrap();
    assert!(start.elapsed() < Duration::from_millis(100));
    f.request("edit");
    f.finish();
    let turns = &f.editor.ai.conversation.as_ref().unwrap().turns;
    assert_eq!(turns[0].status, Status::Cancelled);
    assert_eq!(turns[1].status, Status::Completed);
    #[cfg(target_os = "linux")]
    assert!(!Path::new(&format!("/proc/{}", fs::read_to_string(pid_path).unwrap())).exists());
}

#[test]
fn source_links_validate_captured_content_and_keep_location_history() {
    let mut f = Fixture::new("old\nsecond\nthird\n");
    f.editor.document.buffer.goto_line(2, false);
    f.editor.viewport.top = 2;
    let cursor = f.editor.document.buffer.cursor();
    let viewport = f.editor.viewport.clone();
    f.request("normal");
    f.finish();
    f.editor.ai.selected = f
        .editor
        .ai
        .rows
        .iter()
        .position(|r| r.text.starts_with("  → context"))
        .unwrap();
    f.editor.handle_key(Key::Enter);
    assert_eq!(f.editor.view, View::Editing);
    assert_eq!(f.editor.document.buffer.cursor(), 0);
    f.editor.execute_command("back").unwrap();
    assert_eq!(f.editor.document.buffer.cursor(), cursor);
    assert_eq!(f.editor.viewport, viewport);
    f.editor.document.buffer.insert("changed");
    f.editor.execute_command("ai").unwrap();
    f.editor.ai.selected = f
        .editor
        .ai
        .rows
        .iter()
        .position(|r| r.text.starts_with("  → context"))
        .unwrap();
    f.editor.handle_key(Key::Enter);
    assert!(f.editor.message_is_error);
    assert_eq!(f.editor.view, View::Ai);
}

#[test]
fn saved_conversations_resume_context_but_never_restore_edit_authority() {
    let mut f = Fixture::new("old\n");
    f.request("edit");
    f.finish();
    assert!(f.editor.execute_command("q").is_err());
    assert!(f.editor.execute_command("ai new").is_err());
    let saved = f.dir.path().join("conversation.json");
    f.editor
        .execute_command(&format!("ai save {}", saved.display()))
        .unwrap();
    f.finish();
    assert!(!f.editor.ai.dirty);
    f.editor.execute_command("ai new").unwrap();
    f.editor
        .execute_command(&format!("ai load {}", saved.display()))
        .unwrap();
    f.finish();
    assert!(f.editor.prepare_ai_edit().is_err());
    assert!(
        f.editor
            .ai
            .rows
            .iter()
            .any(|r| r.text.contains("Captured text received"))
    );
    f.request("followup");
    f.finish();
    assert_eq!(f.editor.ai.conversation.as_ref().unwrap().turns.len(), 2);
    assert_eq!(answer(&f.editor).summary, "Captured text received");
    f.editor.execute_command("ai save").unwrap();
    f.finish();
    assert_eq!(ai_store::load(&saved).unwrap().0.turns.len(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn transcript_save_conflicts_corruption_and_source_destinations_never_clobber_data() {
    let mut f = Fixture::new("old\n");
    f.request("normal");
    f.finish();
    let conversation = f.editor.ai.conversation.as_ref().unwrap();
    assert!(ai_store::save(conversation, &f.source, None, &[]).is_err());
    let saved = f.dir.path().join("conversation.json");
    let (_, hash) = ai_store::save(conversation, &saved, None, &[]).unwrap();
    assert!(ai_store::save(conversation, &saved, None, &[]).is_err());
    fs::write(&saved, "corrupted").unwrap();
    assert!(ai_store::save(conversation, &saved, Some(&hash), &[]).is_err());
    assert!(ai_store::load(&saved).is_err());
    assert_eq!(fs::read_to_string(&saved).unwrap(), "corrupted");
    f.editor
        .execute_command(&format!("ai load! {}", saved.display()))
        .unwrap();
    f.finish();
    assert!(f.editor.message_is_error);
    assert_eq!(answer(&f.editor).summary, "Captured text received");
}

#[test]
fn structured_replies_refuse_invented_citations_ambiguous_edits_and_extra_fields() {
    let draft = Draft {
        question: "Explain".into(),
        context: vec![Context {
            label: "text".into(),
            text: "old old".into(),
            source: None,
        }],
    };
    for text in [
        r#"{"summary":"x","observations":[{"text":"x","citations":[{"context":2,"line":1}]}],"inferences":[],"edits":[]}"#,
        r#"{"summary":"x","observations":[{"text":"x","citations":[{"context":1,"line":2}]}],"inferences":[],"edits":[]}"#,
        r#"{"summary":"x","observations":[],"inferences":[],"edits":[],"command":"anything"}"#,
        r#"{"summary":"x","observations":[],"inferences":[],"edits":[{"context":1,"before":"old","after":"new"}]}"#,
    ] {
        assert!(Answer::parse(text, &draft).is_err());
    }
}

#[test]
fn suggestions_cannot_split_graphemes_crlf_or_overlap() {
    let mut f = Fixture::new("old e\u{301} 👩‍💻\r\n");
    f.request("normal");
    f.finish();
    for (before, after) in [("e", "a"), ("👩", "X"), ("\r", ""), ("", "insert")] {
        f.editor.ai.conversation.as_mut().unwrap().turns[0]
            .answer
            .as_mut()
            .unwrap()
            .edits = vec![Replacement {
            context: 1,
            before: before.into(),
            after: after.into(),
        }];
        assert!(f.editor.prepare_ai_edit().is_err(), "{before:?}");
    }
    let edit = Replacement {
        context: 1,
        before: "old".into(),
        after: "new".into(),
    };
    f.editor.ai.conversation.as_mut().unwrap().turns[0]
        .answer
        .as_mut()
        .unwrap()
        .edits = vec![edit.clone(), edit];
    assert!(f.editor.prepare_ai_edit().is_err());
}

#[test]
fn hunk_diff_and_exported_review_feedback_are_explicit_context_without_source_mutation() {
    let mut f = Fixture::new("base\n");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&f.root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "AI Fixture"]);
    git(&["config", "user.email", "ai@example.invalid"]);
    git(&["add", "."]);
    git(&["-c", "commit.gpgsign=false", "commit", "-m", "base"]);
    fs::write(&f.source, "old revision\n").unwrap();
    f.editor.execute_command("e!").unwrap();
    f.editor.execute_command("diff").unwrap();
    wait(&mut f.editor, |e| {
        e.git_current() && e.review.store.is_some()
    });
    f.editor.execute_command("note file").unwrap();
    assert!(f.editor.execute_command("ai context file").is_err());
    f.editor.handle_key(Key::Char('i'));
    f.editor.paste("Please clarify this revision.");
    f.editor.handle_key(Key::Escape);
    f.editor.execute_command("note save").unwrap();
    wait(&mut f.editor, |e| {
        e.note_edit.is_none() && e.git_current() && !e.review.busy
    });
    let index = fs::read(f.root.join(".git/index")).unwrap();
    f.editor.execute_command("ai context hunk").unwrap();
    assert!(f.editor.ai.draft.context[0].text.contains("-base"));
    assert!(f.editor.ai.draft.context[0].text.contains("+old revision"));
    f.editor.execute_command("ai context diff").unwrap();
    f.editor.execute_command("ai add feedback").unwrap();
    let feedback = &f.editor.ai.draft.context[1];
    assert!(feedback.label.contains("Markdown"));
    assert!(feedback.text.contains("Please clarify this revision"));
    assert!(feedback.source.is_none());
    assert!(!f.editor.ai.pending);
    f.editor.ai.config.model = Some("normal".into());
    f.editor
        .execute_command("ai ask Explain this feedback and infer any uncertainty")
        .unwrap();
    f.editor.execute_command("ai send").unwrap();
    f.finish();
    assert_eq!(answer(&f.editor).summary, "Captured text received");
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read_to_string(&f.source).unwrap(), "old revision\n");
    assert_eq!(f.text(), "old revision\n");
}

#[test]
fn overlapping_occurrences_are_ambiguous_even_when_match_indices_would_find_one() {
    let mut f = Fixture::new("aaa\n");
    f.request("normal");
    f.finish();
    f.editor.ai.conversation.as_mut().unwrap().turns[0]
        .answer
        .as_mut()
        .unwrap()
        .edits = vec![Replacement {
        context: 1,
        before: "aa".into(),
        after: "x".into(),
    }];
    assert!(f.editor.prepare_ai_edit().is_err());
    assert_eq!(f.text(), "aaa\n");
}

#[cfg(unix)]
#[test]
fn raw_paths_roundtrip_and_symlinks_and_hardlinks_are_refused() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let mut f = Fixture::new("old\n");
    let path = f
        .root
        .join(std::ffi::OsString::from_vec(b"raw\xff:\n.txt".to_vec()));
    fs::write(&path, "old\n").unwrap();
    f.editor.open_buffer(&path).unwrap();
    f.request("normal");
    f.finish();
    let saved = f.dir.path().join("saved.json");
    let conversation = f.editor.ai.conversation.as_ref().unwrap();
    let (_, hash) = ai_store::save(conversation, &saved, None, &[]).unwrap();
    assert_eq!(
        ai_store::load(&saved).unwrap().0.turns[0].draft.context[0]
            .source
            .as_ref()
            .unwrap()
            .path,
        path
    );
    let alias = f.dir.path().join("alias.json");
    symlink(&saved, &alias).unwrap();
    assert!(ai_store::load(&alias).is_err());
    assert!(ai_store::save(conversation, &alias, Some(&hash), &[]).is_err());
    fs::remove_file(&alias).unwrap();
    fs::hard_link(&saved, &alias).unwrap();
    assert!(ai_store::load(&saved).is_err());
}
