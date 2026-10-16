use editor_pro::{
    editor::{Editor, Mode, View},
    file::Document,
    input::Key,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
use tempfile::tempdir;

fn wait(editor: &mut Editor, condition: impl Fn(&Editor) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        editor.tick();
        if condition(editor) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} / {:?}",
            editor.message,
            editor.disk.view.as_ref().map(|v| &v.message)
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn scan(editor: &mut Editor) {
    editor.check_external();
    wait(editor, |e| !e.disk.busy);
}
fn compare(editor: &mut Editor) {
    editor.execute_command("refresh").unwrap();
    wait(editor, |e| !e.disk.busy);
}
fn text(editor: &Editor) -> String {
    editor.document.buffer.text().to_string()
}
fn screen(editor: &mut Editor, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let theme = editor.theme.palette();
    terminal.draw(|f| ui::render(f, editor, &theme)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn periodic_poll_detects_same_size_preserved_mtime_writes_without_replacing_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, "old\r\n").unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    fs::write(&path, "new\r\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    // No focus event or explicit check: the production tick schedules polling.
    wait(&mut e, |e| e.document.external_changed);
    assert_eq!(text(&e), "old\r\n");
    assert!(!e.document.buffer.is_modified());
    assert!(e.disk.view.is_none());
    assert!(e.message.contains(":refresh"));
    assert!(screen(&mut e, 100, 10).contains("!DISK"));
    e.message = "keep typing".into();
    for version in ["two\r\n", "end\r\n"] {
        let replacement = dir.path().join("replacement");
        fs::write(&replacement, version).unwrap();
        fs::rename(replacement, &path).unwrap();
        scan(&mut e);
        assert_eq!(e.message, "keep typing");
        assert_eq!(text(&e), "old\r\n");
        assert!(e.disk.view.is_none());
    }
    fs::write(&path, "old\r\n").unwrap();
    scan(&mut e);
    assert!(!e.document.external_changed);
}

#[test]
fn clean_refresh_compares_and_preserves_unicode_cursor_viewport_and_crlf() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    let original = "old\r\n".to_owned() + &"e\u{301}界👩‍💻 line\r\n".repeat(40);
    let updated = original.replacen("old", "new", 1);
    fs::write(&path, &original).unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    e.document
        .buffer
        .set_cursor(e.document.buffer.text().line_to_char(20) + 3, false);
    e.viewport.top = 15;
    e.viewport.left = 1;
    e.viewport.height = 10;
    let cursor = e.document.buffer.position();
    let viewport = e.viewport.clone();
    let instance = e.document.buffer.instance();
    fs::write(&path, &updated).unwrap();
    compare(&mut e);
    let output = screen(&mut e, 110, 12);
    assert!(output.contains("- old"), "{output}");
    assert!(output.contains("+ new"), "{output}");
    for (w, h) in [(1, 1), (20, 5), (0, 0), (100, 12)] {
        screen(&mut e, w, h);
    }
    assert_eq!(text(&e), original);
    e.handle_key(Key::Char('r'));
    wait(&mut e, |e| e.disk.view.is_none());
    assert_eq!(text(&e), updated);
    assert_eq!(e.document.buffer.position(), cursor);
    assert_eq!(e.viewport, viewport);
    assert_ne!(e.document.buffer.instance(), instance);
    assert!(!e.document.buffer.is_modified());
    assert!(!e.document.external_changed);
    assert_eq!(e.document.buffer.newline_name(), "CRLF");
    e.execute_command("w").unwrap();
    assert_eq!(fs::read(&path).unwrap(), updated.as_bytes());
    fs::write(&path, "e\u{301}\r\n").unwrap();
    e.execute_command("e").unwrap();
    assert_eq!(e.document.buffer.position(), (1, 0));
    assert_eq!(e.viewport.top, 1);
    assert_eq!(e.viewport.left, 0);
}

#[test]
fn dirty_refresh_is_read_only_cancel_preserves_context_and_undo_and_reload_requires_confirmation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, "disk 界\r\n").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    e.handle_key(Key::Char('i'));
    e.handle_key(Key::Char('a'));
    fs::write(&path, "agent👩‍💻\r\n").unwrap();
    let cursor = e.document.buffer.cursor();
    compare(&mut e);
    assert!(screen(&mut e, 100, 12).contains("UNSAVED conflict"));
    for key in [Key::Char('i'), Key::Char('x'), Key::Enter, Key::Char('y')] {
        e.handle_key(key);
    }
    e.paste("unexpected");
    assert!(e.execute_command("e!").is_err());
    assert_eq!(text(&e), "adisk 界\r\n");
    e.handle_key(Key::Char('r'));
    assert!(e.disk.view.as_ref().unwrap().confirming);
    e.handle_key(Key::Char('n'));
    assert!(!e.disk.view.as_ref().unwrap().confirming);
    e.handle_key(Key::Escape);
    assert_eq!(e.mode, Mode::Insert);
    assert_eq!(e.document.buffer.cursor(), cursor);
    e.handle_key(Key::Char('b'));
    e.handle_key(Key::Escape);
    assert!(e.execute_command("wq").is_err());
    assert!(!e.quit);
    e.handle_key(Key::Char('u'));
    assert_eq!(text(&e), "disk 界\r\n");
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(text(&e), "abdisk 界\r\n");
    compare(&mut e);
    e.handle_key(Key::Char('r'));
    e.handle_key(Key::Char('y'));
    e.handle_key(Key::Char('n')); // Cancel even after approval, while I/O is pending.
    scan(&mut e);
    assert_eq!(text(&e), "abdisk 界\r\n");
    e.handle_key(Key::Char('r'));
    e.handle_key(Key::Char('y'));
    wait(&mut e, |e| e.disk.view.is_none());
    assert_eq!(text(&e), "agent👩‍💻\r\n");
    assert!(!e.document.buffer.is_modified());
    assert_eq!(fs::read_to_string(&path).unwrap(), text(&e));
}

#[test]
fn reload_refuses_new_disk_versions_and_late_buffer_changes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, "old").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    fs::write(&path, "first agent version").unwrap();
    compare(&mut e);
    fs::write(&path, "second agent version").unwrap();
    e.handle_key(Key::Char('r'));
    wait(&mut e, |e| !e.disk.busy);
    assert_eq!(text(&e), "old");
    assert!(e.disk.view.as_ref().unwrap().stale);
    e.handle_key(Key::Char('c'));
    wait(&mut e, |e| !e.disk.busy);
    e.handle_key(Key::Char('r'));
    e.document.buffer.insert("local ");
    wait(&mut e, |e| !e.disk.busy);
    assert_eq!(text(&e), "local old");
    assert!(
        e.disk
            .view
            .as_ref()
            .unwrap()
            .message
            .contains("Buffer changed")
    );
    e.handle_key(Key::Escape);
    assert!(e.execute_command("w").is_err());
    let recovered = dir.path().join("recovered");
    e.execute_command(&format!("w {}", recovered.display()))
        .unwrap();
    assert_eq!(fs::read_to_string(recovered).unwrap(), "local old");
    assert_eq!(fs::read_to_string(&path).unwrap(), "second agent version");
}

#[test]
fn deletion_and_rename_retain_clean_and_dirty_buffers_and_refuse_empty_reload() {
    let dir = tempdir().unwrap();
    for dirty in [false, true] {
        let path = dir.path().join(format!("file-{dirty}.txt"));
        fs::write(&path, "keep\r\n").unwrap();
        let mut e = Editor::new(Document::open(&path).unwrap());
        if dirty {
            e.document.buffer.insert("local ");
        }
        let retained = text(&e);
        if dirty {
            fs::remove_file(&path).unwrap();
        } else {
            fs::rename(&path, dir.path().join("renamed")).unwrap();
        }
        scan(&mut e);
        assert!(e.document.external_changed);
        compare(&mut e);
        assert!(
            e.disk
                .view
                .as_ref()
                .unwrap()
                .message
                .contains("deleted or renamed")
        );
        e.handle_key(Key::Char('r'));
        e.handle_key(Key::Char('y'));
        assert_eq!(text(&e), retained);
        e.handle_key(Key::Escape);
        assert!(e.execute_command("e!").is_err());
        assert!(e.execute_command("w").is_err());
        assert!(!path.exists());
        fs::write(&path, "recreated").unwrap();
        compare(&mut e);
        e.handle_key(Key::Char('r'));
        if dirty {
            e.handle_key(Key::Char('y'));
        }
        wait(&mut e, |e| e.disk.view.is_none());
        assert_eq!(text(&e), "recreated");
    }
}

#[test]
fn polling_rotates_over_inactive_buffers_and_does_not_accept_old_save_or_reload_results() {
    let dir = tempdir().unwrap();
    let mut e = Editor::new(Document::default());
    for id in 0..20 {
        let path = dir.path().join(format!("{id}.txt"));
        fs::write(&path, "old").unwrap();
        e.open_buffer(&path).unwrap();
        fs::write(&path, "new").unwrap();
    }
    for _ in 0..2 {
        scan(&mut e);
    }
    assert_eq!(
        e.buffer_infos()
            .iter()
            .filter(|b| b.external_changed)
            .count(),
        20
    );
    // A replacement document cannot inherit an older scan's observation even
    // when both buffers have mutation generation zero.
    e.check_external();
    e.execute_command("e!").unwrap();
    wait(&mut e, |e| !e.disk.busy);
    assert!(!e.document.external_changed);
    assert_eq!(text(&e), "new");
    e.document.buffer.insert("saved ");
    e.check_external();
    e.execute_command("w").unwrap();
    wait(&mut e, |e| !e.disk.busy);
    assert!(!e.document.external_changed);
    assert_eq!(text(&e), "saved new");
}

#[test]
fn invalid_disk_content_and_symlink_replacement_cannot_replace_unsaved_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, "old").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    e.document.buffer.insert("local ");
    for bytes in [b"binary\0".as_slice(), b"\xff"] {
        fs::write(&path, bytes).unwrap();
        compare(&mut e);
        e.handle_key(Key::Char('r'));
        e.handle_key(Key::Char('y'));
        wait(&mut e, |e| !e.disk.busy);
        assert_eq!(text(&e), "local old");
        assert!(e.disk.view.as_ref().unwrap().stale);
        e.handle_key(Key::Escape);
    }
    #[cfg(unix)]
    {
        fs::remove_file(&path).unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "protected").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        scan(&mut e);
        compare(&mut e);
        assert!(e.disk.view.as_ref().unwrap().message.contains("Symlink"));
        e.handle_key(Key::Char('r'));
        e.handle_key(Key::Char('y'));
        e.handle_key(Key::Escape);
        assert_eq!(text(&e), "local old");
        assert!(e.execute_command("w").is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "protected");
    }
}

#[test]
fn help_defers_reload_and_cancelled_requests_cannot_change_a_new_context() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, "old").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    fs::write(&path, "new").unwrap();
    compare(&mut e);
    e.handle_key(Key::Char('r'));
    e.handle_key(Key::F1);
    thread::sleep(Duration::from_millis(200));
    e.tick();
    assert_eq!(text(&e), "old");
    fs::write(&path, "while help was open").unwrap();
    e.handle_key(Key::Escape);
    wait(&mut e, |e| !e.disk.busy);
    assert_eq!(text(&e), "old");
    assert!(e.disk.view.as_ref().unwrap().stale);
    e.handle_key(Key::Char('c'));
    wait(&mut e, |e| !e.disk.busy);
    e.handle_key(Key::Char('r'));
    wait(&mut e, |e| e.disk.view.is_none());
    assert_eq!(text(&e), "while help was open");
    fs::write(&path, "later").unwrap();
    compare(&mut e);
    e.handle_key(Key::Char('r'));
    e.handle_key(Key::Escape);
    e.new_buffer();
    e.document.buffer.insert("scratch");
    scan(&mut e);
    assert_eq!(text(&e), "scratch");
    assert_eq!(e.view, View::Editing);
    e.switch_buffer(1).unwrap();
    assert_eq!(text(&e), "while help was open");
}

#[test]
fn rapid_atomic_writes_refuse_unstable_refresh_and_leave_editing_responsive() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, "original").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    e.document.buffer.insert("unsaved ");
    let (stop, receiver) = std::sync::mpsc::channel::<()>();
    let writer_path = path.clone();
    let writer = thread::spawn(move || {
        let mut revision = 0;
        loop {
            match receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            revision += 1;
            let temporary = writer_path.with_extension("replacement");
            fs::write(&temporary, format!("agent revision {revision}")).unwrap();
            fs::rename(temporary, &writer_path).unwrap();
        }
    });
    compare(&mut e);
    let message = e.disk.view.as_ref().unwrap().message.clone();
    drop(stop);
    writer.join().unwrap();
    assert!(message.contains("still changing"), "{message}");
    assert_eq!(text(&e), "unsaved original");
    e.handle_key(Key::Escape);
    e.handle_key(Key::Char('i'));
    e.handle_key(Key::Char('x'));
    e.handle_key(Key::Escape);
    assert_eq!(text(&e), "unsaved xoriginal");
    scan(&mut e);
    assert!(e.document.external_changed);
    assert!(e.disk.view.is_none());
    compare(&mut e);
    e.handle_key(Key::Char('r'));
    e.handle_key(Key::Char('y'));
    wait(&mut e, |e| e.disk.view.is_none());
    assert_eq!(text(&e), fs::read_to_string(path).unwrap());
}
