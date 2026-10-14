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
