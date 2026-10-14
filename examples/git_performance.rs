//! Reproduce with cargo run --locked --release --example git_performance.
//! Includes debounce and polling; excludes repository creation and terminal I/O.
use editor_pro::{editor::Editor, file::Document, input::Key, theme::Theme, ui};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};
fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn settle(e: &mut Editor) -> Duration {
    let start = Instant::now();
    loop {
        e.tick();
        if !e.git.loading {
            assert!(e.git.ready(), "{}", e.git.summary());
            return start.elapsed();
        }
        assert!(start.elapsed() < Duration::from_secs(30));
        thread::sleep(Duration::from_millis(1));
    }
}
fn percentile(samples: &mut [Duration], percent: usize) -> f64 {
    samples.sort_unstable();
    samples[(samples.len() - 1) * percent / 100].as_secs_f64() * 1000.0
}
fn main() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Measurement"]);
    git(root, &["config", "user.email", "measure@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "core.autocrlf", "false"]);
    for n in 0..2500 {
        fs::write(
            root.join(format!("file{n:04}.rs")),
            "fn sample() { let text = \"界\"; }\n".repeat(40),
        )
        .unwrap();
    }
    let source = root.join("source.rs");
    fs::write(&source, "fn sample() { let text = \"界\"; }\n".repeat(3000)).unwrap();
    git(root, &["add", "--all"]);
    git(root, &["commit", "-m", "fixture"]);
    for n in 0..100 {
        fs::write(root.join(format!("file{n:04}.rs")), "staged\n").unwrap();
    }
    git(root, &["add", "--all"]);
    for n in 100..200 {
        fs::write(root.join(format!("file{n:04}.rs")), "unstaged\n").unwrap();
    }
    for n in 0..100 {
        fs::write(root.join(format!("untracked{n:04}.rs")), "new\n").unwrap();
    }
    let index = fs::read(root.join(".git/index")).unwrap();
    let disk = fs::read(&source).unwrap();
    let mut editor = Editor::new(Document::open(&source).unwrap());
    let initial = settle(&mut editor);
    assert_eq!(editor.git.snapshot.as_ref().unwrap().entries.len(), 300);
    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    let theme = Theme::night();
    terminal
        .draw(|f| ui::render(f, &mut editor, &theme))
        .unwrap();
    editor.refresh_git();
    let mut navigation = Vec::new();
    for _ in 0..150 {
        let start = Instant::now();
        editor.handle_key(Key::Char('j'));
        editor.tick();
        terminal
            .draw(|f| ui::render(f, &mut editor, &theme))
            .unwrap();
        navigation.push(start.elapsed());
        thread::sleep(Duration::from_millis(2));
    }
    editor.handle_key(Key::Char('i'));
    let mut typing = Vec::new();
    for _ in 0..150 {
        let start = Instant::now();
        editor.handle_key(Key::Char('x'));
        editor.tick();
        terminal
            .draw(|f| ui::render(f, &mut editor, &theme))
            .unwrap();
        typing.push(start.elapsed());
        thread::sleep(Duration::from_millis(1));
    }
    editor.handle_key(Key::Escape);
    let final_refresh = settle(&mut editor);
    assert!(editor.git.diff().unwrap().right_label.contains("UNSAVED"));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(&source).unwrap(), disk);
    println!(
        "Git workload: 2,501 tracked files, 100 staged + 100 unstaged + 100 untracked; active 3,001-line UTF-8 Rust buffer, 120x32 TestBackend"
    );
    println!(
        "initial status+comparison {:.2} ms; final refresh after 150 coalesced edits {:.2} ms (75 ms debounce included)",
        initial.as_secs_f64() * 1000.0,
        final_refresh.as_secs_f64() * 1000.0
    );
    println!(
        "key+tick+draw while refresh pending: navigation p95 {:.2}, max {:.2} ms; typing p95 {:.2}, max {:.2} ms",
        percentile(&mut navigation, 95),
        percentile(&mut navigation, 100),
        percentile(&mut typing, 95),
        percentile(&mut typing, 100)
    );
    println!(
        "Local temporary repository; one run; excludes setup, terminal transport and existing synchronous disk checks. No hard latency guarantee."
    );
}
