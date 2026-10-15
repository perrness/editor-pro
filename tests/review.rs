use editor_pro::{
    diff::Diff,
    git::{self, BufferVersion, Comparison, Config, Request},
    review::{Freshness, Record, State},
};
use ropey::Rope;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::{TempDir, tempdir};

struct Repo(TempDir);
impl Repo {
    fn new() -> Self {
        let r = Self(tempdir().unwrap());
        r.git(&["init", "-b", "main"]);
        r.git(&["config", "user.name", "Review Test"]);
        r.git(&["config", "user.email", "review@example.invalid"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r.git(&["config", "core.autocrlf", "false"]);
        r
    }
    fn path(&self, p: &str) -> PathBuf {
        self.0.path().join(p)
    }
    fn write(&self, p: &str, value: &str) {
        fs::write(self.path(p), value).unwrap();
    }
    fn git(&self, args: &[&str]) -> String {
        git(self.0.path(), args)
    }
    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-m", "fixture"]);
    }
    fn diff(&self, p: &str, base: Comparison) -> Diff {
        self.inspect(Request {
            path: self.path(p),
            buffer: None,
            comparison: base,
            inspect: true,
            review: None,
            review_buffers: Vec::new(),
            review_generation: 0,
        })
    }
    fn inspect(&self, req: Request) -> Diff {
        git::inspect(&req, &Config::default())
            .unwrap()
            .unwrap()
            .diff
            .unwrap()
    }
}
fn git(root: &Path, args: &[&str]) -> String {
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
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn mark(diff: &Diff, row: usize, kind: &str) -> Record {
    let (attachment, context) = diff.attachment(row, kind).unwrap();
    Record {
        id: 1,
        revision: 1,
        identity: diff.identity.clone().unwrap(),
        attachment,
        context,
        state: Some(State::Reviewed),
        note: None,
        unsaved: false,
    }
}
#[test]
fn content_not_stat_or_status_controls_freshness_and_exact_revert_is_valid() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    r.write("a", "next\n");
    let before = r.diff("a", Comparison::Head);
    let rec = mark(&before, 0, "file");
    let stamp = fs::metadata(r.path("a")).unwrap().modified().unwrap();
    r.write("a", "same\n");
    fs::File::options()
        .write(true)
        .open(r.path("a"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(stamp))
        .unwrap();
    assert_eq!(
        rec.freshness(Some(&r.diff("a", Comparison::Head))),
        Freshness::Stale
    );
    assert_eq!(rec.freshness(None), Freshness::Unavailable);
    r.write("a", "next\n");
    assert_eq!(
        rec.freshness(Some(&r.diff("a", Comparison::Head))),
        Freshness::Fresh
    );
}
#[test]
fn either_side_index_and_moving_refs_invalidate_even_with_unchanged_new_bytes() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    r.git(&["branch", "review-base"]);
    r.write("a", "next\n");
    let head = mark(&r.diff("a", Comparison::Head), 0, "file");
    let index = mark(&r.diff("a", Comparison::Index), 0, "file");
    let rev = mark(
        &r.diff("a", Comparison::Revision("review-base".into())),
        0,
        "file",
    );
    r.git(&["add", "a"]);
    assert_eq!(
        index.freshness(Some(&r.diff("a", Comparison::Index))),
        Freshness::Stale
    );
    r.git(&["commit", "-m", "moved"]);
    r.git(&["branch", "-f", "review-base", "HEAD"]);
    assert_eq!(
        head.freshness(Some(&r.diff("a", Comparison::Head))),
        Freshness::Stale
    );
    assert_eq!(
        rev.freshness(Some(
            &r.diff("a", Comparison::Revision("review-base".into()))
        )),
        Freshness::Stale
    );
    r.write("a", "more\n");
    r.git(&["add", "a"]);
    let staged = mark(&r.diff("a", Comparison::Staged), 0, "file");
    r.write("a", "last\n");
    r.git(&["add", "a"]);
    assert_eq!(
        staged.freshness(Some(&r.diff("a", Comparison::Staged))),
        Freshness::Stale
    );
}
#[test]
fn endings_presence_and_unsaved_bytes_are_exact() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    r.write("a", "new\r\n");
    let rec = mark(&r.diff("a", Comparison::Head), 0, "file");
    for text in ["new\n", "new", "", "new\r"] {
        r.write("a", text);
        assert_eq!(
            rec.freshness(Some(&r.diff("a", Comparison::Head))),
            Freshness::Stale
        );
    }
    r.write("a", "");
    let empty = r.diff("a", Comparison::Head).identity.unwrap();
    fs::remove_file(r.path("a")).unwrap();
    let missing = r.diff("a", Comparison::Head).identity.unwrap();
    assert_ne!(empty, missing);
    assert_eq!(empty.new.sha256, missing.new.sha256);
    assert_ne!(empty.new.present, missing.new.present);
    let req = |text, generation| Request {
        path: r.path("a"),
        buffer: Some(BufferVersion {
            id: 3,
            generation,
            dirty: true,
            text: Rope::from_str(text),
        }),
        comparison: Comparison::Head,
        inspect: true,
        review: None,
        review_buffers: Vec::new(),
        review_generation: 0,
    };
    let unsaved = r.inspect(req("new\r\n", 1));
    assert_eq!(rec.freshness(Some(&unsaved)), Freshness::Fresh);
    assert_eq!(
        rec.freshness(Some(&r.inspect(req("new\r\n", 100)))),
        Freshness::Fresh
    );
    assert_eq!(
        rec.freshness(Some(&r.inspect(req("edit\r\n", 101)))),
        Freshness::Stale
    );
    assert!(!r.path("a").exists());
}
#[test]
fn hunk_and_side_keys_ignore_navigation_but_reject_repetition() {
    let r = Repo::new();
    r.write("a", "old\n");
    r.commit();
    r.write("a", "new\n");
    let mut diff = r.diff("a", Comparison::Head);
    let rec = mark(&diff, 0, "hunk");
    let (old, _) = diff.attachment(1, "old").unwrap();
    let (new, _) = diff.attachment(2, "new").unwrap();
    assert_ne!(old, new);
    assert!(diff.attachment(1, "new").is_err());
    for row in &mut diff.rows {
        row.anchor += 90;
    }
    diff.identify_locations();
    assert_eq!(rec.freshness(Some(&diff)), Freshness::Fresh);
    let mut repeated =
        Diff::parse("@@ -1,1 +1,1 @@\n-old\n+new\n@@ -9,1 +9,1 @@\n-old\n+new\n").unwrap();
    repeated.identify_locations();
    assert!(repeated.attachment(0, "hunk").is_err());
    assert!(repeated.attachment(1, "old").is_err());
}
#[test]
fn identities_isolate_repositories_worktrees_bases_renames_and_untracked() {
    let r = Repo::new();
    r.write("old", "base\n");
    r.commit();
    let worktree = tempdir().unwrap();
    r.git(&[
        "worktree",
        "add",
        "-b",
        "other",
        worktree.path().to_str().unwrap(),
    ]);
    let rec = mark(&r.diff("old", Comparison::Head), 0, "file");
    let other = r.inspect(Request {
        path: worktree.path().join("old"),
        buffer: None,
        comparison: Comparison::Head,
        inspect: true,
        review: None,
        review_buffers: Vec::new(),
        review_generation: 0,
    });
    assert_eq!(rec.freshness(Some(&other)), Freshness::Stale);
    assert_ne!(
        rec.identity.repository.git_dir,
        other.identity.unwrap().repository.git_dir
    );
    assert_eq!(
        rec.freshness(Some(&r.diff("old", Comparison::Index))),
        Freshness::Stale
    );
    r.git(&["mv", "old", "new"]);
    let renamed = r.diff("new", Comparison::Head).identity.unwrap();
    assert_eq!(renamed.old_path, Path::new("old"));
    assert_eq!(renamed.new_path, Path::new("new"));
    r.write("untracked", "");
    let added = r.diff("untracked", Comparison::Head).identity.unwrap();
    assert!(!added.old.present);
    assert!(added.new.present);
    assert_ne!(added.old, added.new);
    let index = fs::read(r.path(".git/index")).unwrap();
    r.diff("untracked", Comparison::Head);
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index);
}

use editor_pro::review_store::{self, Change, Export};
use std::sync::Arc;
fn fixture() -> (Repo, Diff) {
    let r = Repo::new();
    r.write("a", "old\n");
    r.commit();
    r.write("a", "new\n");
    let diff = r.diff("a", Comparison::Head);
    (r, diff)
}
#[test]
fn store_is_lazy_durable_and_merges_independent_writers_with_cas_conflicts() {
    let (r, d) = fixture();
    let repo = &d.identity.as_ref().unwrap().repository;
    assert!(review_store::load(repo).unwrap().records.is_empty());
    assert!(!review_store::store_path(repo).exists());
    let source = fs::read(r.path("a")).unwrap();
    let index = fs::read(r.path(".git/index")).unwrap();
    let a = Change {
        expected: None,
        replacement: Some(mark(&d, 0, "file")),
    };
    let (store, id) = review_store::apply(repo, &a).unwrap();
    let first = store.records[0].clone();
    assert_eq!(id, first.id);
    let mut note = mark(&d, 1, "old");
    note.state = None;
    note.note = Some("界 👩‍💻 e\u{301}\nsecond `line`".into());
    let (store, _) = review_store::apply(
        repo,
        &Change {
            expected: None,
            replacement: Some(note),
        },
    )
    .unwrap();
    assert_eq!(store.records.len(), 2);
    assert!(
        review_store::apply(repo, &a)
            .unwrap_err()
            .contains("conflict")
    );
    let mut changed = first.clone();
    changed.state = Some(State::Attention);
    review_store::apply(
        repo,
        &Change {
            expected: Some(first.clone()),
            replacement: Some(changed),
        },
    )
    .unwrap();
    let before = fs::read(review_store::store_path(repo)).unwrap();
    assert!(
        review_store::apply(
            repo,
            &Change {
                expected: Some(first),
                replacement: None
            }
        )
        .unwrap_err()
        .contains("conflict")
    );
    assert_eq!(fs::read(review_store::store_path(repo)).unwrap(), before);
    let loaded = review_store::load(repo).unwrap();
    assert_eq!(loaded.records.len(), 2);
    assert_eq!(loaded.records[0].state, Some(State::Attention));
    assert!(loaded.records[1].note.as_ref().unwrap().contains("second"));
    assert_eq!(fs::read(r.path("a")).unwrap(), source);
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index);
    review_store::apply(
        repo,
        &Change {
            expected: Some(loaded.records[0].clone()),
            replacement: None,
        },
    )
    .unwrap();
    assert_eq!(review_store::load(repo).unwrap().records.len(), 1);
}
#[test]
fn corrupt_oversized_future_readonly_and_locked_stores_are_preserved() {
    let (_r, d) = fixture();
    let repo = &d.identity.as_ref().unwrap().repository;
    let path = review_store::store_path(repo);
    let change = Change {
        expected: None,
        replacement: Some(mark(&d, 0, "file")),
    };
    let mut future = review_store::load(repo).unwrap();
    future.version = 999;
    let mut unknown = serde_json::to_value(review_store::load(repo).unwrap()).unwrap();
    unknown["unrecognized-data"] = "preserve this".into();
    for bytes in [
        serde_json::to_vec(&unknown).unwrap(),
        b"broken json".to_vec(),
        serde_json::to_vec(&future).unwrap(),
        vec![b'x'; review_store::STORE_LIMIT as usize + 1],
    ] {
        fs::write(&path, &bytes).unwrap();
        assert!(review_store::load(repo).is_err());
        assert!(review_store::apply(repo, &change).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::remove_file(&path).unwrap();
    review_store::apply(repo, &change).unwrap();
    let before = fs::read(&path).unwrap();
    fs::write(path.with_extension("json.lock"), "another writer").unwrap();
    assert!(
        review_store::apply(repo, &change)
            .unwrap_err()
            .contains("lock")
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_file(path.with_extension("json.lock")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let old = review_store::load(repo).unwrap().records.remove(0);
    assert!(
        review_store::apply(
            repo,
            &Change {
                expected: Some(old),
                replacement: None
            }
        )
        .unwrap_err()
        .contains("read-only")
    );
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn export_is_deterministic_escaped_complete_and_never_clobbers_source_or_metadata() {
    let (r, d) = fixture();
    let repo = &d.identity.as_ref().unwrap().repository;
    let mut note = mark(&d, 1, "old");
    note.state = None;
    note.note = Some("# title\n```\n</pre><script>界\u{1b}[2J\n& *text*".into());
    let (store, id) = review_store::apply(
        repo,
        &Change {
            expected: None,
            replacement: Some(note),
        },
    )
    .unwrap();
    let dest = tempdir().unwrap();
    let mut export = Export {
        store: Arc::new(store),
        freshness: vec![(id, Freshness::Stale)],
        destination: dest.path().join("feedback.md"),
        protected: vec![],
        scope: "Current compared file only".into(),
    };
    let md = review_store::markdown(&export);
    assert_eq!(md, review_store::markdown(&export));
    assert!(md.contains("STALE"));
    assert!(md.contains("Old line: 1"));
    assert!(!md.contains("<script>"));
    assert!(!md.contains("```"));
    assert!(!md.contains('\u{1b}'));
    assert!(md.contains("界"));
    review_store::export(&export).unwrap();
    assert_eq!(fs::read_to_string(&export.destination).unwrap(), md);
    assert!(review_store::export(&export).is_err());
    assert_eq!(fs::read_to_string(&export.destination).unwrap(), md);
    for path in [
        r.path("a"),
        r.path("new.md"),
        review_store::store_path(repo),
    ] {
        export.destination = path;
        assert!(review_store::export(&export).is_err());
    }
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "new\n");
}
#[test]
fn queued_annotation_is_durable_when_worker_shuts_down() {
    let (_r, d) = fixture();
    let repo = &d.identity.as_ref().unwrap().repository;
    let w = review_store::Worker::new().unwrap();
    w.submit(review_store::Job::Change(
        repo.clone(),
        Box::new(Change {
            expected: None,
            replacement: Some(mark(&d, 0, "file")),
        }),
    ))
    .unwrap();
    drop(w);
    assert_eq!(review_store::load(repo).unwrap().records.len(), 1);
}

use editor_pro::{
    editor::{Editor, View, Viewport},
    file::Document,
    input::Key,
};
use std::{
    thread,
    time::{Duration, Instant},
};
fn settle(e: &mut Editor) {
    let start = Instant::now();
    loop {
        e.tick();
        if e.git_current() && !e.review.busy && e.review.store.is_some() {
            assert!(e.review.error.is_none(), "{}", e.review.status);
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(12),
            "{} / {}",
            e.git.summary(),
            e.review.status
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn wait_io(e: &mut Editor) {
    let start = Instant::now();
    while e.review.busy {
        e.tick();
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(1));
    }
}
fn editor(r: &Repo) -> Editor {
    let mut e = Editor::new(Document::open(&r.path("a")).unwrap());
    settle(&mut e);
    e
}
fn act(e: &mut Editor, command: &str) {
    e.execute_command(command).unwrap();
    settle(e);
}
fn records(e: &Editor) -> Vec<Record> {
    e.review.store.as_ref().unwrap().records.clone()
}
#[test]
fn editor_marks_transitions_staleness_revalidation_clear_and_exact_undo() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    assert!(!r.path(".git/editor-pro-review.json").exists());
    act(&mut e, "diff");
    act(&mut e, "review hunk reviewed");
    assert!(e.review_summary().contains("F0 H1"));
    act(&mut e, "review file reviewed");
    assert!(e.review_summary().contains("F1 H1"));
    act(&mut e, "review hunk attention");
    assert!(e.review_summary().contains("attention:1"));
    assert_eq!(records(&e).len(), 2);
    let file = records(&e)
        .into_iter()
        .find(|r| r.attachment == editor_pro::review::Attachment::File)
        .unwrap();
    e.close_diff();
    settle(&mut e);
    e.handle_key(Key::Char('i'));
    e.handle_key(Key::Char('x'));
    e.handle_key(Key::Escape);
    assert_ne!(e.review_freshness(&file), Freshness::Fresh);
    assert!(e.execute_command("review file reviewed").is_err()); // Before tick can accept an obsolete result.
    settle(&mut e);
    assert_eq!(e.review_freshness(&file), Freshness::Stale);
    e.handle_key(Key::Char('u'));
    assert_ne!(e.review_freshness(&file), Freshness::Fresh);
    settle(&mut e);
    assert_eq!(e.review_freshness(&file), Freshness::Fresh); // Explicit documented exact-byte undo policy.
    e.handle_key(Key::Ctrl('r'));
    settle(&mut e);
    assert_eq!(e.review_freshness(&file), Freshness::Stale);
    act(&mut e, &format!("review revalidate {} file", file.id));
    let updated = records(&e).into_iter().find(|r| r.id == file.id).unwrap();
    assert_eq!(e.review_freshness(&updated), Freshness::Fresh);
    assert_ne!(updated.identity, file.identity);
    act(&mut e, "review file unreviewed");
    assert!(e.review_summary().contains("F0"));
    act(&mut e, "review file clear");
    assert_eq!(records(&e).len(), 1);
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "new\n");
}
#[test]
fn unicode_multiline_note_uses_existing_editor_and_preserves_dirty_source_history() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    e.handle_key(Key::Char('i'));
    e.paste("unsaved 界\n");
    e.handle_key(Key::Escape);
    settle(&mut e);
    e.viewport = Viewport {
        top: 1,
        left: 2,
        height: 6,
    };
    let source = e.document.buffer.text().to_string();
    let cursor = e.document.buffer.cursor();
    let viewport = e.viewport.clone();
    e.register.text = "source register".into();
    act(&mut e, "diff");
    e.git.diff_view.as_mut().unwrap().selected = 1;
    e.execute_command("note old").unwrap();
    assert!(e.note_edit.is_some());
    assert!(e.execute_command("w /tmp/should-not-write-note").is_err());
    e.handle_key(Key::Char('i'));
    e.paste("界👩‍💻 e\u{301}\nsecond line `code`\n");
    e.handle_key(Key::Escape);
    e.handle_key(Key::Char('u'));
    e.handle_key(Key::Ctrl('r'));
    e.execute_command("note save").unwrap();
    assert!(e.execute_command("note cancel").is_err());
    settle(&mut e);
    assert!(e.note_edit.is_none());
    let note = records(&e).remove(0);
    assert!(note.note.as_ref().unwrap().contains("界👩‍💻"));
    assert_eq!(note.context.old_line, Some(0));
    assert_eq!(e.document.buffer.text().to_string(), source);
    assert_eq!(e.register.text, "source register");
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!(e.viewport, viewport);
    e.handle_key(Key::Char('u'));
    assert_eq!(e.document.buffer.text().to_string(), "new\n");
    e.handle_key(Key::Ctrl('r'));
    assert_eq!(e.document.buffer.text().to_string(), source);
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "new\n");
}
#[test]
fn notes_cover_file_hunk_new_and_deleted_side_edit_remove_and_explicit_correction() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    act(&mut e, "diff");
    for (target, row) in [("file", 0), ("hunk", 0), ("old", 1), ("new", 2)] {
        e.git.diff_view.as_mut().unwrap().selected = row;
        e.execute_command(&format!("note {target}")).unwrap();
        e.handle_key(Key::Char('i'));
        e.paste(target);
        e.handle_key(Key::Escape);
        act(&mut e, "note save");
    }
    assert_eq!(records(&e).len(), 4);
    let id = records(&e)[2].id;
    e.execute_command(&format!("note edit {id}")).unwrap();
    e.handle_key(Key::Char('A'));
    e.paste(" edited\n多行");
    e.handle_key(Key::Escape);
    act(&mut e, "note save");
    assert!(records(&e)[2].note.as_ref().unwrap().contains("edited"));
    r.write("a", "changed\n");
    act(&mut e, "e!");
    act(&mut e, "diff");
    assert!(
        records(&e)
            .iter()
            .all(|r| e.review_freshness(r) == Freshness::Stale)
    );
    act(&mut e, &format!("note attach {id} file"));
    let attached = records(&e).into_iter().find(|r| r.id == id).unwrap();
    assert_eq!(attached.attachment, editor_pro::review::Attachment::File);
    assert_eq!(e.review_freshness(&attached), Freshness::Fresh);
    act(&mut e, &format!("note remove {id}"));
    assert_eq!(records(&e).len(), 3);
}
#[test]
fn resume_validates_disk_and_unsaved_annotations_without_restoring_unsaved_source() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    act(&mut e, "review file reviewed");
    drop(e);
    let mut e = editor(&r);
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Fresh);
    e.handle_key(Key::Char('i'));
    e.paste("unsaved ");
    e.handle_key(Key::Escape);
    settle(&mut e);
    act(&mut e, "review file attention");
    assert_eq!(records(&e).len(), 2);
    drop(e);
    let e = editor(&r);
    assert_eq!(e.document.buffer.text().to_string(), "new\n");
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Fresh);
    assert_eq!(e.review_freshness(&records(&e)[1]), Freshness::Stale);
    assert!(records(&e)[1].unsaved);
}
#[test]
fn progress_validates_multiple_files_and_external_same_size_changes() {
    let r = Repo::new();
    r.write("a", "old\n");
    r.write("b", "old\n");
    r.commit();
    r.write("a", "new\n");
    r.write("b", "new\n");
    let mut e = editor(&r);
    act(&mut e, "review file reviewed");
    e.open_diff(r.path("b"), Comparison::Head);
    settle(&mut e);
    act(&mut e, "review file reviewed");
    assert!(e.review_summary().contains("F2"), "{}", e.review_summary());
    let stamp = fs::metadata(r.path("b")).unwrap().modified().unwrap();
    r.write("b", "bad\n");
    fs::File::options()
        .write(true)
        .open(r.path("b"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(stamp))
        .unwrap();
    act(&mut e, "gitrefresh");
    assert!(e.review_summary().contains("F1"));
    let b = records(&e)
        .into_iter()
        .find(|r| r.identity.new_path == Path::new("b"))
        .unwrap();
    assert_eq!(e.review_freshness(&b), Freshness::Stale);
    act(&mut e, "review next file unreviewed");
    assert_eq!(e.git.diff_view.as_ref().unwrap().path, r.path("b"));
}
#[test]
fn note_save_errors_keep_draft_and_source_and_quit_waits_for_durability() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    act(&mut e, "diff");
    e.execute_command("note file").unwrap();
    e.handle_key(Key::Char('i'));
    e.paste("keep this draft");
    e.handle_key(Key::Escape);
    let lock = r.path(".git/editor-pro-review.json.lock");
    fs::write(&lock, "busy").unwrap();
    e.execute_command("note save").unwrap();
    wait_io(&mut e);
    assert!(e.review.error.is_some());
    assert!(e.note_edit.is_some());
    assert_eq!(e.document.buffer.text().to_string(), "keep this draft");
    assert!(e.execute_command("q!").is_err());
    fs::remove_file(lock).unwrap();
    e.execute_command("note save").unwrap();
    settle(&mut e);
    assert!(e.note_edit.is_none());
    assert_eq!(e.document.buffer.text().to_string(), "new\n");
    e.execute_command("review file reviewed").unwrap();
    assert!(e.execute_command("q").is_err());
    assert!(!e.quit);
    settle(&mut e);
    e.execute_command("q").unwrap();
    assert!(e.quit);
}
#[test]
fn two_sessions_merge_and_conflict_without_losing_annotations() {
    let (r, _) = fixture();
    let mut a = editor(&r);
    let mut b = editor(&r);
    act(&mut a, "review file reviewed");
    act(&mut b, "diff");
    act(&mut b, "review hunk attention");
    assert_eq!(records(&b).len(), 2);
    act(&mut a, "review reload");
    act(&mut b, "review file attention");
    a.execute_command("review file unreviewed").unwrap();
    wait_io(&mut a);
    assert!(a.review.error.as_ref().unwrap().contains("conflict"));
    assert!(a.execute_command("q").is_err());
    act(&mut a, "review reload");
    assert_eq!(records(&a)[0].state, Some(State::Attention));
    assert_eq!(records(&a).len(), 2);
}
#[test]
fn review_render_navigation_and_layout_preserve_source_and_identical_freshness() {
    use editor_pro::{theme::Theme, ui};
    use ratatui::{Terminal, backend::TestBackend};
    let (r, _) = fixture();
    let mut e = editor(&r);
    act(&mut e, "diff");
    act(&mut e, "review hunk reviewed");
    act(&mut e, "review file reviewed");
    let original = e.document.buffer.cursor();
    for (width, height) in [(120, 24), (40, 9), (1, 1), (100, 20)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        e.handle_key(Key::Tab);
        e.handle_key(Key::Char('j'));
        terminal
            .draw(|f| ui::render(f, &mut e, &Theme::night()))
            .unwrap();
        assert!(
            records(&e)
                .iter()
                .all(|r| e.review_freshness(r) == Freshness::Fresh)
        );
    }
    act(&mut e, "review list");
    assert_eq!(e.view, View::Review);
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal
        .draw(|f| ui::render(f, &mut e, &Theme::night()))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(rendered.contains("fresh"));
    assert!(rendered.contains("reviewed"));
    e.handle_key(Key::Escape);
    assert_eq!(e.view, View::Diff);
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.cursor(), original);
}
#[test]
fn invalid_comparison_keeps_prior_records_unavailable_and_rejects_actions() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    act(&mut e, "review file reviewed");
    e.execute_command("diff missing-ref").unwrap();
    let start = Instant::now();
    loop {
        e.tick();
        if !e.git.loading {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Unavailable);
    assert!(e.execute_command("review file reviewed").is_err());
    assert!(e.execute_command("note file").is_err());
    assert_eq!(records(&e).len(), 1);
}

#[test]
fn queued_obsolete_comparison_cannot_restore_a_fresh_mark() {
    let (r, d) = fixture();
    let repo = d.identity.as_ref().unwrap().repository.clone();
    let (store, id) = review_store::apply(
        &repo,
        &Change {
            expected: None,
            replacement: Some(mark(&d, 0, "file")),
        },
    )
    .unwrap();
    let worker = git::Worker::new(Config {
        debounce: Duration::from_millis(25),
        ..Config::default()
    })
    .unwrap();
    let mut req = Request {
        path: r.path("a"),
        buffer: None,
        comparison: Comparison::Head,
        inspect: true,
        review: Some(Arc::new(store)),
        review_buffers: vec![],
        review_generation: 0,
    };
    let old = worker.submit(req.clone());
    req.buffer = Some(BufferVersion {
        id: 1,
        generation: 22,
        dirty: true,
        text: Rope::from_str("changed\n"),
    });
    let new = worker.submit(req);
    assert_ne!(old, new);
    let start = Instant::now();
    loop {
        if let Some(reply) = worker.poll() {
            assert_eq!(reply.serial, new);
            let snapshot = reply.result.unwrap().unwrap();
            assert_eq!(snapshot.review_freshness[&id].1, Freshness::Stale);
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn linked_worktree_persistence_and_comparison_scopes_never_share_progress() {
    let (r, d) = fixture();
    let repo = d.identity.as_ref().unwrap().repository.clone();
    let (store, _) = review_store::apply(
        &repo,
        &Change {
            expected: None,
            replacement: Some(mark(&d, 0, "file")),
        },
    )
    .unwrap();
    let wt = tempdir().unwrap();
    r.git(&[
        "worktree",
        "add",
        "-b",
        "second",
        wt.path().to_str().unwrap(),
    ]);
    fs::write(wt.path().join("a"), "new\n").unwrap();
    let other = r.inspect(Request {
        path: wt.path().join("a"),
        buffer: None,
        comparison: Comparison::Head,
        inspect: true,
        review: Some(Arc::new(store.clone())),
        review_buffers: vec![],
        review_generation: 0,
    });
    let other_repo = &other.identity.as_ref().unwrap().repository;
    assert!(review_store::load(other_repo).unwrap().records.is_empty());
    assert!(!review_store::store_path(other_repo).exists());
    review_store::apply(
        other_repo,
        &Change {
            expected: None,
            replacement: Some(mark(&other, 0, "file")),
        },
    )
    .unwrap();
    assert_eq!(review_store::load(&repo).unwrap(), store);
    let mut e = editor(&r);
    act(&mut e, "diff index");
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Unavailable);
    act(&mut e, "review file attention");
    assert_eq!(records(&e).len(), 2);
    act(&mut e, "diff HEAD");
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Fresh);
    assert_eq!(e.review_freshness(&records(&e)[1]), Freshness::Unavailable);
}
#[cfg(unix)]
#[test]
fn unusual_paths_multiline_names_and_note_control_text_roundtrip_safely() {
    use std::os::unix::ffi::OsStringExt;
    let r = Repo::new();
    r.write("a", "old\n");
    r.commit();
    // macOS filesystems require UTF-8; retain delimiter and Unicode coverage.
    let name = std::ffi::OsString::from_vec(if cfg!(target_os = "macos") {
        "-`odd`界\n*name*.rs".as_bytes().to_vec()
    } else {
        b"-`odd`\xff\n*name*.rs".to_vec()
    });
    let path = r.0.path().join(name);
    fs::write(&path, "new\n").unwrap();
    let d = r.inspect(Request {
        path: path.clone(),
        buffer: None,
        comparison: Comparison::Head,
        inspect: true,
        review: None,
        review_buffers: vec![],
        review_generation: 0,
    });
    let repo = &d.identity.as_ref().unwrap().repository;
    let mut note = mark(&d, 1, "new");
    note.state = None;
    note.note = Some("界\n\u{1b}]0;bad\u{7}\n</pre>```".into());
    let (store, id) = review_store::apply(
        repo,
        &Change {
            expected: None,
            replacement: Some(note),
        },
    )
    .unwrap();
    assert_eq!(review_store::load(repo).unwrap(), store);
    assert_eq!(
        store.records[0].identity.new_path,
        path.strip_prefix(r.0.path()).unwrap()
    );
    let dir = tempdir().unwrap();
    let export = Export {
        store: Arc::new(store),
        freshness: vec![(id, Freshness::Fresh)],
        destination: dir.path().join("feedback.md"),
        protected: vec![],
        scope: "Test".into(),
    };
    let md = review_store::markdown(&export);
    assert!(!md.contains('\u{1b}'));
    assert!(!md.contains("```"));
    if cfg!(target_os = "macos") {
        assert!(md.contains('界'));
    } else {
        assert!(md.contains("&#37;FF"));
    }
    assert_eq!(fs::read(&path).unwrap(), b"new\n");
}
#[test]
fn deleted_lines_notes_remain_historical_and_stale_notes_never_jump_by_ordinal() {
    let r = Repo::new();
    r.write("a", "old\n");
    r.write("gone", "deleted\n");
    r.commit();
    r.write("a", "new\n");
    fs::remove_file(r.path("gone")).unwrap();
    let mut e = editor(&r);
    let cursor = e.document.buffer.cursor();
    e.open_diff(r.path("gone"), Comparison::Head);
    settle(&mut e);
    e.git.diff_view.as_mut().unwrap().selected = 1;
    e.execute_command("note old").unwrap();
    e.handle_key(Key::Char('i'));
    e.paste("Explain removal");
    e.handle_key(Key::Escape);
    act(&mut e, "note save");
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Fresh);
    assert!(e.jump_diff_source().is_err());
    e.handle_key(Key::Escape);
    assert_eq!(e.document.buffer.cursor(), cursor);
    r.write("gone", "unrelated\n");
    act(&mut e, "gitrefresh");
    act(&mut e, "note next");
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Stale);
    assert!(e.message.contains("no automatic attachment"));
    assert_eq!(e.document.path(), Some(r.path("a").as_path()));
    assert_eq!(fs::read_to_string(r.path("gone")).unwrap(), "unrelated\n");
}
#[test]
fn bounded_context_and_note_limits_do_not_persist_an_unsaved_source_copy() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    e.handle_key(Key::Char('i'));
    e.paste(&"秘密".repeat(4000));
    e.handle_key(Key::Escape);
    settle(&mut e);
    act(&mut e, "diff");
    e.git.diff_view.as_mut().unwrap().selected = 2;
    e.execute_command("note new").unwrap();
    e.handle_key(Key::Char('i'));
    e.paste(&"x".repeat(editor_pro::review::MAX_NOTE + 1));
    assert!(e.document.buffer.text().len_bytes() <= editor_pro::review::MAX_NOTE);
    e.paste("short note");
    e.handle_key(Key::Escape);
    act(&mut e, "note save");
    assert!(records(&e)[0].context.excerpt.len() <= editor_pro::review::MAX_CONTEXT);
    let bytes = fs::read(r.path(".git/editor-pro-review.json")).unwrap();
    assert!(bytes.len() < 8000);
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "new\n");
}
#[cfg(unix)]
#[test]
fn executable_mode_changes_invalidate_and_export_symlinks_do_not_clobber() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (r, d) = fixture();
    let rec = mark(&d, 0, "file");
    fs::set_permissions(r.path("a"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        rec.freshness(Some(&r.diff("a", Comparison::Head))),
        Freshness::Stale
    );
    let repo = &d.identity.as_ref().unwrap().repository;
    let (store, _) = review_store::apply(
        repo,
        &Change {
            expected: None,
            replacement: Some(rec),
        },
    )
    .unwrap();
    let dir = tempdir().unwrap();
    let target = dir.path().join("target");
    fs::write(&target, "preserve").unwrap();
    let link = dir.path().join("feedback");
    symlink(&target, &link).unwrap();
    let export = Export {
        store: Arc::new(store),
        freshness: vec![],
        destination: link,
        protected: vec![],
        scope: "Test".into(),
    };
    assert!(review_store::export(&export).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
    let path = review_store::store_path(repo);
    fs::remove_file(&path).unwrap();
    symlink(r.path("a"), &path).unwrap();
    assert!(review_store::load(repo).is_err());
    assert!(
        review_store::apply(
            repo,
            &Change {
                expected: None,
                replacement: Some(mark(&d, 0, "file"))
            }
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "new\n");
}
#[test]
fn repeated_note_navigation_and_all_filters_have_explicit_empty_states() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    act(&mut e, "diff");
    for text in ["first note", "second note"] {
        e.execute_command("note file").unwrap();
        e.handle_key(Key::Char('i'));
        e.paste(text);
        e.handle_key(Key::Escape);
        act(&mut e, "note save");
    }
    act(&mut e, "note next");
    let first = e.review.focused_record;
    act(&mut e, "note next");
    assert_ne!(e.review.focused_record, first);
    act(&mut e, "note prev");
    assert_eq!(e.review.focused_record, first);
    assert!(
        e.execute_command("review next hunk attention")
            .unwrap_err()
            .to_string()
            .contains("No attention")
    );
    act(&mut e, "review list attention");
    assert!(e.review_records().is_empty());
    assert!(e.execute_command("review file reviewed").is_err());
    assert!(e.execute_command("note file").is_err());
    e.handle_key(Key::Enter);
    assert!(e.message.contains("No matching"));
}

#[test]
fn identical_hunks_in_different_files_do_not_share_displayed_marks() {
    use ratatui::{Terminal, backend::TestBackend};
    let r = Repo::new();
    r.write("a", "old\n");
    r.write("b", "old\n");
    r.commit();
    r.write("a", "new\n");
    r.write("b", "new\n");
    let mut e = editor(&r);
    act(&mut e, "diff");
    act(&mut e, "review hunk reviewed");
    e.open_diff(r.path("b"), Comparison::Head);
    settle(&mut e);
    let mut terminal = Terminal::new(TestBackend::new(140, 20)).unwrap();
    terminal
        .draw(|f| editor_pro::ui::render(f, &mut e, &editor_pro::theme::Theme::night()))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(rendered.contains("Hunk unreviewed"), "{rendered}");
    e.execute_command("note file").unwrap();
    e.handle_key(Key::Escape);
    assert!(e.note_edit.is_none());
    settle(&mut e);
    e.execute_command("note file").unwrap();
    e.handle_key(Key::Char('i'));
    e.paste("draft");
    e.handle_key(Key::Escape);
    e.handle_key(Key::Escape);
    assert!(e.note_edit.is_some());
    assert!(e.message.contains("Unsaved note"));
    e.execute_command("note cancel").unwrap();
    assert_eq!(e.view, View::Diff);
}
#[test]
fn obsolete_record_revision_cannot_borrow_new_record_validation() {
    let (r, _) = fixture();
    let mut e = editor(&r);
    act(&mut e, "review file reviewed");
    let old = records(&e)[0].clone();
    act(&mut e, "review file attention");
    assert_ne!(e.review_freshness(&old), Freshness::Fresh);
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Fresh);
}
#[test]
fn bounded_batch_prioritizes_current_path_and_reports_unvalidated_progress() {
    let r = Repo::new();
    r.write("a", "old\n");
    for n in 0..35 {
        r.write(&format!("b{n:02}"), "old\n");
    }
    r.commit();
    r.write("a", "new\n");
    for n in 0..35 {
        r.write(&format!("b{n:02}"), "new\n");
    }
    let d = r.diff("a", Comparison::Head);
    let base = mark(&d, 0, "file");
    let repo = &base.identity.repository;
    let mut store = editor_pro::review::Store::empty(repo.clone());
    for name in std::iter::once("a".into()).chain((0..35).map(|n| format!("b{n:02}"))) {
        let mut rec = base.clone();
        rec.id = store.next_id;
        rec.identity.old_path = name.clone().into();
        rec.identity.new_path = name.into();
        store.records.push(rec);
        store.next_id += 1;
    }
    fs::write(
        review_store::store_path(repo),
        serde_json::to_vec(&store).unwrap(),
    )
    .unwrap();
    let mut e = editor(&r);
    assert!(e.git.snapshot.as_ref().unwrap().review_limited);
    assert_eq!(
        records(&e)
            .iter()
            .filter(|r| e.review_freshness(r) == Freshness::Fresh)
            .count(),
        32
    );
    let last = records(&e).pop().unwrap();
    assert_eq!(e.review_freshness(&last), Freshness::Unavailable);
    e.open_diff(r.path("b34"), Comparison::Head);
    settle(&mut e);
    assert_eq!(e.review_freshness(&last), Freshness::Fresh);
    assert!(e.review_summary().contains("validation limited"));
}
#[test]
fn concurrent_store_writers_never_silently_lose_independent_records() {
    let (_r, d) = fixture();
    let repo = &d.identity.as_ref().unwrap().repository;
    let a = Change {
        expected: None,
        replacement: Some(mark(&d, 0, "file")),
    };
    let mut note = mark(&d, 0, "file");
    note.state = None;
    note.note = Some("concurrent note".into());
    let b = Change {
        expected: None,
        replacement: Some(note),
    };
    let barrier = std::sync::Barrier::new(2);
    let (one, two) = thread::scope(|scope| {
        let x = scope.spawn(|| {
            barrier.wait();
            review_store::apply(repo, &a)
        });
        let y = scope.spawn(|| {
            barrier.wait();
            review_store::apply(repo, &b)
        });
        (x.join().unwrap(), y.join().unwrap())
    });
    for (result, change) in [(one, &a), (two, &b)] {
        if let Err(e) = result {
            assert!(e.contains("lock"), "{e}");
            review_store::apply(repo, change).unwrap();
        }
    }
    let store = review_store::load(repo).unwrap();
    assert_eq!(store.records.len(), 2);
    assert_ne!(store.records[0].id, store.records[1].id);
}

#[test]
fn switching_repositories_cannot_discard_a_failed_pending_annotation() {
    let (a, _) = fixture();
    let (b, _) = fixture();
    let mut e = editor(&a);
    let lock = a.path(".git/editor-pro-review.json.lock");
    fs::write(&lock, "held").unwrap();
    e.execute_command("review file reviewed").unwrap();
    e.open_buffer(&b.path("a")).unwrap();
    wait_io(&mut e);
    assert!(e.review.error.as_ref().unwrap().contains("lock"));
    assert!(e.execute_command("q").is_err());
    fs::remove_file(lock).unwrap();
    e.execute_command("review retry").unwrap();
    wait_io(&mut e);
    settle(&mut e);
    assert!(records(&e).is_empty());
    e.open_buffer(&a.path("a")).unwrap();
    settle(&mut e);
    assert_eq!(records(&e).len(), 1);
    assert_eq!(e.review_freshness(&records(&e)[0]), Freshness::Fresh);
}
