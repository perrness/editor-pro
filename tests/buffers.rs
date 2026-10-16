use editor_pro::{
    editor::{Editor, View},
    file::Document,
    input::Key,
};
use std::fs;
use tempfile::tempdir;
fn keys(e: &mut Editor, keys: &str) {
    for c in keys.chars() {
        e.handle_key(Key::Char(c));
    }
}
fn text(e: &Editor) -> String {
    e.document.buffer.text().to_string()
}
#[test]
fn switching_preserves_dirty_text_independent_history_and_location() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.rs");
    let b = dir.path().join("b.rs");
    fs::write(&a, "a\n".repeat(100)).unwrap();
    fs::write(&b, "b\n".repeat(100)).unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    keys(&mut e, "iA");
    e.handle_key(Key::Escape);
    e.document.buffer.goto_line(60, false);
    e.viewport.top = 50;
    e.viewport.left = 4;
    e.viewport.height = 20;
    let saved = e.viewport.clone();
    let cursor = e.document.buffer.cursor();
    e.open_buffer(&b).unwrap();
    keys(&mut e, "iB");
    e.handle_key(Key::Escape);
    e.document.buffer.goto_line(80, false);
    e.viewport.top = 70;
    e.switch_buffer(1).unwrap();
    assert!(text(&e).starts_with("Aa"));
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.viewport, saved);
    keys(&mut e, "u");
    assert!(!e.document.buffer.is_modified());
    e.switch_buffer(2).unwrap();
    assert!(text(&e).starts_with("Bb"));
    assert_eq!(e.document.buffer.position().0, 80);
    assert_eq!(e.viewport.top, 70);
    assert!(e.document.buffer.is_modified());
    keys(&mut e, "u");
    assert!(text(&e).starts_with("b"));
    e.switch_buffer(1).unwrap();
    e.handle_key(Key::Ctrl('r'));
    assert!(text(&e).starts_with("Aa"));
}
#[test]
fn opening_existing_path_alias_reuses_buffer_without_reloading_edits() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a");
    fs::write(&path, "disk").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    keys(&mut e, "ilocal");
    e.handle_key(Key::Escape);
    e.new_buffer();
    e.open_buffer(&dir.path().join("./a")).unwrap();
    assert_eq!(e.active_buffer_id, 1);
    assert_eq!(e.buffer_infos().len(), 2);
    assert_eq!(text(&e), "localdisk");
    e.execute_command(&format!("e {}", path.display())).unwrap();
    assert_eq!(text(&e), "localdisk");
}
#[test]
fn quit_and_write_quit_protect_inactive_dirty_buffers() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let mut e = Editor::new(Document::open(&a).unwrap());
    keys(&mut e, "iunsaved");
    e.handle_key(Key::Escape);
    e.open_buffer(&b).unwrap();
    assert!(e.execute_command("q").is_err());
    assert!(e.execute_command("wq").is_err());
    assert!(!e.quit);
    assert!(b.exists());
    e.execute_command("b 1").unwrap();
    e.execute_command("w").unwrap();
    e.execute_command("q").unwrap();
    assert!(e.quit);
}
#[test]
fn closing_and_reloading_dirty_buffer_require_force() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a");
    fs::write(&path, "disk").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    keys(&mut e, "ilocal");
    e.handle_key(Key::Escape);
    assert!(e.execute_command("bd").is_err());
    assert!(e.execute_command("e").is_err());
    assert!(e.execute_command("e! /missing/parent/file").is_err());
    assert_eq!(text(&e), "localdisk");
    e.execute_command("e!").unwrap();
    assert_eq!(text(&e), "disk");
    keys(&mut e, "iagain");
    e.handle_key(Key::Escape);
    e.new_buffer();
    e.execute_command("bp").unwrap();
    e.execute_command("bd!").unwrap();
    assert_eq!(e.active_buffer_id, 2);
    assert_eq!(e.buffer_infos().len(), 1);
    e.execute_command("bd").unwrap();
    assert!(e.document.path().is_none());
    assert_eq!(e.buffer_infos().len(), 1);
}
#[test]
fn failed_open_and_switch_preserve_active_document_and_viewport() {
    let mut e = Editor::new(Document::default());
    keys(&mut e, "ikeep");
    e.handle_key(Key::Escape);
    e.viewport.top = 3;
    assert!(
        e.open_buffer(std::path::Path::new("/missing/parent/file"))
            .is_err()
    );
    assert!(e.switch_buffer(90).is_err());
    assert_eq!(text(&e), "keep");
    assert_eq!(e.viewport.top, 3);
    assert_eq!(e.buffer_infos().len(), 1);
}
#[test]
fn save_as_cannot_take_the_path_of_an_open_unsaved_new_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("new.rs");
    let mut e = Editor::new(Document::open(&path).unwrap());
    keys(&mut e, "ikeep");
    e.handle_key(Key::Escape);
    e.new_buffer();
    keys(&mut e, "iother");
    e.handle_key(Key::Escape);
    assert!(e.execute_command(&format!("w {}", path.display())).is_err());
    assert!(!path.exists());
    e.switch_buffer(1).unwrap();
    assert_eq!(text(&e), "keep");
}
#[test]
fn disk_checks_cover_inactive_buffers_and_switch_never_overwrites_them() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a");
    fs::write(&path, "disk").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    keys(&mut e, "ilocal");
    e.handle_key(Key::Escape);
    e.new_buffer();
    fs::write(&path, "external").unwrap();
    e.check_external();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while e.disk.busy {
        e.tick();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        e.buffer_infos()
            .iter()
            .find(|b| b.id == 1)
            .unwrap()
            .external_changed
    );
    e.switch_buffer(1).unwrap();
    assert_eq!(text(&e), "localdisk");
    assert!(e.execute_command("w").is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "external");
}
#[test]
fn buffer_list_is_a_contextual_view_and_cannot_edit_text() {
    let mut e = Editor::new(Document::default());
    keys(&mut e, "ione");
    e.handle_key(Key::Escape);
    e.new_buffer();
    e.execute_command("ls").unwrap();
    assert!(matches!(e.view, View::Buffers { .. }));
    keys(&mut e, "ix");
    e.paste("oops");
    assert_eq!(text(&e), "");
    e.handle_key(Key::Up);
    e.handle_key(Key::Enter);
    assert_eq!(e.view, View::Editing);
    assert_eq!(text(&e), "one");
    e.execute_command("ls").unwrap();
    e.handle_key(Key::Escape);
    assert_eq!(e.active_buffer_id, 1);
}
#[test]
fn search_cache_is_invalidated_between_equal_generation_buffers() {
    let mut e = Editor::new(Document::default());
    keys(&mut e, "ione");
    e.handle_key(Key::Escape);
    e.search.set("one", false);
    e.search.refresh(&e.document.buffer);
    assert_eq!(e.search.matches.len(), 1);
    e.new_buffer();
    keys(&mut e, "itwo");
    e.handle_key(Key::Escape);
    e.search.refresh(&e.document.buffer);
    assert!(e.search.matches.is_empty());
    e.switch_buffer(1).unwrap();
    e.search.refresh(&e.document.buffer);
    assert_eq!(e.search.matches.len(), 1);
}

#[test]
fn reselecting_active_buffer_commits_each_insert_session() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.txt");
    fs::write(&path, "界\r\n").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    let mut states = vec![text(&e)];
    for word in ["one", "two", "three"] {
        keys(&mut e, "i");
        keys(&mut e, word);
        // Reselecting an already active document exits Insert just like a switch.
        e.open_buffer(&path).unwrap();
        assert_eq!(e.mode, editor_pro::editor::Mode::Normal);
        states.push(text(&e));
    }
    for expected in states[..3].iter().rev() {
        keys(&mut e, "u");
        assert_eq!(&text(&e), expected);
    }
    for expected in &states[1..] {
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(&text(&e), expected);
    }
}

#[test]
fn multi_step_history_survives_save_and_switch_with_independent_redo() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "界\r\n").unwrap();
    fs::write(&b, "other").unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    let mut states = vec![text(&e)];
    for word in ["one", "two", "three"] {
        keys(&mut e, "A");
        keys(&mut e, word);
        e.handle_key(Key::Escape);
        states.push(text(&e));
    }
    keys(&mut e, ":w");
    e.handle_key(Key::Enter);
    assert!(!e.document.buffer.is_modified());
    assert_eq!(fs::read_to_string(&a).unwrap(), states[3]);
    keys(&mut e, "u");
    e.open_buffer(&b).unwrap();
    keys(&mut e, "iB");
    e.handle_key(Key::Escape);
    keys(&mut e, "u");
    e.switch_buffer(1).unwrap();
    assert_eq!(text(&e), states[2]);
    for expected in states[..2].iter().rev() {
        keys(&mut e, "u");
        assert_eq!(&text(&e), expected);
    }
    for expected in &states[1..] {
        e.handle_key(Key::Ctrl('r'));
        assert_eq!(&text(&e), expected);
    }
    assert!(!e.document.buffer.is_modified());
    e.switch_buffer(2).unwrap();
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(text(&e), "Bother");
    assert_eq!(fs::read_to_string(&b).unwrap(), "other");
}
