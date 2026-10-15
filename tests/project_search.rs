use editor_pro::{
    editor::{Editor, View},
    file::Document,
    input::Key,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn editor(root: &Path) -> Editor {
    let source = root.join("source.txt");
    fs::write(&source, "source\n".repeat(100)).unwrap();
    let mut editor = Editor::new(Document::open(&source).unwrap());
    editor.project_search.config.root = Some(root.into());
    editor
}
fn finish(e: &mut Editor) {
    let start = Instant::now();
    while e.project_search.pending {
        e.tick();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "search timed out: {}",
            e.message
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ripgrep_search_handles_unicode_crlf_literal_patterns_and_ignore_rules() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(dir.path().join("ignored.txt"), "[needle]").unwrap();
    fs::write(dir.path().join(".git/private"), "[needle]").unwrap();
    fs::write(dir.path().join(".hidden"), "[needle]").unwrap();
    fs::write(dir.path().join("binary"), b"[needle]\0").unwrap();
    let target = dir.path().join("a: space 界.txt");
    fs::write(&target, "😀 e\u{301} [needle] [needle]\r\nlast [needle]").unwrap();
    let mut e = editor(dir.path());
    e.document.buffer.goto_line(50, false);
    e.viewport.top = 40;
    let viewport = e.viewport.clone();
    let cursor = e.document.buffer.cursor();
    e.execute_command("grep [needle]").unwrap();
    finish(&mut e);
    assert!(!e.message_is_error, "{}", e.message);
    let results = e.project_search.results.as_ref().unwrap();
    assert_eq!(results.hits.len(), 4);
    assert_eq!(results.hits.iter().filter(|h| h.path == target).count(), 3);
    let selected = results.hits.iter().position(|h| h.path == target).unwrap();
    assert_eq!(results.hits[selected].byte_column, "😀 e\u{301} ".len());
    e.project_search.selected = selected;
    for (w, h) in [(1, 1), (2, 2), (18, 5), (100, 24)] {
        Terminal::new(TestBackend::new(w, h))
            .unwrap()
            .draw(|frame| ui::render(frame, &mut e, &Theme::night()))
            .unwrap();
    }
    assert_eq!(e.viewport, viewport);
    assert_eq!(e.document.buffer.cursor(), cursor);
    e.handle_key(Key::Enter);
    assert_eq!(e.document.path(), Some(target.as_path()));
    assert_eq!(e.document.buffer.cursor(), "😀 e\u{301} ".chars().count());
    assert!(!e.document.buffer.is_modified());
    e.execute_command("back").unwrap();
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.viewport, viewport);
    e.execute_command("forward").unwrap();
    assert_eq!(e.document.path(), Some(target.as_path()));
    assert_eq!(
        fs::read_to_string(target).unwrap(),
        "😀 e\u{301} [needle] [needle]\r\nlast [needle]"
    );
}

#[cfg(unix)]
#[test]
fn search_paths_are_lossless_and_patterns_cannot_become_arguments() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let dir = tempfile::tempdir().unwrap();
    let pattern = "--$(literal)";
    let name = std::ffi::OsString::from_vec(b"raw\xff:\nfile".to_vec());
    let path = dir.path().join(name);
    fs::write(&path, pattern).unwrap();
    symlink(&path, dir.path().join("link")).unwrap();
    let mut e = editor(dir.path());
    e.search_project(pattern).unwrap();
    finish(&mut e);
    assert!(!e.message_is_error, "{}", e.message);
    let results = e.project_search.results.as_ref().unwrap();
    assert_eq!(results.hits.len(), 1);
    assert_eq!(results.hits[0].path, path);
    e.handle_key(Key::Enter);
    assert_eq!(e.document.path(), Some(path.as_path()));
}

#[test]
fn stale_disk_matches_cannot_move_source_or_overwrite_dirty_targets() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    fs::write(&target, "needle\r\n").unwrap();
    let mut e = editor(dir.path());
    e.open_buffer(&target).unwrap();
    e.document.buffer.insert("local ");
    e.document.buffer.commit();
    e.switch_buffer(1).unwrap();
    let cursor = e.document.buffer.cursor();
    e.search_project("needle").unwrap();
    finish(&mut e);
    e.handle_key(Key::Enter);
    assert_eq!(e.active_buffer_id, 1);
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert!(e.message_is_error);
    assert_eq!(e.view, View::ProjectSearch);
    e.handle_key(Key::Escape);
    e.switch_buffer(2).unwrap();
    assert_eq!(e.document.buffer.text().to_string(), "local needle\r\n");
    e.execute_command("bd!").unwrap();
    e.search_project("needle").unwrap();
    finish(&mut e);
    fs::write(&target, "external change\n").unwrap();
    e.handle_key(Key::Enter);
    assert_eq!(e.active_buffer_id, 1);
    assert_eq!(e.buffer_count(), 1);
    assert!(e.message_is_error);
    assert_eq!(fs::read_to_string(target).unwrap(), "external change\n");
}

#[test]
fn root_discovery_no_matches_and_explicit_root_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("elsewhere"), "needle").unwrap();
    let mut e = editor(&dir.path().join("src"));
    e.project_search.config.root = None;
    e.search_project("needle").unwrap();
    finish(&mut e);
    assert!(!e.message_is_error, "{}", e.message);
    assert_eq!(e.project_search.results.as_ref().unwrap().root, dir.path());
    assert_eq!(e.project_search.results.as_ref().unwrap().hits.len(), 1);
    e.execute_command(&format!("grepdir {}", dir.path().join("src").display()))
        .unwrap();
    e.execute_command("grep needle").unwrap();
    finish(&mut e);
    assert!(!e.message_is_error, "{}", e.message);
    assert!(e.project_search.results.as_ref().unwrap().hits.is_empty());
    assert!(e.search_project("").is_err());
    assert!(e.search_project("two\nlines").is_err());
}

#[cfg(unix)]
fn fake_rg(root: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("fake-rg");
    fs::write(
        &path,
        r#"#!/usr/bin/env python3
import os, sys, time
query = sys.argv[-2]
if query in ('slow', 'hang'):
    with open('pid', 'w') as f: f.write(str(os.getpid()))
    time.sleep(.35 if query == 'slow' else 30)
if query == 'error':
    print('permission denied',file=sys.stderr)
    sys.exit(2)
if query == 'malformed':
    sys.stdout.buffer.write(b'not a record\n')
else:
    count = 5000 if query == 'many' else 1
    for _ in range(count):
        sys.stdout.buffer.write(b'./target.txt\x001:1:' + query.encode() + b'\n')
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn slow_search_can_be_replaced_or_cancelled_without_blocking_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = editor(dir.path());
    e.project_search.config.executable = fake_rg(dir.path());
    let begin = Instant::now();
    e.search_project("slow").unwrap();
    assert!(begin.elapsed() < Duration::from_millis(200));
    while !dir.path().join("pid").exists() {
        thread::sleep(Duration::from_millis(5));
        assert!(begin.elapsed() < Duration::from_secs(3));
    }
    e.search_project("fast").unwrap();
    finish(&mut e);
    assert_eq!(
        e.project_search.results.as_ref().unwrap().hits[0].text,
        "fast"
    );
    e.search_project("hang").unwrap();
    e.execute_command("ls").unwrap();
    e.tick();
    assert!(!e.project_search.pending);
    e.handle_key(Key::Escape);
    e.search_project("hang").unwrap();
    let begin = Instant::now();
    e.handle_key(Key::Escape);
    e.handle_key(Key::Char('i'));
    e.paste("unsaved ");
    e.handle_key(Key::Escape);
    assert!(begin.elapsed() < Duration::from_millis(200));
    assert_eq!(e.view, View::Editing);
    for _ in 0..80 {
        e.tick();
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(e.view, View::Editing);
    assert!(e.project_search.results.is_none());
    assert!(e.document.buffer.text().to_string().starts_with("unsaved "));
    e.handle_key(Key::Char('u'));
    assert!(!e.document.buffer.is_modified());
}

#[cfg(unix)]
#[test]
fn search_limits_failures_and_timeouts_are_explicit_and_children_are_reaped() {
    let dir = tempfile::tempdir().unwrap();
    let mut e = editor(dir.path());
    e.project_search.config.executable = fake_rg(dir.path());
    e.project_search.config.timeout = Duration::from_millis(200);
    e.search_project("many").unwrap();
    finish(&mut e);
    let results = e.project_search.results.as_ref().unwrap();
    assert_eq!(results.hits.len(), 4096);
    assert!(results.limited);
    for (query, message) in [
        ("error", "permission denied"),
        ("malformed", "Malformed"),
        ("hang", "timed out"),
    ] {
        e.search_project(query).unwrap();
        finish(&mut e);
        assert!(
            e.project_search.error.as_ref().unwrap().contains(message),
            "{}",
            e.message
        );
        assert!(e.project_search.results.is_none());
    }
    let pid = fs::read_to_string(dir.path().join("pid")).unwrap();
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let mut missing = editor(dir.path());
    missing.project_search.config.executable = dir.path().join("not-installed");
    missing.search_project("needle").unwrap();
    finish(&mut missing);
    assert!(missing.message.contains("Cannot run ripgrep"));
    assert_eq!(
        missing.document.buffer.text().to_string(),
        "source\n".repeat(100)
    );
}
