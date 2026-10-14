//! Reproduce: cargo run --locked --release --example review_performance.
//! Local temporary fixtures, sampled latency; no hard responsiveness guarantee.
use editor_pro::{
    editor::Editor,
    file::Document,
    git::{self, Comparison, Config, Request},
    input::Key,
    review::{ContentId, Record, State, Store},
    review_store,
    theme::Theme,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};
fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn settle(e: &mut Editor) -> Duration {
    let start = Instant::now();
    loop {
        e.tick();
        if e.git_current() && !e.review.busy && e.review.store.is_some() {
            assert!(e.review.error.is_none(), "{}", e.review.status);
            return start.elapsed();
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{} {}",
            e.git.summary(),
            e.review.status
        );
        thread::sleep(Duration::from_millis(1));
    }
}
fn io_done(e: &mut Editor) -> Duration {
    let start = Instant::now();
    while e.review.busy {
        e.tick();
        assert!(start.elapsed() < Duration::from_secs(30));
        thread::sleep(Duration::from_millis(1));
    }
    assert!(e.review.error.is_none(), "{}", e.review.status);
    start.elapsed()
}
fn p(samples: &mut [Duration], percentile: usize) -> f64 {
    samples.sort_unstable();
    samples[(samples.len() - 1) * percentile / 100].as_secs_f64() * 1000.0
}
fn main() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "Review benchmark"]);
    git(&root, &["config", "user.email", "review@example.invalid"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    let rows: Vec<_> = (0..1000)
        .map(|n| format!("fn row_{n}() {{ let text = \"界👩‍💻\"; }}\n"))
        .collect();
    for n in 0..24 {
        fs::write(root.join(format!("file{n:02}.rs")), rows.concat()).unwrap();
    }
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "-m", "fixture"]);
    let mut rows = rows;
    rows[10] = "fn changed_first() {}\n".into();
    rows[900] = "fn changed_second() {}\n".into();
    let text = rows.concat();
    for n in 0..24 {
        fs::write(root.join(format!("file{n:02}.rs")), &text).unwrap();
    }
    let index = fs::read(root.join(".git/index")).unwrap();
    let mut store = None;
    for n in 0..24 {
        let d = git::inspect(
            &Request {
                path: root.join(format!("file{n:02}.rs")),
                buffer: None,
                comparison: Comparison::Head,
                inspect: true,
                review: None,
                review_buffers: vec![],
                review_generation: 0,
            },
            &Config::default(),
        )
        .unwrap()
        .unwrap()
        .diff
        .unwrap();
        let identity = d.identity.as_ref().unwrap();
        let s = store.get_or_insert_with(|| Store::empty(identity.repository.clone()));
        for k in 0..12 {
            let (attachment, context) = d
                .attachment(
                    if k % 2 == 0 { 0 } else { d.hunks[1] },
                    if k == 0 || k > 1 { "file" } else { "hunk" },
                )
                .unwrap();
            s.records.push(Record {
                id: s.next_id,
                revision: 1,
                identity: identity.clone(),
                attachment,
                context,
                state: if k == 0 {
                    Some(State::Reviewed)
                } else if k == 1 {
                    Some(State::Attention)
                } else {
                    None
                },
                note: if k > 1 {
                    Some("Check Unicode 界, boundary conditions and error recovery.\n".repeat(12))
                } else {
                    None
                },
                unsaved: false,
            });
            s.next_id += 1;
        }
    }
    let store = store.unwrap();
    store.validate(&store.repository).unwrap();
    let path = review_store::store_path(&store.repository);
    fs::write(&path, serde_json::to_vec_pretty(&store).unwrap()).unwrap(); // Fixture setup, excluded.
    let mut e = Editor::new(Document::open(&root.join("file00.rs")).unwrap());
    let initial = settle(&mut e);
    assert!(e.review_summary().contains("F24"), "{}", e.review_summary());
    e.execute_command("diff").unwrap();
    settle(&mut e);
    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    let theme = Theme::night();
    e.execute_command("note file").unwrap();
    e.handle_key(Key::Char('i'));
    let mut typing = Vec::new();
    for _ in 0..150 {
        let start = Instant::now();
        e.handle_key(Key::Char('界'));
        e.tick();
        terminal.draw(|f| ui::render(f, &mut e, &theme)).unwrap();
        typing.push(start.elapsed());
    }
    e.handle_key(Key::Escape);
    let save_start = Instant::now();
    e.execute_command("note save").unwrap();
    io_done(&mut e);
    let save = save_start.elapsed();
    let refresh_start = Instant::now();
    e.tick();
    let mut navigation = Vec::new();
    for _ in 0..150 {
        let start = Instant::now();
        e.handle_key(Key::Char('j'));
        e.tick();
        terminal.draw(|f| ui::render(f, &mut e, &theme)).unwrap();
        navigation.push(start.elapsed());
        thread::sleep(Duration::from_millis(1));
    }
    settle(&mut e);
    let refresh = refresh_start.elapsed();
    let export_start = Instant::now();
    e.export_review(dir.path().join("feedback.md").to_str().unwrap())
        .unwrap();
    io_done(&mut e);
    let export = export_start.elapsed();
    assert!(
        fs::read_to_string(dir.path().join("feedback.md"))
            .unwrap()
            .contains("24 fresh reviewed file marks")
    );
    let bytes = vec![b'x'; git::CONTENT_LIMIT];
    let mut hashing = Vec::new();
    for _ in 0..40 {
        let start = Instant::now();
        std::hint::black_box(ContentId::new(&bytes, true));
        std::hint::black_box(ContentId::new(&bytes, true));
        hashing.push(start.elapsed());
    }
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    for n in 0..24 {
        assert_eq!(
            fs::read_to_string(root.join(format!("file{n:02}.rs"))).unwrap(),
            text
        );
    }
    println!(
        "Review fixture: 24 tracked/changed files, 1,001 lines and {} bytes each; 288 initial records (24 file marks, 24 attention hunks, 240 multiline notes); store {} bytes after one added note; 120x32 TestBackend",
        text.len(),
        fs::metadata(&path).unwrap().len()
    );
    println!(
        "initial load+validation {:.2} ms; note serialization+atomic persistence {:.2} ms; refresh with 24 annotated paths {:.2} ms; Markdown export {:.2} ms",
        initial.as_secs_f64() * 1000.0,
        save.as_secs_f64() * 1000.0,
        refresh.as_secs_f64() * 1000.0,
        export.as_secs_f64() * 1000.0
    );
    println!(
        "150 key+tick+draw samples: note typing p95/max {:.2}/{:.2} ms; navigation during refresh {:.2}/{:.2} ms",
        p(&mut typing, 95),
        p(&mut typing, 100),
        p(&mut navigation, 95),
        p(&mut navigation, 100)
    );
    println!(
        "SHA-256 both 2 MiB sides, 40 samples p95/max {:.2}/{:.2} ms",
        p(&mut hashing, 95),
        p(&mut hashing, 100)
    );
    println!(
        "Load/refresh include debounce and 1 ms polling. Refresh also spans the navigation sampling loop. Excludes fixture setup, terminal transport, synchronous disk checks and remote filesystems. No hard guarantee; unavailable paths never count as fresh."
    );
}
