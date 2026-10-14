//! Reproducible smoke measurements, not a microbenchmark or a timing gate.
//! Run with: cargo run --locked --release --example performance
use editor_pro::{
    buffer::Buffer, editor::Editor, file::Document, picker::FilePicker, syntax::Syntax,
    theme::Theme, ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
fn percentile(samples: &mut [Duration], percent: usize) -> f64 {
    samples.sort_unstable();
    samples[(samples.len() - 1) * percent / 100].as_secs_f64() * 1000.0
}
fn main() {
    for (path, source) in [
        (
            "sample.rs",
            "fn example() {\n    let café = \"界👩‍💻\";\n}\n".repeat(1000),
        ),
        (
            "sample.java",
            "class Example { String name = \"界\"; }\n".repeat(1000),
        ),
        (
            "sample.kt",
            "fun example() { println(\"界\") }\n".repeat(1000),
        ),
        (
            "sample.cs",
            "class Example { string name = \"界\"; }\n".repeat(1000),
        ),
        (
            "sample.json",
            format!(
                "[\n{}{{\"value\":42}}\n]\n",
                "{\"value\":\"界\",\"count\":42},\n".repeat(1000)
            ),
        ),
        ("sample.yaml", "name: \"界\"\ncount: 42\n".repeat(1000)),
        ("sample.toml", "name = \"界\"\ncount = 42\n".repeat(1000)),
        (
            "sample.md",
            "# Heading 界\n\nSome **strong** and `code`.\n\n```\n**literal**\n```\n\n".repeat(1000),
        ),
    ] {
        let mut document = Document::default();
        document.buffer = Buffer::new(&source);
        document.syntax = Syntax::for_path(Some(Path::new(path)));
        let mut editor = Editor::new(document);
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        let theme = Theme::night();
        let start = Instant::now();
        terminal
            .draw(|frame| ui::render(frame, &mut editor, &theme))
            .unwrap();
        let initial = start.elapsed();
        assert!(
            editor.document.syntax.fallback.is_none(),
            "{path}: {:?}",
            editor.document.syntax.fallback
        );
        let mut edit_samples = Vec::new();
        let mut scroll_samples = Vec::new();
        for step in 0..50 {
            let row = step * 37 % editor.document.buffer.line_count();
            editor.document.buffer.goto_line(row, true);
            editor.document.buffer.line_end(true);
            let start = Instant::now();
            editor.document.buffer.insert("x");
            editor.document.buffer.commit();
            terminal
                .draw(|frame| ui::render(frame, &mut editor, &theme))
                .unwrap();
            edit_samples.push(start.elapsed());
            assert!(
                editor.document.syntax.fallback.is_none(),
                "{path}, step {step}: {:?}",
                editor.document.syntax.fallback
            );
            editor.document.buffer.undo();
            terminal
                .draw(|frame| ui::render(frame, &mut editor, &theme))
                .unwrap();
            assert!(
                editor.document.syntax.fallback.is_none(),
                "{path}, undo step {step}: {:?}",
                editor.document.syntax.fallback
            );
            editor
                .document
                .buffer
                .goto_line((row + 100) % editor.document.buffer.line_count(), false);
            let start = Instant::now();
            terminal
                .draw(|frame| ui::render(frame, &mut editor, &theme))
                .unwrap();
            scroll_samples.push(start.elapsed());
        }
        println!(
            "{path}: {} bytes / {} lines; initial {:.2} ms; edit+draw p50 {:.2}, p95 {:.2}, max {:.2} ms; scroll+draw p95 {:.2} ms",
            source.len(),
            editor.document.buffer.line_count(),
            initial.as_secs_f64() * 1000.0,
            percentile(&mut edit_samples, 50),
            percentile(&mut edit_samples, 95),
            percentile(&mut edit_samples, 100),
            percentile(&mut scroll_samples, 95)
        );
    }
    let dir = tempfile::tempdir().unwrap();
    for index in 0..10_000 {
        fs::write(dir.path().join(format!("file_{index:05}.rs")), "").unwrap();
    }
    let start = Instant::now();
    let mut picker = FilePicker::new(dir.path()).unwrap();
    settle(&mut picker);
    let scan = start.elapsed();
    let start = Instant::now();
    picker.set_query("file04200".into());
    settle(&mut picker);
    let query = start.elapsed();
    assert_eq!(picker.candidate(0).unwrap().display, "file_04200.rs");
    println!(
        "picker: {} files; scan+rank {:.2} ms; query {:.2} ms",
        picker.item_count(),
        scan.as_secs_f64() * 1000.0,
        query.as_secs_f64() * 1000.0
    );
}
fn settle(picker: &mut FilePicker) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        picker.tick();
        if !picker.working() {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
}
