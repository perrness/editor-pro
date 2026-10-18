use editor_pro::{
    editor::{Editor, View},
    file::Document,
    input::Key,
    picker::FilePicker,
};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use tempfile::tempdir;
fn settle(picker: &mut FilePicker) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        picker.tick();
        if !picker.working() {
            break;
        }
        assert!(Instant::now() < deadline, "picker stalled");
        thread::sleep(Duration::from_millis(2));
    }
}
fn keys(e: &mut Editor, s: &str) {
    for c in s.chars() {
        e.handle_key(Key::Char(c));
    }
}
#[test]
fn picker_respects_ignore_files_and_keeps_unicode_space_and_hidden_paths() {
    let dir = tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".gitignore"), "ignored*\n").unwrap();
    for name in [
        "src/界 file.rs",
        "src/main.rs",
        "ignored.rs",
        ".hidden.toml",
        ".git/config",
    ] {
        fs::write(dir.path().join(name), "").unwrap();
    }
    let mut p = FilePicker::new(dir.path()).unwrap();
    settle(&mut p);
    let paths: Vec<_> = (0..p.match_count())
        .map(|i| p.candidate(i).unwrap().display.clone())
        .collect();
    assert!(paths.contains(&"src/界 file.rs".into()));
    assert!(paths.contains(&".hidden.toml".into()));
    assert!(
        !paths
            .iter()
            .any(|s| s.starts_with("ignored") || s.starts_with(".git/"))
    );
    p.set_query("界 frs".into());
    settle(&mut p);
    assert_eq!(p.match_count(), 1);
    assert_eq!(
        p.selected_path().unwrap(),
        dir.path().join("src/界 file.rs")
    );
}
#[test]
fn fuzzy_matching_is_ranked_and_query_changes_cannot_select_stale_results() {
    let dir = tempdir().unwrap();
    for name in ["main.rs", "many_other_items.rs", "README.md"] {
        fs::write(dir.path().join(name), "").unwrap();
    }
    let mut p = FilePicker::new(dir.path()).unwrap();
    settle(&mut p);
    p.set_query("main".into());
    assert!(p.selected_path().is_none());
    settle(&mut p);
    assert_eq!(p.candidate(0).unwrap().display, "main.rs");
    p.set_query("missing".into());
    assert!(p.selected_path().is_none());
    settle(&mut p);
    assert_eq!(p.match_count(), 0);
    p.set_query("README".into());
    p.set_query("ma".into());
    settle(&mut p);
    assert_eq!(p.candidate(0).unwrap().display, "main.rs");
}
#[cfg(unix)]
#[test]
fn picker_does_not_follow_symlink_files_or_directory_loops() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("real.rs"), "").unwrap();
    symlink(dir.path().join("real.rs"), dir.path().join("link.rs")).unwrap();
    symlink(dir.path(), dir.path().join("loop")).unwrap();
    let mut p = FilePicker::new(dir.path()).unwrap();
    settle(&mut p);
    assert_eq!(p.item_count(), 1);
    assert_eq!(p.candidate(0).unwrap().display, "real.rs");
}
#[test]
fn opening_a_picker_result_preserves_dirty_source_and_history() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("target.rs");
    fs::write(&path, "target").unwrap();
    let mut e = Editor::new(Document::default());
    keys(&mut e, "iunsaved");
    e.handle_key(Key::Escape);
    let cursor = e.document.buffer.cursor();
    e.open_picker(dir.path()).unwrap();
    keys(&mut e, "tgrs");
    settle(e.picker.as_mut().unwrap());
    e.handle_key(Key::Enter);
    assert_eq!(e.view, View::Editing);
    assert!(e.picker.is_none());
    assert_eq!(e.document.path(), Some(path.as_path()));
    e.switch_buffer(1).unwrap();
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.document.buffer.text().to_string(), "unsaved");
    keys(&mut e, "u");
    assert_eq!(e.document.buffer.text().to_string(), "");
}

#[test]
fn themed_navigation_panels_preserve_selection_and_unicode_paths() {
    use editor_pro::{theme::Theme, ui};
    use ratatui::{Terminal, backend::TestBackend};
    let dir = tempdir().unwrap();
    let source = dir.path().join("界e\u{301}.rs");
    fs::write(&source, "fn main() {}\r\n").unwrap();
    let mut e = Editor::new(Document::default());
    e.document.buffer.insert("dirty source");
    e.open_picker(dir.path()).unwrap();
    settle(e.picker.as_mut().unwrap());
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    for theme in [Theme::night(), Theme::catppuccin(), Theme::gruvbox()] {
        terminal.draw(|f| ui::render(f, &mut e, &theme)).unwrap();
        let cells = terminal.backend().buffer();
        let row: String = (0..80).map(|x| cells[(x, 2)].symbol()).collect();
        assert!(row.contains("▎ [rs] 界"), "{row}");
        assert_eq!(cells[(9, 2)].symbol(), "e\u{301}");
        assert_eq!(cells[(2, 2)].fg, theme.number);
        assert_eq!(cells[(79, 2)].bg, theme.selection);
        assert_eq!(cells[(79, 3)].bg, theme.panel);
        assert_eq!(e.picker.as_ref().unwrap().selected_path().unwrap(), source);
    }
    e.handle_key(Key::Enter);
    assert_eq!(e.document.path(), Some(source.as_path()));
    assert_eq!(e.dirty_buffer_count(), 1);
    e.execute_command("ls").unwrap();
    for theme in [Theme::night(), Theme::catppuccin(), Theme::gruvbox()] {
        terminal.draw(|f| ui::render(f, &mut e, &theme)).unwrap();
        let cells = terminal.backend().buffer();
        let row: String = (0..80).map(|x| cells[(x, 2)].symbol()).collect();
        assert!(row.contains("[rs]"), "{row}");
        assert!(row.contains("界") && row.contains("e\u{301}.rs"), "{row}");
        assert_eq!(cells[(0, 2)].symbol(), "▎");
        assert_eq!(cells[(79, 2)].bg, theme.selection);
        let dirty: String = (0..80).map(|x| cells[(x, 1)].symbol()).collect();
        assert_eq!(
            cells[(dirty.find('+').unwrap() as u16, 1)].fg,
            theme.modified
        );
    }
    e.handle_key(Key::Up);
    e.handle_key(Key::Enter);
    assert_eq!(e.document.buffer.text().to_string(), "dirty source");
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "");
    assert_eq!(fs::read_to_string(&source).unwrap(), "fn main() {}\r\n");
}
#[test]
fn cancelled_picker_and_failed_open_preserve_source_location() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("binary"), b"\0").unwrap();
    let mut e = Editor::new(Document::default());
    keys(&mut e, "ikeep");
    e.handle_key(Key::Escape);
    e.viewport.left = 9;
    e.open_picker(dir.path()).unwrap();
    settle(e.picker.as_mut().unwrap());
    e.handle_key(Key::Enter);
    assert_eq!(e.view, View::Files);
    assert!(e.message_is_error);
    assert_eq!(e.document.buffer.text().to_string(), "keep");
    e.handle_key(Key::Escape);
    assert_eq!(e.view, View::Editing);
    assert!(e.picker.is_none());
    assert_eq!(e.viewport.left, 9);
    assert!(e.open_picker(Path::new("/missing/parent")).is_err());
    assert_eq!(e.view, View::Editing);
}
#[test]
fn picker_prompt_is_data_and_backspace_removes_one_grapheme() {
    let dir = tempdir().unwrap();
    let mut e = Editor::new(Document::default());
    e.open_picker(dir.path()).unwrap();
    keys(&mut e, ":q!e\u{301}");
    e.handle_key(Key::Backspace);
    assert_eq!(e.picker.as_ref().unwrap().query, ":q!");
    e.paste(":q!\n");
    assert!(!e.quit);
    assert_eq!(e.document.buffer.text().to_string(), "");
}
