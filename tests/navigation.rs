use editor_pro::{
    editor::{Editor, View},
    file::Document,
    input::Key,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::fs;

#[test]
fn history_restores_locations_and_viewports_without_replacing_dirty_text() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "界 e\u{301} 😀\r\n".repeat(100)).unwrap();
    fs::write(&b, "second\n".repeat(100)).unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    e.document.buffer.insert("unsaved ");
    e.document.buffer.commit();
    e.document.buffer.goto_line(50, false);
    e.viewport.top = 40;
    e.viewport.left = 3;
    let cursor = e.document.buffer.cursor();
    let viewport = e.viewport.clone();
    e.open_buffer(&b).unwrap();
    e.document.buffer.goto_line(80, false);
    e.viewport.top = 70;
    let b_cursor = e.document.buffer.cursor();
    let b_viewport = e.viewport.clone();
    e.handle_key(Key::Ctrl('o'));
    assert_eq!(e.document.path(), Some(a.as_path()));
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.viewport, viewport);
    assert!(e.document.buffer.text().to_string().starts_with("unsaved "));
    e.handle_key(Key::Tab);
    assert_eq!(e.document.path(), Some(b.as_path()));
    assert_eq!(e.document.buffer.cursor(), b_cursor);
    assert_eq!(e.viewport, b_viewport);
    e.execute_command("back").unwrap();
    e.handle_key(Key::Char('u'));
    assert!(!e.document.buffer.is_modified());
    assert!(fs::read_to_string(a).unwrap().starts_with('界'));
}

#[test]
fn history_branches_only_on_successful_navigation_and_clamps_after_edits() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        fs::write(dir.path().join(name), "abcdef").unwrap();
    }
    let mut e = Editor::new(Document::open(&dir.path().join("a")).unwrap());
    e.document.buffer.set_cursor(5, false);
    e.open_buffer(&dir.path().join("b")).unwrap();
    e.execute_command("back").unwrap();
    assert!(e.switch_buffer(999).is_err());
    assert!(e.open_buffer(&dir.path().join("missing/child")).is_err());
    e.execute_command("forward").unwrap();
    e.execute_command("back").unwrap();
    e.document.buffer.replace_range(0..6, "😀\r\n");
    e.open_buffer(&dir.path().join("c")).unwrap();
    assert!(e.execute_command("forward").is_err());
    e.execute_command("back").unwrap();
    assert!(e.document.buffer.cursor() <= e.document.buffer.text().len_chars());
    assert!(e.document.buffer.is_modified());
}

#[test]
fn recent_files_are_unique_bounded_and_preserve_source_on_cancel_or_failure() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::write(&a, "a\n".repeat(100)).unwrap();
    fs::write(&b, "b").unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    e.open_buffer(&b).unwrap();
    e.open_buffer(&a).unwrap();
    assert_eq!(e.navigation.recent, vec![a.clone(), b.clone()]);
    e.document.buffer.goto_line(50, false);
    e.viewport.top = 45;
    let viewport = e.viewport.clone();
    let cursor = e.document.buffer.cursor();
    e.execute_command("recent").unwrap();
    for (width, height) in [(1, 1), (2, 2), (24, 8), (100, 30)] {
        Terminal::new(TestBackend::new(width, height))
            .unwrap()
            .draw(|f| ui::render(f, &mut e, &Theme::night()))
            .unwrap();
    }
    e.handle_key(Key::Char('j'));
    e.handle_key(Key::Char('i'));
    e.paste("ignored");
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.viewport, viewport);
    e.execute_command("recent").unwrap();
    e.handle_key(Key::Down);
    e.handle_key(Key::Enter);
    assert_eq!(e.document.path(), Some(b.as_path()));
    e.execute_command("bd").unwrap();
    fs::remove_file(&b).unwrap();
    e.execute_command("recent").unwrap();
    e.handle_key(Key::Down);
    e.handle_key(Key::Enter);
    assert!(e.message_is_error);
    assert_eq!(e.view, View::Recent);
    assert_eq!(e.document.path(), Some(a.as_path()));
    e.handle_key(Key::Escape);
    for n in 0..110 {
        e.open_buffer(&dir.path().join(format!("new{n}"))).unwrap();
    }
    assert_eq!(e.navigation.recent.len(), 100);
}

#[test]
fn history_reopens_closed_files_and_reuses_open_files_after_parent_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    let a = sub.join("a");
    let b = dir.path().join("b");
    fs::write(&a, "abcdef").unwrap();
    fs::write(&b, "second").unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    e.document.buffer.set_cursor(4, false);
    e.open_buffer(&b).unwrap();
    e.switch_buffer(1).unwrap();
    e.execute_command("bd").unwrap();
    e.execute_command("back").unwrap(); // last departure was from b
    e.execute_command("back").unwrap();
    assert_eq!(e.document.path(), Some(a.as_path()));
    assert_eq!(e.document.buffer.cursor(), 4);
    e.open_buffer(&b).unwrap();
    fs::remove_dir_all(sub).unwrap();
    e.execute_command("back").unwrap();
    assert_eq!(e.document.path(), Some(a.as_path()));
    assert_eq!(e.document.buffer.cursor(), 4);
}

#[test]
fn unavailable_history_entries_are_skipped_without_creating_files_or_moving_source() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::write(&a, "original").unwrap();
    fs::write(&b, "target").unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    e.new_buffer();
    e.open_buffer(&b).unwrap();
    e.switch_buffer(2).unwrap();
    e.execute_command("bd").unwrap();
    assert_eq!(e.active_buffer_id, 3);
    e.execute_command("back").unwrap();
    assert!(
        e.execute_command("back")
            .unwrap_err()
            .to_string()
            .contains("skipped")
    );
    assert_eq!(e.active_buffer_id, 3);
    e.execute_command("back").unwrap();
    assert_eq!(e.active_buffer_id, 1);
    e.open_buffer(&b).unwrap();
    e.switch_buffer(1).unwrap();
    e.execute_command("bd").unwrap();
    fs::remove_file(&a).unwrap();
    assert!(e.execute_command("back").is_ok());
    assert!(e.execute_command("back").is_err());
    assert!(!a.exists());
    assert_eq!(e.document.path(), Some(b.as_path()));
}

#[test]
fn history_follows_save_as_and_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let renamed = dir.path().join("renamed");
    fs::write(&a, "abcdef").unwrap();
    fs::write(&b, "second").unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    e.document.buffer.set_cursor(4, false);
    e.open_buffer(&b).unwrap();
    e.execute_command("back").unwrap();
    e.execute_command(&format!("w {}", renamed.display()))
        .unwrap();
    e.open_buffer(&b).unwrap();
    e.switch_buffer(1).unwrap();
    e.execute_command("bd").unwrap();
    e.execute_command("back").unwrap();
    e.execute_command("back").unwrap();
    assert_eq!(e.document.path(), Some(renamed.as_path()));
    assert_eq!(e.document.buffer.cursor(), 4);
    let id = e.active_buffer_id;
    for i in 0..300 {
        e.switch_buffer(if i % 2 == 0 { 2 } else { id }).unwrap();
    }
    let mut count = 0;
    while e.execute_command("back").is_ok() {
        count += 1;
    }
    assert_eq!(count, 256);
}
