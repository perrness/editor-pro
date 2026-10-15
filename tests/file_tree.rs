use editor_pro::{
    editor::{Editor, View},
    file::Document,
    input::Key,
    theme::Theme,
    ui,
};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn settle(e: &mut Editor) {
    let deadline = Instant::now() + Duration::from_secs(3);
    // A scan can return an empty directory, so wait for the completion tick.
    while !e.tick_tree() {
        assert!(Instant::now() < deadline, "scan did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn tree(root: &Path) -> Editor {
    let mut e = Editor::new(Document::default());
    e.toggle_tree(Some(root)).unwrap();
    settle(&mut e);
    e
}
fn type_keys(e: &mut Editor, value: &str) {
    for c in value.chars() {
        e.handle_key(Key::Char(c));
    }
}

#[test]
fn nested_keyboard_navigation_preserves_dirty_source_and_viewport() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::write(dir.path().join("nested/界.txt"), "😀\r\n").unwrap();
    let mut e = tree(dir.path());
    e.handle_key(Key::Escape);
    e.document.buffer.insert("unsaved\nsource");
    e.document.buffer.commit();
    e.document
        .buffer
        .set_cursor(e.document.buffer.cursor(), false);
    e.viewport.top = 3;
    e.viewport.left = 2;
    let cursor = e.document.buffer.cursor();
    let viewport = e.viewport.clone();
    e.toggle_tree(Some(dir.path())).unwrap();
    e.handle_key(Key::Right);
    settle(&mut e);
    assert_eq!(e.tree.entries.len(), 2);
    e.handle_key(Key::Left);
    assert_eq!(e.tree.entries.len(), 1);
    e.handle_key(Key::Right);
    settle(&mut e);
    type_keys(&mut e, " pv");
    assert_eq!(e.view, View::Editing);
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.viewport, viewport);
    assert!(e.document.buffer.is_modified());
    e.toggle_tree(Some(dir.path())).unwrap();
    e.handle_key(Key::Down);
    e.handle_key(Key::Enter);
    assert_eq!(e.view, View::Editing);
    assert!(e.document.path().unwrap().ends_with("界.txt"));
    assert_eq!(e.dirty_buffer_count(), 1);
    assert_eq!(e.document.buffer.text().to_string(), "😀\r\n");
}

#[test]
fn leader_sequence_is_normal_only_and_cancels_without_edits() {
    let mut e = Editor::new(Document::default());
    type_keys(&mut e, " px");
    assert_eq!(e.document.buffer.text().to_string(), "");
    type_keys(&mut e, " p");
    e.handle_key(Key::Escape);
    type_keys(&mut e, "i pv");
    assert_eq!(e.document.buffer.text().to_string(), " pv");
    e.handle_key(Key::Escape);
    type_keys(&mut e, " pv");
    assert_eq!(e.view, View::Tree);
    settle(&mut e);
    type_keys(&mut e, " pv");
    assert_eq!(e.view, View::Editing);
    e.handle_key(Key::Ctrl('p'));
    assert_eq!(e.view, View::Files);
}

#[test]
fn create_move_delete_confirmation_collisions_and_directory_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut e = tree(&root);
    e.handle_key(Key::Char('a'));
    type_keys(&mut e, "界.txt");
    e.handle_key(Key::Enter);
    settle(&mut e);
    let source = root.join("界.txt");
    assert!(source.exists());
    assert!(e.tree_operation('a', &root, "界.txt").is_err());
    e.tree_operation('A', &root, "folder").unwrap();
    settle(&mut e);
    e.tree_operation('r', &source, "folder/moved.txt").unwrap();
    settle(&mut e);
    let moved = root.join("folder/moved.txt");
    assert!(!source.exists());
    assert!(moved.exists());
    assert!(e.tree_operation('d', &root.join("folder"), "yes").is_err());
    assert!(e.tree_operation('r', &moved, "missing/file").is_err());
    assert!(e.tree_operation('a', &root, "../outside").is_err());
    e.tree_operation('r', &root.join("folder"), "renamed")
        .unwrap();
    settle(&mut e);
    e.handle_key(Key::Right);
    settle(&mut e);
    e.handle_key(Key::Down);
    e.handle_key(Key::Char('d'));
    type_keys(&mut e, "no");
    e.handle_key(Key::Enter);
    assert!(root.join("renamed/moved.txt").exists());
    e.handle_key(Key::Char('d'));
    type_keys(&mut e, "yes");
    e.handle_key(Key::Enter);
    settle(&mut e);
    assert!(!root.join("renamed/moved.txt").exists());
    e.tree_operation('d', &root.join("renamed"), "yes").unwrap();
    settle(&mut e);
    assert!(!root.join("renamed").exists());
}

#[test]
fn open_buffers_and_their_ancestors_are_protected_and_history_is_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir(root.join("folder")).unwrap();
    let a = root.join("folder/a");
    let b = root.join("b");
    fs::write(&a, "a\r\n").unwrap();
    fs::write(&b, "b").unwrap();
    let mut e = Editor::new(Document::open(&a).unwrap());
    e.document.buffer.insert("dirty");
    e.document.buffer.commit();
    e.open_buffer(&b).unwrap();
    e.toggle_tree(Some(&root)).unwrap();
    settle(&mut e);
    for path in [&a, &b, &root.join("folder")] {
        assert!(e.tree_operation('d', path, "yes").is_err());
        assert!(e.tree_operation('r', path, "new").is_err());
    }
    assert_eq!(e.dirty_buffer_count(), 1);
    assert_eq!(e.buffer_count(), 2);
    assert_eq!(fs::read(&a).unwrap(), b"a\r\n");
    e.navigation.recent.push(root.join("old"));
    fs::write(root.join("old"), "old").unwrap();
    e.tree_operation('d', &root.join("old"), "yes").unwrap();
    settle(&mut e);
    assert!(!e.navigation.recent.contains(&root.join("old")));
}

#[cfg(unix)]
#[test]
fn symlinks_are_leaves_and_escaping_paths_and_dangling_collisions_are_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    symlink(outside.path(), root.join("link")).unwrap();
    symlink(outside.path().join("missing"), root.join("dangling")).unwrap();
    let mut e = tree(&root);
    assert!(e.tree.entries.iter().all(|e| e.symlink && !e.directory));
    assert!(e.tree_operation('a', &root, "dangling").is_err());
    assert!(
        e.tree_operation('a', &root.join("link"), "escaped")
            .is_err()
    );
    assert!(!outside.path().join("escaped").exists());
    e.tree.selected = 1;
    e.handle_key(Key::Enter);
    assert!(e.message_is_error);
    assert_eq!(e.view, View::Tree);
    e.tree_operation('d', &root.join("link"), "yes").unwrap();
    settle(&mut e);
    assert!(outside.path().exists());
}

#[test]
fn rendering_handles_tiny_terminals_and_control_characters() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("界\x1b.txt"), "").unwrap();
    let mut e = tree(dir.path());
    for (w, h) in [(80, 24), (12, 4), (1, 1)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| ui::render(f, &mut e, &Theme::night()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!text.contains('\x1b'));
        if w == 80 {
            assert!(text.contains("a/A new"));
        }
    }
}

#[test]
fn scans_are_bounded_and_mutations_wait_for_pending_results() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..10_002 {
        fs::write(dir.path().join(format!("{i:05}")), "").unwrap();
    }
    let root = dir.path().canonicalize().unwrap();
    let mut e = Editor::new(Document::default());
    e.toggle_tree(Some(&root)).unwrap();
    assert!(e.tree_operation('a', &root, "new").is_err());
    e.handle_key(Key::Escape);
    settle(&mut e);
    assert_eq!(e.view, View::Editing);
    assert_eq!(e.tree.entries.len(), 10_000);
    assert!(e.message.contains("limited"));
    type_keys(&mut e, " pv");
    assert_eq!(e.tree.root, root);
    assert_eq!(e.view, View::Tree);
}

#[cfg(unix)]
#[test]
fn filesystem_permission_errors_preserve_entries_and_source() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut e = tree(&root);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
    let result = e.tree_operation('a', &root, "denied");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    // Privileged test runners may bypass filesystem mode bits.
    if !root.join("denied").exists() {
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(e.tree.entries.is_empty());
        assert_eq!(e.document.buffer.text().to_string(), "");
    }
}

#[test]
fn moved_closed_file_history_reopens_new_path_and_deleted_entries_do_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("a.txt");
    fs::write(&path, "kept").unwrap();
    let mut e = Editor::new(Document::open(&path).unwrap());
    let id = e.active_buffer_id;
    e.new_buffer();
    e.switch_buffer(id).unwrap();
    e.close_buffer(false).unwrap();
    e.toggle_tree(Some(&root)).unwrap();
    settle(&mut e);
    e.tree_operation('r', &path, "new.txt").unwrap();
    settle(&mut e);
    assert!(e.navigation.recent.contains(&root.join("new.txt")));
    assert!(!e.navigation.recent.contains(&path));
    e.handle_key(Key::Escape);
    e.navigate_history(true).unwrap(); // Return through the unnamed source.
    e.navigate_history(true).unwrap();
    assert_eq!(e.document.path(), Some(root.join("new.txt").as_path()));
    assert_eq!(e.document.buffer.text().to_string(), "kept");
    e.close_buffer(false).unwrap();
    e.toggle_tree(Some(&root)).unwrap();
    fs::remove_file(root.join("new.txt")).unwrap();
    e.handle_key(Key::Enter);
    assert!(e.message_is_error);
    assert_eq!(e.view, View::Tree);
    assert!(e.document.path().is_none());
}

#[cfg(unix)]
#[test]
fn folder_replaced_by_symlink_after_listing_is_not_scanned() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = dir.path().join("folder");
    fs::create_dir(&path).unwrap();
    fs::write(outside.path().join("secret"), "").unwrap();
    let mut e = tree(dir.path());
    fs::remove_dir(&path).unwrap();
    symlink(outside.path(), &path).unwrap();
    e.handle_key(Key::Right);
    settle(&mut e);
    assert!(e.message_is_error);
    assert_eq!(e.tree.entries.len(), 1);
}
