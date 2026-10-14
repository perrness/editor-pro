use editor_pro::{
    diff::{Diff, Kind, Mark},
    git::{self, BufferVersion, Comparison, Config, Request, Snapshot, Worker},
};
use ropey::Rope;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};
use tempfile::{TempDir, tempdir};

struct Repo(TempDir);
impl Repo {
    fn new() -> Self {
        let repo = Self(tempdir().unwrap());
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.invalid"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }
    fn git(&self, args: &[&str]) -> String {
        command(self.0.path(), args)
    }
    fn write(&self, name: &str, text: &str) {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-m", "fixture"]);
    }
    fn request(&self, name: &str, comparison: Comparison) -> Request {
        Request {
            path: self.path(name),
            buffer: None,
            comparison,
            inspect: true,
            review: None,
            review_buffers: Vec::new(),
        }
    }
    fn inspect(&self, name: &str, comparison: Comparison) -> Snapshot {
        git::inspect(&self.request(name, comparison), &Config::default())
            .unwrap()
            .unwrap()
    }
}
fn command(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn text(diff: &Diff, kind: Kind) -> Vec<&str> {
    diff.rows
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.text.as_str())
        .collect()
}
#[test]
fn head_index_revision_and_staged_are_distinct_and_read_only() {
    let r = Repo::new();
    r.write("sub/a", "old\n");
    r.commit();
    let first = r.git(&["rev-parse", "HEAD"]);
    r.write("sub/a", "head\n");
    r.commit();
    r.write("sub/a", "index\n");
    r.git(&["add", "--", "sub/a"]);
    r.write("sub/a", "disk\n");
    let before = fs::read(r.path(".git/index")).unwrap();
    for (base, old, new) in [
        (Comparison::Head, "head", "disk"),
        (Comparison::Index, "index", "disk"),
        (Comparison::Revision(first.trim().into()), "old", "disk"),
        (Comparison::Staged, "head", "index"),
    ] {
        let s = r.inspect("sub/a", base);
        assert_eq!(s.repository.root, r.0.path());
        assert_eq!(s.entries[0].index, 'M');
        assert_eq!(s.entries[0].worktree, 'M');
        let d = s.diff.unwrap();
        assert_eq!(text(&d, Kind::Delete), [old]);
        assert_eq!(text(&d, Kind::Add), [new]);
    }
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), before);
    assert_eq!(fs::read(r.path("sub/a")).unwrap(), b"disk\n");
}
#[test]
fn unsaved_rope_is_compared_without_claiming_disk_or_index_changes() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    let mut req = r.request("a", Comparison::Head);
    req.buffer = Some(BufferVersion {
        id: 7,
        generation: 9,
        dirty: true,
        text: Rope::from_str("unsaved\n"),
    });
    let s = git::inspect(&req, &Config::default()).unwrap().unwrap();
    assert!(s.entries.is_empty());
    let d = s.diff.unwrap();
    assert!(d.right_label.contains("B7@9 UNSAVED"));
    assert_eq!(text(&d, Kind::Add), ["unsaved"]);
    assert_eq!(fs::read(r.path("a")).unwrap(), b"base\n");
}
#[test]
fn unusual_paths_and_rename_sources_survive_machine_readable_output() {
    let r = Repo::new();
    let old = "sub/-界 space\nold";
    let new = "-界 space\nnew";
    r.write(old, "same\n");
    r.commit();
    r.git(&["mv", "--", old, new]);
    r.write(new, "same\nextra\n");
    let s = r.inspect(new, Comparison::Head);
    let e = s.entries.iter().find(|e| e.path == Path::new(new)).unwrap();
    assert_eq!(e.original.as_deref(), Some(Path::new(old)));
    assert_eq!(e.index, 'R');
    let d = s.diff.unwrap();
    assert_eq!(text(&d, Kind::Add), ["extra"]);
    assert!(text(&d, Kind::Delete).is_empty());
    assert!(d.left_label.contains("\\nold"));
    let d = r.inspect(new, Comparison::Index).diff.unwrap();
    assert_eq!(text(&d, Kind::Add), ["extra"]);
}
#[test]
fn selected_revision_follows_a_rename_already_committed() {
    let r = Repo::new();
    r.write("before", "one\ntwo\nthree\n");
    r.commit();
    let old = r.git(&["rev-parse", "HEAD"]);
    r.git(&["mv", "before", "after"]);
    r.commit();
    r.write("after", "one\ntwo\nthree\nnew\n");
    let d = r
        .inspect("after", Comparison::Revision(old.trim().into()))
        .diff
        .unwrap();
    assert!(d.left_label.contains(":before"));
    assert_eq!(text(&d, Kind::Add), ["new"]);
}
#[test]
fn additions_deletions_untracked_empty_and_unborn_are_inspectable() {
    let r = Repo::new();
    r.write("new", "hello\n");
    let s = r.inspect("new", Comparison::Head);
    assert!(s.branch.contains("unborn"));
    assert!(s.entries[0].untracked());
    assert_eq!(text(&s.diff.unwrap(), Kind::Add), ["hello"]);
    r.git(&["add", "new"]);
    assert_eq!(
        text(
            &r.inspect("new", Comparison::Staged).diff.unwrap(),
            Kind::Add
        ),
        ["hello"]
    );
    r.commit();
    fs::remove_file(r.path("new")).unwrap();
    let s = r.inspect("new", Comparison::Head);
    assert_eq!(s.entries[0].worktree, 'D');
    let d = s.diff.unwrap();
    assert!(!d.source_available);
    assert_eq!(text(&d, Kind::Delete), ["hello"]);
    assert_eq!(d.marks.get(&0), Some(&Mark::Deleted));
    r.write("empty", "");
    assert!(
        r.inspect("empty", Comparison::Head)
            .diff
            .unwrap()
            .rows
            .is_empty()
    );
}
#[test]
fn detached_linked_worktrees_and_outside_repository() {
    let r = Repo::new();
    r.write("a", "base");
    r.commit();
    r.git(&["checkout", "--detach"]);
    assert!(
        r.inspect("a", Comparison::Head)
            .branch
            .starts_with("detached@")
    );
    let parent = tempdir().unwrap();
    let linked = parent.path().join("linked\n界");
    r.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
    let req = Request {
        path: linked.join("a"),
        buffer: None,
        comparison: Comparison::Head,
        inspect: true,
        review: None,
        review_buffers: Vec::new(),
    };
    let s = git::inspect(&req, &Config::default()).unwrap().unwrap();
    assert_eq!(s.repository.root, linked);
    assert!(s.repository.git_dir.starts_with(r.path(".git/worktrees")));
    assert_eq!(s.branch, "linked");
    let outside = tempdir().unwrap();
    let req = Request {
        path: outside.path().join("new"),
        ..req
    };
    assert!(git::inspect(&req, &Config::default()).unwrap().is_none());
}
#[test]
fn invalid_revisions_binary_and_conflicts_are_explicit() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    assert!(
        r.inspect("a", Comparison::Revision("--bad".into()))
            .diff
            .unwrap_err()
            .contains("Unavailable revision")
    );
    fs::write(r.path("a"), b"binary\0").unwrap();
    assert!(
        r.inspect("a", Comparison::Head)
            .diff
            .unwrap_err()
            .contains("binary")
    );
    r.write("a", "base\n");
    r.git(&["checkout", "-b", "other"]);
    r.write("a", "other\n");
    r.commit();
    r.git(&["checkout", "main"]);
    r.write("a", "main\n");
    r.commit();
    let merged = Command::new("git")
        .current_dir(r.0.path())
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merged.status.success());
    let s = r.inspect("a", Comparison::Head);
    assert!(s.entries[0].conflict);
    assert!(s.diff.unwrap_err().contains("Unresolved conflict"));
}
#[test]
fn unicode_crlf_and_missing_final_newlines_keep_coordinates() {
    let r = Repo::new();
    r.write("a", "界\r\ne\u{301}\r\nlast");
    r.commit();
    r.write("a", "界\r\n👩‍💻\r\nlast\r\n");
    let d = r.inspect("a", Comparison::Head).diff.unwrap();
    assert_eq!(text(&d, Kind::Delete), ["e\u{301}\r", "last"]);
    assert_eq!(text(&d, Kind::Add), ["👩‍💻\r", "last\r"]);
    assert!(d.rows.iter().any(|r| r.no_newline));
    assert_eq!(d.marks.get(&1), Some(&Mark::Modified));
    assert_eq!(
        d.rows.iter().find(|r| r.text == "👩‍💻\r").unwrap().new,
        Some(1)
    );
}
#[test]
fn hunks_pair_alignment_and_deletion_anchors_map_to_source() {
    let d =
        Diff::parse("@@ -1,3 +1,4 @@\n a\n-b\n+B\n+extra\n c\n@@ -10 +10,0 @@\n-tail\n").unwrap();
    assert_eq!(d.hunks.len(), 2);
    assert_eq!(d.pairs.len(), 7);
    assert_eq!(d.map_old(1), 1);
    assert_eq!(d.map_old(2), 3);
    assert_eq!(d.map_old(5), 6);
    assert_eq!(d.map_old(9), 10);
    assert_eq!(d.map_old(10), 10);
    assert_eq!(d.marks.get(&10), Some(&Mark::Deleted));
    let d = Diff::parse("@@ -1 +0,0 @@\n-first\n").unwrap();
    assert_eq!(d.rows[1].anchor, 0);
    assert_eq!(d.map_old(0), 0);
}
#[test]
fn staged_coordinates_include_unsaved_insertions_before_the_hunk() {
    let r = Repo::new();
    r.write("a", "a\nb\nc\n");
    r.commit();
    r.write("a", "a\nB\nc\n");
    r.git(&["add", "a"]);
    let mut req = r.request("a", Comparison::Staged);
    req.buffer = Some(BufferVersion {
        id: 1,
        generation: 1,
        dirty: true,
        text: Rope::from_str("prefix\na\nB\nc\n"),
    });
    let d = git::inspect(&req, &Config::default())
        .unwrap()
        .unwrap()
        .diff
        .unwrap();
    assert_eq!(
        d.rows.iter().find(|r| r.kind == Kind::Add).unwrap().anchor,
        2
    );
    assert!(!d.right_label.contains("UNSAVED"));
}
fn reply(worker: &Worker) -> git::Reply {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(r) = worker.poll() {
            return r;
        }
        assert!(Instant::now() < until, "worker did not finish");
        thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn worker_coalesces_switches_and_discards_superseded_results() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    let worker = Worker::new(Config::default()).unwrap();
    for n in 0..100 {
        let mut req = r.request("a", Comparison::Revision("missing".into()));
        req.buffer = Some(BufferVersion {
            id: 1,
            generation: n,
            dirty: true,
            text: Rope::from_str("stale"),
        });
        worker.submit(req);
    }
    let serial = worker.submit(r.request("a", Comparison::Head));
    let result = reply(&worker);
    assert_eq!(result.serial, serial);
    assert!(
        result
            .result
            .unwrap()
            .unwrap()
            .diff
            .unwrap()
            .rows
            .is_empty()
    );
    worker.submit(r.request("a", Comparison::Head));
    worker.cancel();
    assert!(worker.poll().is_none());
}
#[test]
fn missing_git_and_failing_git_are_not_clean_repositories() {
    let r = Repo::new();
    let config = Config {
        executable: r.path("missing-git"),
        ..Config::default()
    };
    assert!(
        git::inspect(&r.request("a", Comparison::Head), &config)
            .unwrap_err()
            .contains("Cannot run Git")
    );
    let config = Config {
        executable: "/bin/false".into(),
        ..Config::default()
    };
    assert!(git::inspect(&r.request("a", Comparison::Head), &config).is_err());
}
#[cfg(unix)]
#[test]
fn slow_git_times_out_and_shutdown_reaps_the_process() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    let script = r.path("slow-git");
    fs::write(&script,"#!/usr/bin/python3\nimport os,time\nopen('started','w').write(str(os.getpid()))\ntime.sleep(60)\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        executable: script,
        timeout: Duration::from_millis(200),
        debounce: Duration::ZERO,
    };
    let worker = Worker::new(config.clone()).unwrap();
    worker.submit(r.request("a", Comparison::Head));
    let result = reply(&worker);
    assert!(result.result.unwrap_err().contains("timed out"));
    let pid = fs::read_to_string(r.path("started")).unwrap();
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", &pid])
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::remove_file(r.path("started")).unwrap();
    let worker = Worker::new(Config {
        timeout: Duration::from_secs(60),
        ..config
    })
    .unwrap();
    worker.submit(r.request("a", Comparison::Head));
    let until = Instant::now() + Duration::from_secs(10);
    while !r.path("started").exists() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(2));
    }
    drop(worker);
    let pid = fs::read_to_string(r.path("started")).unwrap();
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", &pid])
            .output()
            .unwrap()
            .status
            .success()
    );
}

use editor_pro::{
    editor::{Editor, View, Viewport},
    file::Document,
    input::Key,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
fn settle_editor(editor: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        editor.tick();
        if !editor.git.loading {
            break;
        }
        assert!(Instant::now() < until, "{}", editor.git.summary());
        thread::sleep(Duration::from_millis(2));
    }
}
fn draw(editor: &mut Editor, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| ui::render(f, editor, &Theme::night()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
#[test]
fn source_gutters_use_selected_base_and_semantic_colors() {
    let r = Repo::new();
    r.write("a", "first\nold\nlast");
    r.commit();
    r.write("a", "first\nnew\nlast");
    r.git(&["add", "a"]);
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    settle_editor(&mut e);
    assert_eq!(e.git_mark(1), Some(Mark::Modified));
    let mut terminal = Terminal::new(TestBackend::new(120, 8)).unwrap();
    let theme = Theme::night();
    terminal.draw(|f| ui::render(f, &mut e, &theme)).unwrap();
    assert_eq!(terminal.backend().buffer()[(0, 1)].fg, theme.diff_modified);
    e.execute_command("gitbase index").unwrap();
    assert_eq!(e.git_mark(1), None);
    settle_editor(&mut e);
    assert_eq!(e.git_mark(1), None);
    e.document.buffer.goto_line(1, false);
    e.document.buffer.replace_range(6..10, "");
    e.document.buffer.commit();
    e.tick();
    assert_eq!(e.git_mark(1), None);
    settle_editor(&mut e);
    assert_eq!(e.git_mark(1), Some(Mark::Deleted));
    let shown = draw(&mut e, 140, 8);
    assert!(shown.contains("S1"));
    assert!(shown.contains("index"));
    assert!(shown.contains("[+]"));
}
#[test]
fn diff_close_and_refresh_preserve_dirty_history_cursor_and_viewport() {
    let r = Repo::new();
    r.write("a", &"row\n".repeat(80));
    r.commit();
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.document.buffer.goto_line(40, true);
    e.document.buffer.insert("界");
    e.document.buffer.commit();
    e.document
        .buffer
        .set_cursor(e.document.buffer.cursor(), false);
    e.viewport = Viewport {
        top: 35,
        left: 0,
        height: 10,
    };
    let location = (e.document.buffer.cursor(), e.viewport.clone());
    let dirty = e.document.buffer.text().to_string();
    e.execute_command("diff HEAD").unwrap();
    settle_editor(&mut e);
    assert!(e.git.diff().unwrap().right_label.contains("UNSAVED"));
    for key in [Key::Char('j'), Key::Char(']'), Key::Tab, Key::Char('G')] {
        e.handle_key(key);
    }
    for (w, h) in [(130, 20), (42, 8), (1, 1), (1, 3), (130, 20)] {
        draw(&mut e, w, h);
        assert_eq!(e.viewport, location.1);
        assert_eq!(e.document.buffer.cursor(), location.0);
    }
    e.execute_command("diff index").unwrap();
    settle_editor(&mut e);
    e.handle_key(Key::Char('r'));
    settle_editor(&mut e);
    e.handle_key(Key::Escape);
    assert_eq!(e.view, View::Editing);
    assert_eq!(e.document.buffer.cursor(), location.0);
    assert_eq!(e.viewport, location.1);
    assert_eq!(e.document.buffer.text().to_string(), dirty);
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "row\n".repeat(80));
    assert!(e.document.buffer.redo());
    assert_eq!(e.document.buffer.text().to_string(), dirty);
}
#[test]
fn changed_file_navigation_retains_documents_and_inspects_deleted_history() {
    let r = Repo::new();
    r.write("a", "a\n");
    r.write("deleted/sub/file", "history\n");
    r.write("renamed", "same\n");
    r.commit();
    r.git(&["mv", "renamed", "new name"]);
    fs::remove_dir_all(r.path("deleted")).unwrap();
    r.write("untracked", "new\n");
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.document.buffer.insert("unsaved");
    e.document.buffer.commit();
    let origin = e.document.buffer.cursor();
    e.execute_command("changes").unwrap();
    settle_editor(&mut e);
    let files = e.changed_files();
    assert_eq!(files.len(), 4);
    assert!(files[0].label.contains("buffer only"));
    assert!(files.iter().any(|f| f.label.contains("renamed → new name")));
    assert!(draw(&mut e, 120, 12).contains("??"));
    e.git.selected = files.iter().position(|f| f.deleted).unwrap();
    e.handle_key(Key::Enter);
    settle_editor(&mut e);
    assert_eq!(e.view, View::Diff);
    assert!(draw(&mut e, 120, 12).contains("history"));
    e.handle_key(Key::Enter);
    assert!(e.message_is_error);
    assert_eq!(e.view, View::Diff);
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.cursor(), origin);
    e.execute_command("changes").unwrap();
    settle_editor(&mut e);
    e.git.selected = e
        .changed_files()
        .iter()
        .position(|f| f.path == r.path("untracked"))
        .unwrap();
    e.handle_key(Key::Enter);
    assert_eq!(e.document.path(), Some(r.path("untracked").as_path()));
    e.document.buffer.insert("also dirty");
    e.document.buffer.commit();
    e.execute_command("b 1").unwrap();
    assert!(e.document.buffer.text().to_string().starts_with("unsaved"));
    assert_eq!(e.document.buffer.cursor(), origin);
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "a\n");
}
#[test]
fn paired_alignment_scroll_and_narrow_fallback_preserve_selection() {
    let r = Repo::new();
    r.write("a", "first\nold1\nold2\nlast\n");
    r.commit();
    r.write("a", "first\nnew1\nnew2\nnew3\nlast\n");
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.execute_command("diff").unwrap();
    settle_editor(&mut e);
    e.handle_key(Key::Tab);
    let output = draw(&mut e, 130, 15);
    assert!(e.git.diff_view.as_ref().unwrap().effective_paired);
    let lines: Vec<_> = output
        .chars()
        .collect::<Vec<_>>()
        .chunks(130)
        .map(|v| v.iter().collect::<String>())
        .collect();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("old1") && l.contains("new1"))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("new3") && !l.contains("old2"))
    );
    e.handle_key(Key::Char('j'));
    e.handle_key(Key::PageDown);
    let selected = e.git.diff_view.as_ref().unwrap().selected;
    let narrow = draw(&mut e, 50, 8);
    assert!(narrow.contains("DIFF inline"));
    assert_eq!(e.git.diff_view.as_ref().unwrap().selected, selected);
    draw(&mut e, 130, 8);
    assert!(e.git.diff_view.as_ref().unwrap().effective_paired);
    assert_eq!(e.git.diff_view.as_ref().unwrap().selected, selected);
}
#[test]
fn diff_jump_uses_deleted_line_anchor_and_clamps_at_eof() {
    for (old, new, expected) in [
        ("first\nkeep\n", "keep\n", 0),
        ("keep\ntail", "keep", 0),
        ("only", "", 0),
    ] {
        let r = Repo::new();
        r.write("a", old);
        r.commit();
        r.write("a", new);
        let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
        e.execute_command("diff").unwrap();
        settle_editor(&mut e);
        let row = e
            .git
            .diff()
            .unwrap()
            .rows
            .iter()
            .position(|r| r.kind == Kind::Delete)
            .unwrap();
        e.git.diff_view.as_mut().unwrap().selected = row;
        e.jump_diff_source().unwrap();
        assert_eq!(e.document.buffer.position().0, expected);
        assert_eq!(e.document.buffer.text().to_string(), new);
    }
}
#[test]
fn rapid_edits_base_and_buffer_switches_cannot_install_stale_results() {
    let r = Repo::new();
    r.write("a", "base");
    r.write("b", "second");
    r.commit();
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.tick();
    for _ in 0..30 {
        e.document.buffer.insert("x");
        e.tick();
        e.execute_command("gitbase missing").unwrap();
        e.tick();
    }
    e.open_buffer(&r.path("b")).unwrap();
    e.execute_command("gitbase index").unwrap();
    e.tick();
    e.document.buffer.insert("LATEST");
    e.document.buffer.commit();
    settle_editor(&mut e);
    let d = e.git.diff().unwrap();
    assert!(d.right_label.contains("B2@"));
    assert_eq!(text(d, Kind::Add), ["LATESTsecond"]);
    assert_eq!(
        e.git.snapshot.as_ref().unwrap().comparison,
        Comparison::Index
    );
}
#[test]
fn external_git_refresh_retains_dirty_text_and_changed_list_selection() {
    let r = Repo::new();
    r.write("a", "head\n");
    r.write("z", "head\n");
    r.commit();
    r.write("z", "disk\n");
    let mut e = Editor::new(Document::open(&r.path("z")).unwrap());
    e.document.buffer.insert("dirty ");
    e.document.buffer.commit();
    e.execute_command("changes").unwrap();
    settle_editor(&mut e);
    e.git.selected = e
        .changed_files()
        .iter()
        .position(|f| f.path == r.path("z"))
        .unwrap();
    r.write("a", "disk\n");
    r.git(&["add", "z"]);
    e.refresh_git();
    settle_editor(&mut e);
    assert_eq!(e.changed_files()[e.git.selected].path, r.path("z"));
    assert!(
        e.git
            .snapshot
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|s| s.index == 'M')
    );
    assert_eq!(e.document.buffer.text().to_string(), "dirty disk\n");
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "disk\n");
}
#[test]
fn diff_is_read_only_and_deleted_clean_buffer_still_has_history() {
    let r = Repo::new();
    r.write("a", "history\n");
    r.commit();
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    fs::remove_file(r.path("a")).unwrap();
    e.execute_command("diff").unwrap();
    settle_editor(&mut e);
    assert_eq!(text(e.git.diff().unwrap(), Kind::Delete), ["history"]);
    for key in [Key::Char('i'), Key::Char('x'), Key::Delete, Key::Char('u')] {
        e.handle_key(key);
    }
    e.paste("bad");
    assert!(e.execute_command("w").is_err());
    assert_eq!(e.document.buffer.text().to_string(), "history\n");
    assert!(!r.path("a").exists());
    assert!(!e.document.buffer.undo());
}
#[test]
fn unsupported_and_invalid_comparisons_render_as_errors_in_tiny_terminals() {
    let r = Repo::new();
    r.write("a", "base");
    r.commit();
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.execute_command("diff not-a-revision").unwrap();
    settle_editor(&mut e);
    assert!(draw(&mut e, 150, 12).contains("Unavailable revision"));
    for (w, h) in [(1, 1), (1, 2), (2, 3), (5, 8)] {
        draw(&mut e, w, h);
    }
    e.handle_key(Key::Char(':'));
    for c in "diff HEAD".chars() {
        e.handle_key(Key::Char(c));
    }
    e.handle_key(Key::Enter);
    settle_editor(&mut e);
    assert!(e.git.diff().is_some());
    e.handle_key(Key::Escape);
    e.new_buffer();
    settle_editor(&mut e);
    assert!(!e.git.loading);
}
#[test]
fn multiple_hunks_navigation_and_base_refresh_keep_source_anchor() {
    let r = Repo::new();
    let original = (0..60).map(|n| format!("row{n}\n")).collect::<String>();
    r.write("a", &original);
    r.commit();
    r.write(
        "a",
        &original
            .replace("row2\n", "changed2\n")
            .replace("row50\n", "changed50\n"),
    );
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.execute_command("diff").unwrap();
    settle_editor(&mut e);
    assert_eq!(e.git.diff().unwrap().hunks.len(), 2);
    e.handle_key(Key::Char(']'));
    let anchor = e.git.diff_view.as_ref().unwrap().anchor;
    assert!(anchor > 40);
    e.execute_command("diff index").unwrap();
    settle_editor(&mut e);
    let v = e.git.diff_view.as_ref().unwrap();
    assert!(
        e.git.diff().unwrap().rows[v.selected]
            .anchor
            .abs_diff(anchor)
            < 2
    );
    e.handle_key(Key::Char('['));
    assert!(e.git.diff_view.as_ref().unwrap().anchor < 10);
}

#[cfg(unix)]
#[test]
fn cancellation_of_running_git_discards_result_and_reaps_before_latest_request() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    let executable = r.path("git-launcher");
    fs::write(&executable,"#!/usr/bin/python3\nimport os,sys,time\nmarker=os.path.join(os.path.dirname(__file__),'started')\nif not os.path.exists(marker):\n    with open(marker,'w') as f: f.write(str(os.getpid()))\n    time.sleep(60)\nos.execv('/usr/bin/git',['git']+sys.argv[1:])\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let worker = Worker::new(Config {
        executable,
        timeout: Duration::from_secs(15),
        debounce: Duration::ZERO,
    })
    .unwrap();
    worker.submit(r.request("a", Comparison::Revision("invalid".into())));
    let until = Instant::now() + Duration::from_secs(10);
    while !r.path("started").exists() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(2));
    }
    let serial = worker.submit(r.request("a", Comparison::Index));
    let result = reply(&worker);
    assert_eq!(result.serial, serial);
    assert_eq!(
        result.result.unwrap().unwrap().comparison,
        Comparison::Index
    );
    let pid = fs::read_to_string(r.path("started")).unwrap();
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", &pid])
            .output()
            .unwrap()
            .status
            .success()
    );
}
#[cfg(unix)]
#[test]
fn configured_helpers_are_disabled_and_inspection_does_not_write_repository() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    let script = r.path("helper");
    fs::write(&script,"#!/usr/bin/python3\nimport sys\nopen('helper-was-run','w').write('bad')\nsys.stdout.buffer.write(sys.stdin.buffer.read())\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    for setting in [
        "diff.external",
        "diff.custom.textconv",
        "filter.custom.clean",
        "filter.custom.process",
        "core.fsmonitor",
    ] {
        r.git(&["config", setting, script.to_str().unwrap()]);
    }
    r.git(&["config", "filter.custom.required", "true"]);
    r.write(".gitattributes", "a diff=custom filter=custom\n");
    r.write("a", "working\n");
    let index = fs::read(r.path(".git/index")).unwrap();
    let s = r.inspect("a", Comparison::Head);
    assert!(s.filters_disabled);
    assert!(s.summary().contains("filters off"));
    assert_eq!(text(&s.diff.unwrap(), Kind::Add), ["working"]);
    assert!(!r.path("helper-was-run").exists());
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index);
    assert_eq!(fs::read(r.path("a")).unwrap(), b"working\n");
}
#[cfg(unix)]
#[test]
fn literal_pathspecs_and_non_utf8_names_are_not_reinterpreted() {
    use std::os::unix::ffi::OsStringExt;
    let r = Repo::new();
    r.write(":(glob)*", "before\n");
    let raw =
        r.0.path()
            .join(std::ffi::OsString::from_vec(b"-raw\xff\n".to_vec()));
    fs::write(&raw, b"before\n").unwrap();
    r.commit();
    r.write(":(glob)*", "after\n");
    fs::write(&raw, b"after\n").unwrap();
    assert_eq!(
        text(
            &r.inspect(":(glob)*", Comparison::Head).diff.unwrap(),
            Kind::Add
        ),
        ["after"]
    );
    let req = Request {
        path: raw.clone(),
        buffer: None,
        comparison: Comparison::Index,
        inspect: true,
        review: None,
        review_buffers: Vec::new(),
    };
    let s = git::inspect(&req, &Config::default()).unwrap().unwrap();
    assert!(s.entries.iter().any(|e| r.0.path().join(&e.path) == raw));
    assert_eq!(text(&s.diff.unwrap(), Kind::Add), ["after"]);
}
#[test]
fn content_and_patch_limits_report_unsupported_and_errors_are_visible_in_source() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    fs::write(r.path("a"), vec![b'x'; git::CONTENT_LIMIT + 1]).unwrap();
    assert!(
        r.inspect("a", Comparison::Head)
            .diff
            .unwrap_err()
            .contains("exceeds 2 MiB")
    );
    let patch = format!("@@ -0,0 +1,100001 @@\n{}", "+x\n".repeat(100001));
    assert!(Diff::parse(&patch).unwrap_err().contains("100,000"));
    assert!(Diff::parse("@@ -1 +1 @@\n-old\n").is_err());
    r.write("a", "base\n");
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.execute_command("gitbase no-such-base").unwrap();
    settle_editor(&mut e);
    assert!(draw(&mut e, 140, 8).contains("Unavailable revision"));
    assert!(e.git_mark(0).is_none());
    e.execute_command("gitbase HEAD").unwrap();
    let screen = draw(&mut e, 80, 8);
    assert!(screen.contains("loading"));
}
#[test]
fn selected_revision_empty_file_crlf_and_final_newline_metadata_render() {
    let r = Repo::new();
    r.write("a", "e\u{301}界");
    r.commit();
    r.write("a", "e\u{301}界\r\n");
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.execute_command("diff HEAD").unwrap();
    settle_editor(&mut e);
    let output = draw(&mut e, 140, 12);
    assert!(output.contains("[no final newline]"));
    assert!(output.contains("[CRLF]"));
}

#[test]
fn dirty_document_remains_a_source_destination_after_parent_directory_deletion() {
    let r = Repo::new();
    r.write("sub/a", "base\n");
    r.write("other", "other\n");
    r.commit();
    let mut e = Editor::new(Document::open(&r.path("sub/a")).unwrap());
    e.document.buffer.insert("unsaved ");
    e.document.buffer.commit();
    fs::remove_dir_all(r.path("sub")).unwrap();
    e.open_buffer(&r.path("other")).unwrap();
    e.execute_command("changes").unwrap();
    settle_editor(&mut e);
    let selected = e
        .changed_files()
        .iter()
        .position(|f| f.path == r.path("sub/a"))
        .unwrap();
    e.git.selected = selected;
    e.handle_key(Key::Enter);
    assert_eq!(e.active_buffer_id, 1);
    assert!(e.document.buffer.is_modified());
    e.execute_command("diff").unwrap();
    settle_editor(&mut e);
    assert!(e.git.diff().unwrap().source_available);
    e.jump_diff_source().unwrap();
    assert_eq!(e.active_buffer_id, 1);
    assert_eq!(e.document.buffer.text().to_string(), "unsaved base\n");
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "base\n");
    assert!(!r.path("sub").exists());
}
