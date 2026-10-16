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
            review_generation: 0,
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
        review_generation: 0,
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
    editor::{Editor, Mode, View, Viewport},
    file::Document,
    input::Key,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
fn keys(editor: &mut Editor, input: &str) {
    for character in input.chars() {
        editor.handle_key(Key::Char(character));
    }
}
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
fn status_line(editor: &mut Editor, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 4)).unwrap();
    terminal
        .draw(|f| ui::render(f, editor, &Theme::night()))
        .unwrap();
    let mut line = String::new();
    let mut x = 0;
    while x < width {
        let symbol = terminal.backend().buffer()[(x, 2)].symbol();
        line.push_str(symbol);
        x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
    }
    line
}
#[test]
fn status_path_is_repository_relative_and_stable_during_refresh() {
    let r = Repo::new();
    let relative = "Gjensidige.Alarm.Bff.Api/DTO/ReferenceDataDto.cs";
    r.write(relative, "// original\r\n");
    let mut e = Editor::new(Document::open(&r.path(relative)).unwrap());
    let absolute = e.document.path().unwrap().to_path_buf();
    assert!(status_line(&mut e, 400).contains(&absolute.display().to_string()));
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).starts_with(&format!(" NORMAL  [B1] {relative} ")));

    e.document.buffer.insert("// local\r\n");
    e.document.buffer.commit();
    e.refresh_git();
    for pending in [true, false] {
        if !pending {
            settle_editor(&mut e);
        }
        let status = status_line(&mut e, 400);
        assert!(
            status.starts_with(&format!(" NORMAL  [B1] {relative} [+] ")),
            "{status}"
        );
        assert!(status.contains("CRLF"));
        assert_eq!(e.document.path(), Some(absolute.as_path()));
    }
    // A failed comparison still has a valid repository root.
    e.execute_command("gitbase missing-revision").unwrap();
    settle_editor(&mut e);
    assert!(e.git.problem().is_some());
    assert!(status_line(&mut e, 400).starts_with(&format!(" NORMAL  [B1] {relative} [+] ")));
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "// original\r\n");
    assert_eq!(fs::read(&absolute).unwrap(), b"// original\r\n");
    fs::rename(r.path(".git"), r.path(".saved-git")).unwrap();
    e.refresh_git();
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).contains(&absolute.display().to_string()));
}
#[test]
fn status_path_drops_whole_outer_directories_when_space_is_limited() {
    let r = Repo::new();
    let short = "Gjensidige.Alarm.Bff.Notifications/Functions/NotificationOutboxFunction.cs";
    let relative = format!("Gjensidige.Alarm.Bff/{short}");
    r.write(&relative, "// original\r\n");
    r.commit();
    let mut e = Editor::new(Document::open(&r.path(&relative)).unwrap());
    settle_editor(&mut e);
    let absolute = e.document.path().unwrap().to_path_buf();

    for width in [195, 400, 195] {
        let expected = if width == 400 { &relative } else { short };
        let status = status_line(&mut e, width);
        assert!(
            status.starts_with(&format!(" NORMAL  [B1] {expected} ")),
            "{width}: {status}"
        );
        for label in ["HEAD→buffer", "main", "LSP", "C# UTF-8 CRLF", "1:1"] {
            assert!(status.contains(label), "{width}: {status}");
        }
    }

    e.document.buffer.insert("// local\r\n");
    e.document.buffer.commit();
    e.refresh_git();
    for pending in [true, false] {
        if !pending {
            settle_editor(&mut e);
        }
        let status = status_line(&mut e, 190);
        assert!(
            status.starts_with(&format!(" NORMAL  [B1] {short} [+] ")),
            "{status}"
        );
    }
    assert_eq!(e.document.path(), Some(absolute.as_path()));
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "// original\r\n");
    assert_eq!(fs::read(absolute).unwrap(), b"// original\r\n");
}
#[test]
fn status_path_does_not_reuse_a_previous_buffers_repository() {
    let r = Repo::new();
    r.write("outer.txt", "outer\n");
    r.write("nested/src/inner.txt", "inner\n");
    command(&r.path("nested"), &["init", "-b", "main"]);
    let mut e = Editor::new(Document::open(&r.path("outer.txt")).unwrap());
    settle_editor(&mut e);
    e.open_buffer(&r.path("nested/src/inner.txt")).unwrap();
    let absolute = e.document.display_name();
    assert!(status_line(&mut e, 400).contains(&absolute));
    e.tick();
    let status = status_line(&mut e, 400);
    assert!(
        status.contains(&absolute) || status.starts_with(" NORMAL  [B2] src/inner.txt "),
        "{status}"
    );
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).starts_with(" NORMAL  [B2] src/inner.txt "));

    let other = Repo::new();
    other.write("other.txt", "other\n");
    e.open_buffer(&other.path("other.txt")).unwrap();
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).starts_with(" NORMAL  [B3] other.txt "));

    let outside = tempdir().unwrap();
    let path = outside.path().join("outside.txt");
    fs::write(&path, "outside\n").unwrap();
    e.open_buffer(&path).unwrap();
    let absolute = e.document.display_name();
    assert!(status_line(&mut e, 400).contains(&absolute));
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).contains(&absolute));
    e.new_buffer();
    assert!(status_line(&mut e, 400).contains("[No Name]"));
    e.switch_buffer(1).unwrap();
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).starts_with(" NORMAL  [B1] outer.txt "));
}
#[test]
fn status_path_uses_linked_worktree_root_and_follows_save_as() {
    let r = Repo::new();
    r.write("src/original.txt", "original\r\n");
    r.commit();
    let parent = tempdir().unwrap();
    let linked = parent.path().join("linked worktree");
    r.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
    let mut e = Editor::new(Document::open(&linked.join("src/original.txt")).unwrap());
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).starts_with(" NORMAL  [B1] src/original.txt "));
    let saved = linked.join("src/saved.txt");
    e.execute_command(&format!("w {}", saved.display()))
        .unwrap();
    let absolute = e.document.display_name();
    assert!(status_line(&mut e, 400).contains(&absolute));
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).starts_with(" NORMAL  [B1] src/saved.txt "));
    assert_eq!(fs::read(saved).unwrap(), b"original\r\n");
    // Newly named files need not exist on disk to have a repository-relative path.
    e.open_buffer(&linked.join("src/new.txt")).unwrap();
    settle_editor(&mut e);
    assert!(status_line(&mut e, 400).starts_with(" NORMAL  [B2] src/new.txt "));
    assert!(!linked.join("src/new.txt").exists());
}
#[test]
fn status_path_preserves_unicode_and_safe_clipping() {
    let r = Repo::new();
    let relative = "outer directory/界 e\u{301} 👩‍💻/src/界 e\u{301} 👩‍💻\nfile.txt";
    r.write(relative, "unchanged\r\n");
    let mut e = Editor::new(Document::open(&r.path(relative)).unwrap());
    settle_editor(&mut e);
    let status = status_line(&mut e, 400);
    assert!(
        status.starts_with(
            " NORMAL  [B1] outer directory/界 e\u{301} 👩‍💻/src/界 e\u{301} 👩‍💻�file.txt "
        ),
        "{status}"
    );
    for width in 1..=160 {
        let status = status_line(&mut e, width);
        assert!(!status.chars().any(char::is_control));
        assert!(unicode_width::UnicodeWidthStr::width(status.as_str()) <= usize::from(width));
        if width >= 40 {
            let name = status.strip_prefix(" NORMAL  [B1] ").unwrap().trim_end();
            if name.contains('/') {
                assert!(
                    [
                        relative,
                        "界 e\u{301} 👩‍💻/src/界 e\u{301} 👩‍💻\nfile.txt",
                        "src/界 e\u{301} 👩‍💻\nfile.txt"
                    ]
                    .iter()
                    .any(|suffix| name.starts_with(&suffix.replace('\n', "�"))),
                    "{width}: {status}"
                );
            }
        }
    }
    assert_eq!(e.document.buffer.text().to_string(), "unchanged\r\n");
    assert_eq!(fs::read(r.path(relative)).unwrap(), b"unchanged\r\n");
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
fn literal_pathspecs_and_unusual_names_are_not_reinterpreted() {
    use std::os::unix::ffi::OsStringExt;
    let r = Repo::new();
    r.write(":(glob)*", "before\n");
    // macOS filesystems require UTF-8; retain delimiter and Unicode coverage.
    let name = std::ffi::OsString::from_vec(if cfg!(target_os = "macos") {
        "-raw界\n".as_bytes().to_vec()
    } else {
        b"-raw\xff\n".to_vec()
    });
    let raw = r.0.path().join(name);
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
        review_generation: 0,
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

#[test]
fn normal_hunk_shortcuts_use_changed_lines_counts_boundaries_and_history() {
    let r = Repo::new();
    let original = (0..70).map(|n| format!("row{n}\r\n")).collect::<String>();
    r.write("a", &original);
    r.commit();
    let changed = original
        .replace("row6\r\n", "界e\u{301}\r\n")
        .replace("row30\r\n", "middle\r\n")
        .replace("row58\r\n", "last\r\n");
    r.write("a", &changed);
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    settle_editor(&mut e);
    let generation = e.document.buffer.generation();
    e.register.text = "saved register".into();
    keys(&mut e, "]c");
    assert_eq!(e.document.buffer.position(), (6, 0));
    assert_eq!(e.view, View::Editing);
    assert_eq!(e.mode, Mode::Normal);
    let location = e.document.buffer.cursor();
    e.viewport = Viewport {
        top: 4,
        left: 2,
        height: 8,
    };
    let viewport = e.viewport.clone();
    keys(&mut e, "2]c");
    assert_eq!(e.document.buffer.position(), (58, 0));
    assert_eq!(e.message, "Git hunk 3/3");
    keys(&mut e, "]c");
    assert_eq!(e.document.buffer.position(), (58, 0));
    assert_eq!(e.message, "No next Git hunk");
    assert!(!e.message_is_error);
    e.handle_key(Key::Ctrl('o'));
    assert_eq!(e.document.buffer.cursor(), location);
    assert_eq!(e.viewport, viewport);
    settle_editor(&mut e);
    keys(&mut e, "99]c99[c");
    assert_eq!(e.document.buffer.position(), (6, 0));
    keys(&mut e, "[c");
    assert_eq!(e.message, "No previous Git hunk");
    assert_eq!(e.document.buffer.generation(), generation);
    assert_eq!(e.register.text, "saved register");
    assert_eq!(e.document.buffer.text().to_string(), changed);
    assert!(!e.document.buffer.undo());
    assert_eq!(fs::read(r.path("a")).unwrap(), changed.as_bytes());
}

#[test]
fn git_shortcuts_respect_head_index_revision_and_unsaved_crlf_content() {
    let r = Repo::new();
    let original = (0..65).map(|n| format!("row{n}\r\n")).collect::<String>();
    r.write("a", &original);
    r.commit();
    let revision = r.git(&["rev-parse", "HEAD"]).trim().to_owned();
    let head = original.replace("row5\r\n", "committed\r\n");
    r.write("a", &head);
    r.commit();
    let staged = head.replace("row25\r\n", "staged\r\n");
    r.write("a", &staged);
    r.git(&["add", "a"]);
    let working = staged.replace("row45\r\n", "working\r\n");
    r.write("a", &working);
    let index_before = fs::read(r.path(".git/index")).unwrap();
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.document.buffer.goto_line(55, true);
    e.document.buffer.insert("界e\u{301} ");
    e.document.buffer.commit();
    let dirty = e.document.buffer.text().to_string();
    for (base, comparison, first, second) in [
        ("HEAD", Comparison::Head, 25, 45),
        ("index", Comparison::Index, 45, 55),
        (
            revision.as_str(),
            Comparison::Revision(revision.clone()),
            5,
            25,
        ),
    ] {
        e.execute_command(&format!("gitbase {base}")).unwrap();
        e.document.buffer.goto_line(0, false);
        settle_editor(&mut e);
        keys(&mut e, "]c");
        assert_eq!(e.document.buffer.position(), (first, 0), "{base}");
        keys(&mut e, "]c");
        assert_eq!(e.document.buffer.position(), (second, 0), "{base}");
        let origin = e.document.buffer.cursor();
        keys(&mut e, " gd");
        settle_editor(&mut e);
        assert_eq!(e.view, View::Diff);
        assert_eq!(e.git.comparison, comparison);
        assert!(e.git.diff().unwrap().right_label.contains("UNSAVED"));
        assert!(draw(&mut e, 180, 15).contains("[CRLF]"));
        e.handle_key(Key::Escape);
        assert_eq!(e.document.buffer.cursor(), origin);
        assert_eq!(e.git.comparison, comparison);
        assert_eq!(e.document.buffer.text().to_string(), dirty);
    }
    assert_eq!(fs::read(r.path("a")).unwrap(), working.as_bytes());
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index_before);
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), working);
    assert!(e.document.buffer.redo());
    assert_eq!(e.document.buffer.text().to_string(), dirty);
}

#[test]
fn normal_hunk_shortcuts_anchor_deletions_and_handle_empty_or_deleted_source() {
    for (old, new, start, shortcut, expected) in [
        ("removed\r\nkeep\r\nlast", "keep\r\nlast", 1, "[c", 0),
        ("first\nremoved\nlast", "first\nlast", 0, "]c", 1),
        ("first\nkeep\ntail", "first\nkeep", 0, "]c", 1),
        ("only", "", 0, "]c", 0),
    ] {
        let r = Repo::new();
        r.write("a", old);
        r.commit();
        r.write("a", new);
        let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
        e.document.buffer.goto_line(start, false);
        settle_editor(&mut e);
        keys(&mut e, shortcut);
        assert_eq!(e.document.buffer.position(), (expected, 0), "{old:?}");
        assert_eq!(e.view, View::Editing);
        assert_eq!(e.document.buffer.text().to_string(), new);
        assert!(!e.document.buffer.undo());
    }
    let r = Repo::new();
    r.write("a", "history\n");
    r.commit();
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    fs::remove_file(r.path("a")).unwrap();
    settle_editor(&mut e);
    keys(&mut e, "]c");
    assert!(e.message.contains("Deleted file has no current source"));
    keys(&mut e, " gd");
    settle_editor(&mut e);
    assert_eq!(text(e.git.diff().unwrap(), Kind::Delete), ["history"]);
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.text().to_string(), "history\n");
    assert!(!r.path("a").exists());
}

#[test]
fn changed_file_shortcuts_wrap_count_and_preserve_dirty_buffers_and_diff_origin() {
    let r = Repo::new();
    r.write("a", "a\n");
    r.write("b", "b\nsecond\n");
    r.write("deleted", "history\n");
    r.write("unchanged", "same\n");
    r.commit();
    r.write("b", "changed\nsecond\n");
    fs::remove_file(r.path("deleted")).unwrap();
    r.write("z", "untracked\n");
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    keys(&mut e, "iunsaved ");
    e.handle_key(Key::Escape);
    let dirty_cursor = e.document.buffer.cursor();
    settle_editor(&mut e);
    keys(&mut e, "]f");
    assert_eq!(e.document.path(), Some(r.path("b").as_path()));
    e.document.buffer.goto_line(1, false);
    e.viewport = Viewport {
        top: 1,
        left: 2,
        height: 6,
    };
    let origin = (e.document.buffer.cursor(), e.viewport.clone());
    settle_editor(&mut e);
    keys(&mut e, "]f");
    settle_editor(&mut e);
    assert_eq!(e.view, View::Diff);
    assert_eq!(e.git.diff_view.as_ref().unwrap().path, r.path("deleted"));
    e.handle_key(Key::Char('n'));
    settle_editor(&mut e);
    assert_eq!(e.git.diff_view.as_ref().unwrap().path, r.path("z"));
    for (width, height) in [(130, 15), (40, 6), (1, 1), (1, 3)] {
        e.handle_key(Key::Tab);
        draw(&mut e, width, height);
        assert_eq!(e.document.buffer.cursor(), origin.0);
        assert_eq!(e.viewport, origin.1);
    }
    e.handle_key(Key::Escape);
    assert_eq!(e.document.path(), Some(r.path("b").as_path()));
    assert_eq!(e.document.buffer.cursor(), origin.0);
    assert_eq!(e.viewport, origin.1);
    settle_editor(&mut e);
    keys(&mut e, "2]f");
    assert_eq!(e.document.path(), Some(r.path("z").as_path()));
    settle_editor(&mut e);
    keys(&mut e, "]f");
    assert_eq!(e.document.path(), Some(r.path("a").as_path()));
    assert_eq!(e.active_buffer_id, 1);
    assert_eq!(e.document.buffer.cursor(), dirty_cursor);
    assert_eq!(e.document.buffer.text().to_string(), "unsaved a\n");
    settle_editor(&mut e);
    keys(&mut e, "[f");
    assert_eq!(e.document.path(), Some(r.path("z").as_path()));
    settle_editor(&mut e);
    keys(&mut e, "3[f");
    assert_eq!(e.document.path(), Some(r.path("a").as_path()));
    e.open_buffer(&r.path("unchanged")).unwrap();
    settle_editor(&mut e);
    keys(&mut e, "2[f");
    assert_eq!(e.git.diff_view.as_ref().unwrap().path, r.path("deleted"));
    e.handle_key(Key::Escape);
    e.switch_buffer(1).unwrap();
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), "a\n");
    assert_eq!(fs::read(r.path("a")).unwrap(), b"a\n");
    assert!(!r.path("deleted").exists());
}

#[test]
fn git_shortcuts_reject_stale_data_without_deferred_jumps_or_lost_input() {
    let r = Repo::new();
    r.write("a", "first\nold\n");
    r.write("b", "other\n");
    r.commit();
    r.write("a", "first\nnew\n");
    r.write("b", "changed\n");
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    settle_editor(&mut e);
    // Mutation can precede the next worker tick while ready() is still true.
    e.document.buffer.insert("unsaved\n");
    e.document.buffer.commit();
    assert!(e.git.ready());
    assert!(!e.git_current());
    let origin = e.document.buffer.cursor();
    for shortcut in ["]c", "[c", "]f", "[f"] {
        keys(&mut e, shortcut);
        assert!(e.message.contains("stale or loading"));
        assert_eq!(e.document.buffer.cursor(), origin);
        assert_eq!(e.active_buffer_id, 1);
        assert_eq!(e.view, View::Editing);
        assert!(e.pending_label().is_empty());
    }
    e.refresh_git();
    keys(&mut e, "]c");
    // Ordinary input remains usable while the comparison runs.
    keys(&mut e, "j");
    let moved = e.document.buffer.cursor();
    settle_editor(&mut e);
    assert_eq!(e.document.buffer.cursor(), moved);
    keys(&mut e, " gc");
    e.handle_key(Key::Enter);
    assert!(e.message.contains("stale or loading"));
    assert_eq!(e.view, View::Changes);
    settle_editor(&mut e);
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.cursor(), moved);
    // Even a base changed before invalidation must not reuse old coordinates.
    e.git.comparison = Comparison::Index;
    keys(&mut e, "]c");
    assert!(e.message.contains("stale or loading"));
    assert_eq!(e.document.buffer.cursor(), moved);
    settle_editor(&mut e);
    assert_eq!(e.document.buffer.cursor(), moved);
}

#[test]
fn git_shortcuts_report_no_changes_and_unavailable_comparisons() {
    let r = Repo::new();
    r.write("a", "unchanged\n");
    r.commit();
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    settle_editor(&mut e);
    for shortcut in ["]c", "[c"] {
        keys(&mut e, shortcut);
        assert!(e.message.contains("No Git hunks"));
        assert!(!e.message_is_error);
    }
    for shortcut in ["]f", "[f"] {
        keys(&mut e, shortcut);
        assert_eq!(e.message, "No changed files");
        assert!(!e.message_is_error);
    }
    e.execute_command("gitbase missing-revision").unwrap();
    settle_editor(&mut e);
    keys(&mut e, "]c");
    assert!(e.message.contains("Unavailable revision"));
    assert!(e.message_is_error);
    assert_eq!(e.view, View::Editing);
    assert_eq!(e.document.buffer.cursor(), 0);
    assert!(!e.document.buffer.undo());
    let outside = tempdir().unwrap();
    let path = outside.path().join("outside");
    fs::write(&path, "outside\n").unwrap();
    e.open_buffer(&path).unwrap();
    settle_editor(&mut e);
    keys(&mut e, "]f");
    assert!(e.message.contains("outside repository"));
    e.new_buffer();
    keys(&mut e, " gd");
    assert!(e.message.contains("Name a file"));
    assert_eq!(e.view, View::Editing);
}

#[test]
fn git_leader_shortcuts_show_help_and_restore_source_after_browsing() {
    let r = Repo::new();
    let original = (0..80).map(|n| format!("row{n}\n")).collect::<String>();
    r.write("a", &original);
    r.write("b", "other\n");
    r.commit();
    r.write("b", "changed\n");
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    e.document.buffer.goto_line(40, true);
    e.document.buffer.insert("界");
    e.document.buffer.commit();
    e.viewport = Viewport {
        top: 35,
        left: 2,
        height: 10,
    };
    let origin = (e.document.buffer.cursor(), e.viewport.clone());
    keys(&mut e, " g");
    assert_eq!(e.pending_label(), "Space g");
    assert!(e.message.contains("d diff"));
    assert!(e.message.contains("c changed files"));
    keys(&mut e, "d");
    settle_editor(&mut e);
    assert!(e.pending_label().is_empty());
    assert_eq!(e.view, View::Diff);
    e.handle_key(Key::Char('n'));
    settle_editor(&mut e);
    assert_eq!(e.git.diff_view.as_ref().unwrap().path, r.path("b"));
    let diff = e.git.diff_view.as_ref().unwrap();
    let diff_origin = (diff.selected, diff.top, diff.left, diff.paired);
    e.handle_key(Key::F1);
    keys(&mut e, "diff");
    e.handle_key(Key::Tab);
    e.handle_key(Key::Enter);
    draw(&mut e, 20, 5);
    e.handle_key(Key::Escape);
    assert_eq!(e.view, View::Diff);
    let diff = e.git.diff_view.as_ref().unwrap();
    assert_eq!(
        (diff.selected, diff.top, diff.left, diff.paired),
        diff_origin
    );
    for (width, height) in [(130, 20), (40, 8), (1, 1), (1, 3)] {
        draw(&mut e, width, height);
        assert_eq!(e.document.buffer.cursor(), origin.0);
        assert_eq!(e.viewport, origin.1);
    }
    e.handle_key(Key::Escape);
    keys(&mut e, " gc");
    settle_editor(&mut e);
    assert_eq!(e.view, View::Changes);
    keys(&mut e, "j");
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.cursor(), origin.0);
    assert_eq!(e.viewport, origin.1);
    e.execute_command("help git diff").unwrap();
    assert!(draw(&mut e, 80, 12).contains("Space g d"));
    e.handle_key(Key::Escape);
    assert!(e.document.buffer.undo());
    assert_eq!(e.document.buffer.text().to_string(), original);
}
