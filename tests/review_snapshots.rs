use editor_pro::{
    editor::{Editor, View},
    file::Document,
    git::{self, Comparison, Config, Request},
    input::Key,
    review::{Attachment, Freshness, Record},
    review_snapshot, review_store,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tempfile::{TempDir, tempdir};

struct Repo(TempDir);
impl Repo {
    fn new() -> Self {
        let r = Self(tempdir().unwrap());
        r.git(&["init", "-b", "main"]);
        r.git(&["config", "user.name", "Snapshot test"]);
        r.git(&["config", "user.email", "snapshot@example.invalid"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r.git(&["config", "core.autocrlf", "false"]);
        r
    }
    fn path(&self, p: &str) -> PathBuf {
        self.0.path().join(p)
    }
    fn write(&self, p: &str, content: &str) {
        fs::write(self.path(p), content).unwrap();
    }
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .current_dir(self.0.path())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
    fn commit(&self) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-m", "fixture"]);
    }
    fn editor(&self) -> Editor {
        let mut e = Editor::new(Document::open(&self.path("a")).unwrap());
        settle(&mut e);
        e
    }
}
fn settle(e: &mut Editor) {
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        e.tick();
        if !e.review.busy && e.git_current() && e.review.store.is_some() {
            assert!(e.review.error.is_none(), "{}", e.review.status);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} / {}",
            e.git.summary(),
            e.review.status
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn act(e: &mut Editor, command: &str) {
    e.execute_command(command).unwrap();
    settle(e);
}
fn records(e: &Editor) -> Vec<Record> {
    e.review.store.as_ref().unwrap().records.clone()
}
fn request(e: &Editor, path: &Path) -> Request {
    Request {
        path: path.into(),
        buffer: None,
        comparison: Comparison::Head,
        inspect: true,
        review: e.review.store.clone(),
        review_generation: 0,
        review_buffers: Vec::new(),
    }
}
fn capture(e: &Editor, path: &Path) -> (git::Repository, review_snapshot::Capture) {
    git::capture_review(
        &request(e, path),
        &Config {
            timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap()
}
fn text(e: &Editor) -> String {
    e.document.buffer.text().to_string()
}
fn hunk_text(e: &Editor) -> String {
    e.git
        .diff()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn checkpoints_are_immutable_durable_and_do_not_touch_sources_index_or_refs() {
    let r = Repo::new();
    r.write("a", "base\r\n界👩‍💻\r\n");
    r.write("empty", "");
    r.commit();
    r.write("a", "first\r\n界👩‍💻\r\n");
    r.write("new", "untracked");
    let mut e = r.editor();
    let index = fs::read(r.path(".git/index")).unwrap();
    let head = r.git(&["rev-parse", "HEAD"]);
    let status = r.git(&["status", "--porcelain=v2", "-z"]);
    act(&mut e, "review snapshot");
    let first = e.review.store.as_ref().unwrap().history.clone().unwrap();
    assert_eq!(first.latest(), 1);
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index);
    assert_eq!(r.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(r.git(&["status", "--porcelain=v2", "-z"]), status);
    assert_eq!(
        fs::read(r.path("a")).unwrap(),
        "first\r\n界👩‍💻\r\n".as_bytes()
    );
    act(&mut e, "review since");
    assert!(e.git.diff().unwrap().rows.is_empty());
    act(&mut e, "review full");
    assert!(hunk_text(&e).contains("base\r"));
    r.write("a", "second\r\n界👩‍💻");
    r.commit(); // Moving HEAD cannot alter the captured full base.
    act(&mut e, "gitrefresh");
    assert!(hunk_text(&e).contains("second"));
    act(&mut e, "review snapshot");
    let next = e.review.store.as_ref().unwrap().history.as_ref().unwrap();
    assert_eq!(next.baseline, first.baseline);
    assert_eq!(next.checkpoints[0], first.checkpoints[0]);
    act(&mut e, "review since 1");
    assert!(hunk_text(&e).contains("first"));
    assert!(hunk_text(&e).contains("second"));
    act(&mut e, "review since");
    assert!(e.git.diff().unwrap().rows.is_empty());
    drop(e);
    let mut resumed = r.editor();
    act(&mut resumed, "review full");
    assert!(hunk_text(&resumed).contains("base\r"));
    assert_eq!(resumed.review.store.as_ref().unwrap().version, 2);
    assert!(resumed.execute_command("review since 999").is_err());
}

#[test]
fn live_refresh_and_snapshot_capture_preserve_dirty_buffers_undo_and_viewports() {
    let r = Repo::new();
    r.write("a", "old\nkeep\n");
    r.write("b", "base\n");
    r.commit();
    let mut e = r.editor();
    e.handle_key(Key::Char('i'));
    e.paste("unsaved界\r\n");
    e.handle_key(Key::Escape);
    let unsaved = text(&e);
    let cursor = e.document.buffer.cursor();
    e.viewport.top = 1;
    e.viewport.left = 2;
    act(&mut e, "review snapshot");
    let history = e.review.store.as_ref().unwrap().history.as_ref().unwrap();
    assert_eq!(
        history.checkpoints[0]
            .files
            .iter()
            .find(|f| f.path == Path::new("a"))
            .unwrap()
            .text,
        unsaved
    );
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "old\nkeep\n");
    act(&mut e, "review since");
    assert!(e.git.diff().unwrap().rows.is_empty());
    r.write("a", "agent changed disk\n");
    r.write("b", "agent changed b\n");
    e.check_external();
    let deadline = Instant::now() + Duration::from_secs(5);
    while e.disk.busy {
        e.tick();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    settle(&mut e); // The disk monitor already invalidated the Git/review snapshot.
    assert!(e.git.diff().unwrap().rows.is_empty()); // Dirty text still owns this side.
    assert!(
        e.git
            .snapshot
            .as_ref()
            .unwrap()
            .external_paths
            .contains(&PathBuf::from("a"))
    );
    assert!(e.changed_files().iter().any(|f| f.path == r.path("b")));
    e.close_diff();
    assert!(
        e.execute_command("w")
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    assert_eq!(text(&e), unsaved);
    assert_eq!(e.document.buffer.cursor(), cursor);
    assert_eq!((e.viewport.top, e.viewport.left), (1, 2));
    e.handle_key(Key::Char('u'));
    assert_eq!(text(&e), "old\nkeep\n");
    assert_eq!(
        fs::read_to_string(r.path("a")).unwrap(),
        "agent changed disk\n"
    );
}

#[test]
fn clean_open_buffers_stay_intact_while_review_displays_same_size_disk_revisions() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    r.write("a", "next\n");
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    act(&mut e, "review full");
    act(&mut e, "review file reviewed");
    let file = records(&e)[0].clone();
    let modified = fs::metadata(r.path("a")).unwrap().modified().unwrap();
    r.write("a", "last\n");
    fs::File::options()
        .write(true)
        .open(r.path("a"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    // Exercise the ordinary two-second periodic refresh, with no explicit refresh.
    let deadline = Instant::now() + Duration::from_secs(8);
    while !hunk_text(&e).contains("last") {
        e.tick();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(e.review_freshness(&file), Freshness::Stale);
    assert_eq!(text(&e), "next\n");
    act(&mut e, "review since");
    assert!(hunk_text(&e).contains("next"));
    assert!(hunk_text(&e).contains("last"));
    e.jump_diff_source().unwrap();
    assert_eq!(text(&e), "next\n");
    assert!(!e.document.buffer.is_modified());
}

#[test]
fn disk_observations_invalidate_review_and_gutters_even_when_conflict_is_already_flagged() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    r.write("a", "next\n");
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    act(&mut e, "review full");
    act(&mut e, "review file reviewed");
    let record = records(&e)[0].clone();
    let index = fs::read(r.path(".git/index")).unwrap();
    for version in ["last\n", "more\n"] {
        r.write("a", version);
        e.check_external();
        let deadline = Instant::now() + Duration::from_secs(5);
        while e.disk.busy {
            e.tick();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        assert!(!e.git_current());
        assert_eq!(e.review_freshness(&record), Freshness::Unavailable);
        assert!(e.git_mark(0).is_none());
        settle(&mut e);
        assert!(hunk_text(&e).contains(version.trim()));
        assert_eq!(e.review_freshness(&record), Freshness::Stale);
        assert!(e.git_mark(0).is_none()); // Compared disk differs from retained text.
        assert_eq!(text(&e), "next\n");
        assert!(e.document.external_changed);
    }
    // The source behind a review note is monitored too; the draft stays intact.
    e.execute_command("note file").unwrap();
    e.document.buffer.insert("draft");
    r.write("a", "note-time revision\n");
    e.check_external();
    let deadline = Instant::now() + Duration::from_secs(5);
    while e.disk.busy {
        e.tick();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(text(&e), "draft");
    assert!(e.note_edit.is_some());
    e.execute_command("note cancel").unwrap();
    assert_eq!(text(&e), "next\n");
    assert!(e.document.external_changed);
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index);
}

#[test]
fn unique_hunks_and_notes_reanchor_but_changed_file_marks_and_ambiguous_content_stale() {
    let r = Repo::new();
    let base = (0..32).map(|n| format!("line {n}\n")).collect::<String>();
    r.write("a", &base);
    r.commit();
    let first = base.replace("line 4\n", "first change 界\n");
    r.write("a", &first);
    let mut e = r.editor();
    act(&mut e, "diff");
    act(&mut e, "review hunk reviewed"); // Existing annotations survive the first checkpoint.
    e.execute_command("note hunk").unwrap();
    e.handle_key(Key::Char('i'));
    e.paste("Keep this contract");
    e.handle_key(Key::Escape);
    act(&mut e, "note save");
    act(&mut e, "review snapshot");
    act(&mut e, "review full");
    act(&mut e, "review file reviewed");
    let before = records(&e);
    r.write("a", &first.replace("line 25\n", "later change\n"));
    act(&mut e, "gitrefresh");
    for record in &before {
        assert_eq!(
            e.review_freshness(record),
            if record.attachment == Attachment::File {
                Freshness::Stale
            } else {
                Freshness::Fresh
            }
        );
    }
    assert!(e.review_summary().contains("F0 H1"));
    act(&mut e, "review next hunk unreviewed");
    let selected = e.git.diff_view.as_ref().unwrap().selected;
    assert_eq!(selected, *e.git.diff().unwrap().hunks.last().unwrap());
    act(&mut e, "note next");
    assert_eq!(
        e.git.diff_view.as_ref().unwrap().selected,
        e.git.diff().unwrap().hunks[0]
    );
    assert_eq!(records(&e), before); // Reattachment is derived, not a rewrite of historical evidence.
    r.write("a", &first.replace("first change 界", "changed contract"));
    act(&mut e, "gitrefresh");
    assert!(
        before
            .iter()
            .all(|r| e.review_freshness(r) == Freshness::Stale)
    );
}

#[test]
fn renamed_deleted_empty_and_new_paths_remain_available_after_external_commits() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.write("gone", "remove\n");
    r.write("empty", "");
    r.commit();
    r.write("a", "reviewed\n");
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    act(&mut e, "review full");
    e.execute_command("note hunk").unwrap();
    e.handle_key(Key::Char('i'));
    e.paste("on moved content");
    e.handle_key(Key::Escape);
    act(&mut e, "note save");
    let note = records(&e)[0].clone();
    r.git(&["mv", "a", "renamed"]);
    fs::remove_file(r.path("gone")).unwrap();
    fs::remove_file(r.path("empty")).unwrap();
    r.write("new", "");
    r.commit();
    act(&mut e, "review since");
    let paths: Vec<_> = e.changed_files().into_iter().map(|f| f.path).collect();
    for name in ["renamed", "gone", "empty", "new"] {
        assert!(paths.contains(&r.path(name)), "missing {name}");
    }
    assert!(
        e.git
            .snapshot
            .as_ref()
            .unwrap()
            .review_renames
            .contains_key(Path::new("renamed"))
    );
    e.open_diff(r.path("gone"), e.git.comparison.clone());
    settle(&mut e);
    assert!(!e.git.diff().unwrap().source_available);
    assert!(e.jump_diff_source().is_err());
    e.open_diff(r.path("empty"), e.git.comparison.clone());
    settle(&mut e);
    let id = e.git.diff().unwrap().identity.as_ref().unwrap();
    assert!(id.old.present && !id.new.present);
    assert!(e.git.diff().unwrap().rows.is_empty());
    act(&mut e, "review full");
    act(&mut e, "note next");
    assert_eq!(e.git.diff_view.as_ref().unwrap().path, r.path("renamed"));
    assert_eq!(e.review_freshness(&note), Freshness::Fresh);
    assert!(note.note.unwrap().contains("moved"));
}

#[test]
fn checkpoint_cas_merges_annotations_but_rejects_competing_snapshots_and_corruption() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    r.write("a", "next\n");
    let mut e = r.editor();
    let (repo, first) = capture(&e, &r.path("a"));
    let (_, competing) = capture(&e, &r.path("a"));
    act(&mut e, "review file reviewed");
    let (saved, _) = review_store::save_snapshot(&repo, first).unwrap();
    assert_eq!(saved.records.len(), 1);
    let bytes = fs::read(review_store::store_path(&repo)).unwrap();
    assert!(
        review_store::save_snapshot(&repo, competing)
            .unwrap_err()
            .contains("conflict")
    );
    assert_eq!(fs::read(review_store::store_path(&repo)).unwrap(), bytes);
    let mut corrupt = saved.clone();
    corrupt.history.as_mut().unwrap().checkpoints[0].files[0]
        .text
        .push('!');
    fs::write(
        review_store::store_path(&repo),
        serde_json::to_vec(&corrupt).unwrap(),
    )
    .unwrap();
    assert!(
        review_store::load(&repo)
            .unwrap_err()
            .contains("Invalid review snapshot")
    );
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "next\n");
    fs::write(review_store::store_path(&repo), bytes).unwrap();
    let mut req = request(&e, &r.path("a"));
    req.review = Some(Arc::new(saved));
    let (_, next) = git::capture_review(&req, &Config::default()).unwrap();
    let mut invalid = next;
    invalid.files[0].path = "../escape".into();
    assert!(review_store::save_snapshot(&repo, invalid).is_err());
    assert_eq!(
        review_store::load(&repo).unwrap().history.unwrap().latest(),
        1
    );
}

#[test]
fn capture_limits_and_unsupported_files_fail_without_partial_history() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    let e = r.editor();
    fs::write(r.path("large"), vec![0; git::CONTENT_LIMIT + 1]).unwrap();
    assert!(
        git::capture_review(&request(&e, &r.path("a")), &Config::default())
            .err()
            .unwrap()
            .contains("2 MiB")
    );
    fs::remove_file(r.path("large")).unwrap();
    for n in 0..review_snapshot::MAX_FILES {
        r.write(&format!("file{n}"), "");
    }
    assert!(
        git::capture_review(&request(&e, &r.path("a")), &Config::default())
            .err()
            .unwrap()
            .contains("512")
    );
    assert!(!r.path(".git/editor-pro-review.json").exists());
    assert_eq!(text(&e), "base\n");
    assert_eq!(e.view, View::Editing);
}

#[test]
fn binary_and_non_utf8_snapshots_roundtrip_without_blocking_text_review() {
    let r = Repo::new();
    r.write("a", "base\r\n界👩‍💻\r\n");
    let logo = "src/main/resources/pdf/assets/bankID_logo.png";
    fs::create_dir_all(r.path(logo).parent().unwrap()).unwrap();
    let original = b"\x89PNG\r\n\x1a\n\0original";
    let revised = b"\x89PNG\r\n\x1a\n\0revision";
    fs::write(r.path(logo), original).unwrap();
    fs::write(r.path("encoded"), b"\xff\xfe\x80").unwrap();
    r.commit();
    r.write("a", "next\r\n界👩‍💻\r\n");
    fs::write(r.path(logo), revised).unwrap();
    fs::write(r.path("untracked"), b"new\0asset").unwrap();
    let mut e = r.editor();
    let index = fs::read(r.path(".git/index")).unwrap();
    let head = r.git(&["rev-parse", "HEAD"]);
    let status_args = ["--no-optional-locks", "status", "--porcelain=v2", "-z"];
    let status = r.git(&status_args);
    act(&mut e, "review snapshot");
    let repo = e.review.store.as_ref().unwrap().repository.clone();
    let history = review_store::load(&repo).unwrap().history.unwrap();
    history.validate().unwrap();
    for (files, name, expected) in [
        (&history.baseline.files, logo, original.as_slice()),
        (&history.checkpoints[0].files, logo, revised.as_slice()),
        (
            &history.baseline.files,
            "encoded",
            b"\xff\xfe\x80".as_slice(),
        ),
        (
            &history.checkpoints[0].files,
            "untracked",
            b"new\0asset".as_slice(),
        ),
    ] {
        let file = files.iter().find(|f| f.path == Path::new(name)).unwrap();
        assert_eq!(file.bytes(), expected);
        assert!(file.binary.is_some());
        assert!(file.text.is_empty());
    }
    let stored = fs::read(review_store::store_path(&repo)).unwrap();
    let encoded: serde_json::Value = serde_json::from_slice(&stored).unwrap();
    assert!(
        encoded["history"]["baseline"]["files"][0]
            .get("binary")
            .is_none()
    );
    act(&mut e, "review full");
    assert!(hunk_text(&e).contains("base\r"));
    assert!(hunk_text(&e).contains("next\r"));
    assert!(e.changed_files().iter().any(|f| f.path == r.path(logo)));
    e.open_diff(r.path(logo), Comparison::ReviewFull);
    settle(&mut e);
    assert!(
        e.git
            .snapshot
            .as_ref()
            .unwrap()
            .diff
            .as_ref()
            .unwrap_err()
            .contains("binary")
    );
    assert!(e.execute_command("review file reviewed").is_err());
    assert_eq!(text(&e), "next\r\n界👩‍💻\r\n");
    drop(e);

    let mut resumed = r.editor();
    act(&mut resumed, "review since");
    assert!(resumed.changed_files().is_empty());
    assert_eq!(
        resumed
            .review
            .store
            .as_ref()
            .unwrap()
            .history
            .as_ref()
            .unwrap(),
        &history
    );
    fs::write(r.path(logo), b"\x89PNG\r\n\x1a\n\0external").unwrap();
    act(&mut resumed, "gitrefresh");
    assert!(
        resumed
            .changed_files()
            .iter()
            .any(|f| f.path == r.path(logo))
    );
    assert!(resumed.git.diff().unwrap().rows.is_empty());
    act(&mut resumed, "review snapshot");
    let later = resumed
        .review
        .store
        .as_ref()
        .unwrap()
        .history
        .as_ref()
        .unwrap();
    assert_eq!(later.baseline, history.baseline);
    assert_eq!(later.checkpoints[0], history.checkpoints[0]);
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index);
    assert_eq!(r.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(r.git(&status_args), status);
    assert_eq!(fs::read(r.path("encoded")).unwrap(), b"\xff\xfe\x80");
}

#[test]
fn binary_changes_and_text_transitions_keep_presence_and_content_identity() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.write("to-binary", "text\n");
    for name in ["to-text", "deleted", "renamed"] {
        fs::write(r.path(name), format!("{name}\0asset")).unwrap();
    }
    r.commit();
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    fs::write(r.path("to-binary"), b"text\0").unwrap();
    r.write("to-text", "now text\n");
    fs::remove_file(r.path("deleted")).unwrap();
    fs::rename(r.path("renamed"), r.path("moved")).unwrap();
    fs::write(r.path("added"), b"\xffnew asset").unwrap();
    r.commit();
    for command in ["review full", "review since"] {
        act(&mut e, command);
        let snapshot = e.git.snapshot.as_ref().unwrap();
        for (path, status) in [
            ("to-binary", 'M'),
            ("to-text", 'M'),
            ("deleted", 'D'),
            ("moved", 'R'),
            ("added", 'A'),
        ] {
            assert!(
                snapshot
                    .review_changes
                    .iter()
                    .any(|entry| entry.path == Path::new(path) && entry.worktree == status),
                "{command}: {path}"
            );
        }
        assert_eq!(snapshot.review_changes.len(), 5);
        assert_eq!(
            snapshot.review_renames[Path::new("moved")],
            Path::new("renamed")
        );
        for path in ["to-binary", "to-text", "deleted", "moved", "added"] {
            e.open_diff(r.path(path), e.git.comparison.clone());
            settle(&mut e);
            assert!(
                e.git
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .diff
                    .as_ref()
                    .unwrap_err()
                    .contains("binary")
            );
            assert!(e.jump_diff_source().is_err());
        }
        e.open_diff(r.path("a"), e.git.comparison.clone());
        settle(&mut e);
    }
    act(&mut e, "review snapshot");
    act(&mut e, "review since");
    assert!(e.changed_files().is_empty());
    r.write("to-text", "later text\n");
    e.open_diff(r.path("to-text"), e.git.comparison.clone());
    settle(&mut e);
    assert!(hunk_text(&e).contains("now text"));
    assert!(hunk_text(&e).contains("later text"));
}

#[test]
fn binary_snapshot_hashes_encoding_and_byte_limits_are_validated() {
    let r = Repo::new();
    r.write("a", "base\n");
    fs::write(r.path("binary"), b"\0\xffpayload").unwrap();
    r.commit();
    let e = r.editor();
    let (repo, capture) = capture(&e, &r.path("a"));
    let (store, _) = review_store::save_snapshot(&repo, capture).unwrap();
    let before = fs::read(review_store::store_path(&repo)).unwrap();
    let history = store.history.as_ref().unwrap();
    let mut corrupt = history.clone();
    corrupt.checkpoints[0].files[1].binary.as_mut().unwrap()[0] = 1;
    assert!(
        corrupt
            .validate()
            .unwrap_err()
            .contains("Invalid review snapshot")
    );
    let mut corrupt = history.clone();
    corrupt.checkpoints[0].files[1].text = "conflicting text".into();
    assert!(corrupt.validate().is_err());
    for invalid in ["0", "gg", "é", &"00".repeat(git::CONTENT_LIMIT + 1)] {
        let mut value = serde_json::to_value(&store).unwrap();
        value["history"]["checkpoints"][0]["files"][1]["binary"] = invalid.into();
        assert!(serde_json::from_value::<editor_pro::review::Store>(value).is_err());
    }
    let mut oversized = history.checkpoints[0].files[1].clone();
    oversized.binary = Some(vec![0; git::CONTENT_LIMIT + 1]);
    oversized.sha256 = editor_pro::review::fingerprint(oversized.bytes());
    assert!(review_snapshot::validate_files(&[oversized]).is_err());
    assert_eq!(fs::read(review_store::store_path(&repo)).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn full_baselines_batch_blob_reads_within_the_git_deadline() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    r.write("a", "base\n");
    for n in 1..review_snapshot::MAX_FILES {
        r.write(
            &format!("file{n:03}\t\n界.txt"),
            &format!("content {n}\r\n"),
        );
    }
    r.commit();
    let e = r.editor();
    let wrapper = tempdir().unwrap();
    let script = wrapper.path().join("git-wrapper");
    let log = wrapper.path().join("calls.jsonl");
    fs::write(&script, format!(
        "#!/usr/bin/env python3\nimport json, os, sys, time\nwith open({}, 'a') as log:\n    log.write(json.dumps(sys.argv[1:]) + '\\n')\ntime.sleep(0.02)\nos.execvp('git', ['git', *sys.argv[1:]])\n",
        serde_json::to_string(&log).unwrap()
    )).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        executable: script,
        ..Default::default()
    };
    let index = fs::read(r.path(".git/index")).unwrap();
    for comparison in [
        Comparison::Head,
        Comparison::Index,
        Comparison::Revision("HEAD".into()),
    ] {
        fs::write(&log, "").unwrap();
        let mut req = request(&e, &r.path("a"));
        req.comparison = comparison;
        let (_, captured) = git::capture_review(&req, &config).unwrap();
        assert_eq!(captured.baseline.files.len(), review_snapshot::MAX_FILES);
        assert_eq!(captured.files.len(), review_snapshot::MAX_FILES);
        for (base, live) in captured.baseline.files.iter().zip(&captured.files) {
            assert_eq!(base.path, live.path);
            assert_eq!(base.bytes(), live.bytes());
        }
        let calls: Vec<Vec<String>> = fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(
            calls.len() <= 20,
            "Expected bounded Git process count, got {}",
            calls.len()
        );
        for option in ["--batch-check", "--batch"] {
            assert_eq!(
                calls
                    .iter()
                    .filter(|args| args.iter().any(|arg| arg == option))
                    .count(),
                2
            );
        }
    }
    assert_eq!(fs::read(r.path(".git/index")).unwrap(), index);
}

#[cfg(unix)]
#[test]
fn baseline_batch_limits_are_checked_before_reading_blob_contents() {
    use std::os::unix::fs::PermissionsExt;
    for (count, size, error) in [
        (1, git::CONTENT_LIMIT + 1, "2 MiB"),
        (4, git::CONTENT_LIMIT, "8 MiB"),
    ] {
        let r = Repo::new();
        r.write("a", "base\n");
        for n in 0..count {
            // Repeated object IDs still count once per path toward tree limits.
            fs::write(r.path(&format!("binary{n}")), vec![0; size]).unwrap();
        }
        r.commit();
        let e = r.editor();
        let wrapper = tempdir().unwrap();
        let script = wrapper.path().join("git-wrapper");
        let marker = wrapper.path().join("contents-requested");
        fs::write(&script, format!(
            "#!/usr/bin/env python3\nimport os, sys\nif '--batch' in sys.argv:\n    open({}, 'w').close()\nos.execvp('git', ['git', *sys.argv[1:]])\n",
            serde_json::to_string(&marker).unwrap()
        )).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let config = Config {
            executable: script,
            ..Default::default()
        };
        assert!(
            git::capture_review(&request(&e, &r.path("a")), &config)
                .err()
                .unwrap()
                .contains(error)
        );
        assert!(!marker.exists(), "Oversized content must not be requested");
        let repo = &e.review.store.as_ref().unwrap().repository;
        assert!(!review_store::store_path(repo).exists());
    }
}

#[cfg(unix)]
#[test]
fn failed_or_timed_out_batch_reads_preserve_the_store_and_allow_retry() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    r.write("a", "next\n");
    let mut e = r.editor();
    act(&mut e, "review file reviewed");
    let repo = e.review.store.as_ref().unwrap().repository.clone();
    let before = fs::read(review_store::store_path(&repo)).unwrap();
    let wrapper = tempdir().unwrap();
    let script = wrapper.path().join("git-wrapper");
    let marker = wrapper.path().join("pid");
    for (behavior, expected) in [
        ("missing", "missing Git blob"),
        ("truncated", "Incomplete"),
        ("extra", "Unexpected"),
        ("slow", "timed out"),
    ] {
        fs::write(&script, format!(
            "#!/usr/bin/env python3\nimport os, subprocess, sys, time\nif '--batch' not in sys.argv:\n    os.execvp('git', ['git', *sys.argv[1:]])\nbehavior = {behavior:?}\nif behavior == 'slow':\n    with open({}, 'w') as marker:\n        marker.write(str(os.getpid()))\n    time.sleep(60)\ndata = sys.stdin.buffer.read()\nif behavior == 'missing':\n    sys.stdout.buffer.write(data.splitlines()[0] + b' missing\\n')\nelif behavior == 'truncated':\n    sys.stdout.buffer.write(data.splitlines()[0] + b' blob 5\\nba')\nelse:\n    result = subprocess.run(['git', *sys.argv[1:]], input=data, capture_output=True, check=True)\n    sys.stdout.buffer.write(result.stdout + b'extra')\n",
            serde_json::to_string(&marker).unwrap()
        )).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let config = Config {
            executable: script.clone(),
            timeout: Duration::from_secs(2),
            ..Default::default()
        };
        let error = git::capture_review(&request(&e, &r.path("a")), &config)
            .err()
            .unwrap();
        assert!(error.contains(expected), "{behavior}: {error}");
        assert_eq!(fs::read(review_store::store_path(&repo)).unwrap(), before);
        if behavior == "slow" {
            let pid = fs::read_to_string(&marker).unwrap();
            assert!(
                !Command::new("/bin/kill")
                    .args(["-0", &pid])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
    }
    act(&mut e, "review snapshot");
    assert_eq!(records(&e).len(), 1);
    assert_eq!(
        e.review
            .store
            .as_ref()
            .unwrap()
            .history
            .as_ref()
            .unwrap()
            .latest(),
        1
    );
}

#[test]
fn old_and_new_line_notes_follow_unique_content_after_line_numbers_shift() {
    let r = Repo::new();
    let base = (0..60).map(|n| format!("row {n}\r\n")).collect::<String>();
    r.write("a", &base);
    r.commit();
    let first = base.replace("row 25\r\n", "changed 👩‍💻界\r\n");
    r.write("a", &first);
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    act(&mut e, "review full");
    for (kind, text) in [("old", "row 25\r"), ("new", "changed 👩‍💻界\r")] {
        let row = e
            .git
            .diff()
            .unwrap()
            .rows
            .iter()
            .position(|r| r.text == text)
            .unwrap();
        e.git.diff_view.as_mut().unwrap().selected = row;
        e.execute_command(&format!("note {kind}")).unwrap();
        e.handle_key(Key::Char('i'));
        e.paste(&format!("{kind} side note"));
        e.handle_key(Key::Escape);
        act(&mut e, "note save");
    }
    let notes = records(&e);
    r.write("a", &format!("inserted before\r\n{first}"));
    act(&mut e, "gitrefresh");
    for note in &notes {
        assert_eq!(e.review_freshness(note), Freshness::Fresh);
    }
    e.review.focused_record = Some(notes[0].id);
    act(&mut e, "note next");
    let row = &e.git.diff().unwrap().rows[e.git.diff_view.as_ref().unwrap().selected];
    assert_eq!(row.new, Some(26));
    assert_eq!(notes[1].context.new_line, Some(25));
    assert_eq!(row.anchor, 25); // Source retains the pre-refresh document.
    assert_eq!(text(&e), first);
    let theme = e.theme.palette();
    for (width, height) in [(1, 2), (80, 20), (130, 24)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| editor_pro::ui::render(f, &mut e, &theme))
            .unwrap();
        assert_eq!(text(&e), first);
    }
}

#[test]
fn duplicated_hunk_signatures_never_keep_fresh_progress_or_reattach_a_note() {
    let r = Repo::new();
    let block = "c0\nc1\nc2\nold\nc3\nc4\nc5\n";
    let middle = (0..12)
        .map(|n| format!("separator {n}\n"))
        .collect::<String>();
    let base = format!("{block}{middle}{block}");
    r.write("a", &base);
    r.commit();
    r.write("a", &base.replacen("old\n", "new\n", 1));
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    act(&mut e, "review full");
    act(&mut e, "review hunk reviewed");
    e.execute_command("note hunk").unwrap();
    e.handle_key(Key::Char('i'));
    e.paste("unique at creation");
    e.handle_key(Key::Escape);
    act(&mut e, "note save");
    let original = records(&e);
    r.write("a", &base.replace("old\n", "new\n"));
    act(&mut e, "gitrefresh");
    assert_eq!(e.git.diff().unwrap().hunks.len(), 2);
    assert!(e.git.diff().unwrap().locations.hunks.is_empty());
    for record in &original {
        assert_eq!(e.review_freshness(record), Freshness::Stale);
    }
    assert!(e.execute_command("review hunk reviewed").is_err());
    assert!(e.review_summary().contains("H0"));
}

#[test]
fn a_capture_keeps_request_time_unsaved_text_and_later_edits_invalidate_comparisons() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    let mut e = r.editor();
    e.handle_key(Key::Char('i'));
    e.paste("first ");
    e.handle_key(Key::Escape);
    let first = text(&e);
    e.execute_command("review snapshot").unwrap();
    assert!(e.execute_command("q").is_err());
    e.handle_key(Key::Char('i'));
    e.paste("second ");
    e.handle_key(Key::Escape);
    let second = text(&e);
    settle(&mut e);
    let checkpoint = &e
        .review
        .store
        .as_ref()
        .unwrap()
        .history
        .as_ref()
        .unwrap()
        .checkpoints[0];
    assert_eq!(checkpoint.files[0].text, first);
    act(&mut e, "review since");
    assert_eq!(
        e.git.diff().unwrap().identity.as_ref().unwrap().new.sha256,
        editor_pro::review::fingerprint(second.as_bytes())
    );
    e.document.buffer.insert("newer"); // Before tick sees the new generation.
    assert!(e.jump_diff_source().is_err());
    assert!(e.execute_command("review file reviewed").is_err());
    settle(&mut e);
    assert!(hunk_text(&e).contains("newer"));
    assert_eq!(fs::read_to_string(r.path("a")).unwrap(), "base\n");
}

#[test]
fn unborn_and_index_baselines_keep_presence_modes_and_original_bytes() {
    let r = Repo::new();
    r.write("a", "unborn\n");
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    act(&mut e, "review full");
    let identity = e.git.diff().unwrap().identity.as_ref().unwrap();
    assert!(!identity.old.present && identity.new.present);
    drop(e);
    let other = Repo::new();
    other.write("a", "base\n");
    other.commit();
    other.write("a", "index\n");
    other.git(&["add", "a"]);
    other.write("a", "working\n");
    let mut e = other.editor();
    act(&mut e, "gitbase index");
    act(&mut e, "review snapshot");
    other.git(&["add", "a"]);
    act(&mut e, "review full");
    assert!(hunk_text(&e).contains("index"));
    assert!(hunk_text(&e).contains("working"));
    assert_eq!(
        e.review
            .store
            .as_ref()
            .unwrap()
            .history
            .as_ref()
            .unwrap()
            .baseline
            .comparison,
        Comparison::Index
    );
}

#[cfg(unix)]
#[test]
fn unusual_paths_roundtrip_and_symlinks_are_refused_without_changing_prior_checkpoints() {
    use std::os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    };
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    // macOS filesystems require UTF-8; retain delimiter and Unicode coverage.
    let name = std::ffi::OsString::from_vec(if cfg!(target_os = "macos") {
        "raw界\n.txt".as_bytes().to_vec()
    } else {
        b"raw\xff\n.txt".to_vec()
    });
    let path = r.0.path().join(name);
    fs::write(&path, "界\r\n").unwrap();
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    let repo = e.review.store.as_ref().unwrap().repository.clone();
    let before = fs::read(review_store::store_path(&repo)).unwrap();
    assert!(
        review_store::load(&repo)
            .unwrap()
            .history
            .unwrap()
            .checkpoints[0]
            .files
            .iter()
            .any(|f| repo.root.join(&f.path) == path)
    );
    assert_eq!(
        fs::metadata(review_store::store_path(&repo))
            .unwrap()
            .permissions()
            .mode()
            & 0o077,
        0
    );
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("file"), "private").unwrap();
    symlink(outside.path(), r.path("linked")).unwrap();
    let mut req = request(&e, &r.path("a"));
    req.review_buffers.push((
        r.path("linked/file"),
        git::BufferVersion {
            id: 9,
            generation: 1,
            dirty: true,
            text: ropey::Rope::from_str("unsaved"),
        },
    ));
    assert!(
        git::capture_review(&req, &Config::default())
            .err()
            .unwrap()
            .contains("symlink")
    );
    assert_eq!(fs::read(review_store::store_path(&repo)).unwrap(), before);
    assert_eq!(
        fs::read_to_string(outside.path().join("file")).unwrap(),
        "private"
    );
}

#[test]
fn immutable_full_content_survives_pruning_the_original_git_commit() {
    let r = Repo::new();
    r.write("a", "original base\n");
    r.commit();
    let oid = String::from_utf8(r.git(&["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    r.write("a", "checkpoint\n");
    let mut e = r.editor();
    act(&mut e, "review snapshot");
    r.write("a", "later committed revision\n");
    r.commit();
    // The saved bytes, not object availability, are the baseline. Only remove a
    // loose object in this disposable repository, leaving current HEAD intact.
    fs::remove_file(r.path(&format!(".git/objects/{}/{}", &oid[..2], &oid[2..]))).unwrap();
    act(&mut e, "review full");
    assert!(hunk_text(&e).contains("original base"));
    assert!(hunk_text(&e).contains("later committed revision"));
    assert_eq!(text(&e), "checkpoint\n");
}

#[test]
fn checkpoint_count_and_union_path_limits_refuse_without_replacing_history() {
    let r = Repo::new();
    r.write("a", "base\n");
    r.commit();
    let e = r.editor();
    let (repo, initial) = capture(&e, &r.path("a"));
    let (mut store, _) = review_store::save_snapshot(&repo, initial).unwrap();
    for _ in 1..review_snapshot::MAX_CHECKPOINTS {
        let history = store.history.as_ref().unwrap();
        let capture = review_snapshot::Capture {
            baseline: history.baseline.clone(),
            files: history.checkpoints[0].files.clone(),
            expected_latest: history.latest(),
        };
        store = review_store::save_snapshot(&repo, capture).unwrap().0;
    }
    let before = fs::read(review_store::store_path(&repo)).unwrap();
    let history = store.history.as_ref().unwrap();
    let extra = review_snapshot::Capture {
        baseline: history.baseline.clone(),
        files: history.checkpoints[0].files.clone(),
        expected_latest: history.latest(),
    };
    assert!(
        review_store::save_snapshot(&repo, extra)
            .unwrap_err()
            .contains("limit")
    );
    assert_eq!(fs::read(review_store::store_path(&repo)).unwrap(), before);
    let mut union = history.clone();
    union.checkpoints.truncate(1);
    let template = union.checkpoints[0].files[0].clone();
    union.checkpoints[0].files = (0..review_snapshot::MAX_FILES)
        .map(|n| {
            let mut file = template.clone();
            file.path = format!("new{n}").into();
            file
        })
        .collect();
    assert!(union.validate().unwrap_err().contains("512 retained paths"));
}
