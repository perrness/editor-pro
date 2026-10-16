//! Opt-in acceptance tests using the actual editor and installed language servers.
//! No protocol doubles and no downloads in the ordinary Cargo test suite.
use editor_pro::{
    editor::{Editor, View},
    file::Document,
    input::Key,
    lsp::{self, Language},
};
use lsp_types::DiagnosticSeverity;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

struct Project {
    // Drop the editor (and its servers) before deleting the project.
    editor: Editor,
    directory: tempfile::TempDir,
    name: &'static str,
    main: PathBuf,
    other: PathBuf,
    originals: Vec<(PathBuf, Vec<u8>)>,
    report: Value,
}

fn copy_fixture(source: &Path, target: &Path, originals: &mut Vec<(PathBuf, Vec<u8>)>) {
    fs::create_dir_all(target).unwrap();
    let mut entries: Vec<_> = fs::read_dir(source).unwrap().map(Result::unwrap).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_fixture(&entry.path(), &destination, originals);
        } else {
            let bytes = fs::read(entry.path()).unwrap();
            fs::write(&destination, &bytes).unwrap();
            originals.push((destination, bytes));
        }
    }
}

impl Project {
    fn new(name: &'static str, main: &str, other: &str) -> Self {
        let language = Language::for_path(Path::new(main)).unwrap();
        let install_dir = std::env::var_os("EDITOR_PRO_LSP_INSTALL_DIR").map(PathBuf::from);
        let argv = std::env::var(language.environment()).unwrap_or_else(|_| {
            assert!(install_dir.is_some(), "Set {} to a pinned real server argv or set EDITOR_PRO_LSP_INSTALL_DIR for managed installation", language.environment());
            "Managed installation".into()
        });
        let directory = tempfile::Builder::new()
            .prefix("editor-pro-lsp-real-")
            .tempdir()
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tools/lsp/fixtures")
            .join(name);
        let mut originals = Vec::new();
        copy_fixture(&source, &root, &mut originals);
        let mut digest = Sha256::new();
        for (path, bytes) in &originals {
            digest.update(
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes(),
            );
            digest.update([0]);
            digest.update(bytes);
            digest.update([0]);
        }
        let main = root.join(main);
        let other = root.join(other);
        let mut editor = Editor::new(Document::open(&main).unwrap());
        editor.lsp.config.installation.directory = install_dir;
        editor.lsp.config.installation.auto_install =
            editor.lsp.config.installation.directory.is_some();
        editor.lsp.config.restart_limit = 0;
        let report = json!({
            "fixture":name, "fixture_sha256":format!("{:x}", digest.finalize()),
            "os":std::env::consts::OS, "arch":std::env::consts::ARCH,
            "server_argv":argv, "checks":{}, "status":"running"
        });
        Self {
            editor,
            directory,
            name,
            main,
            other,
            originals,
            report,
        }
    }

    fn wait(&mut self, label: &str, condition: impl Fn(&Editor) -> bool) {
        let start = Instant::now();
        eprintln!("{}: waiting for {label}", self.name);
        self.report["phase"] = json!(label);
        loop {
            self.editor.tick();
            assert!(
                self.editor
                    .lsp
                    .installations
                    .values()
                    .all(|state| !state.failed),
                "{}: installation failed: {}",
                self.name,
                self.state()
            );
            if condition(&self.editor) {
                self.report["checks"][label] =
                    json!({"status":"passed", "seconds":start.elapsed().as_secs_f64()});
                return;
            }
            assert!(
                start.elapsed()
                    < Duration::from_secs(if self.editor.lsp.config.installation.auto_install {
                        900
                    } else {
                        180
                    }),
                "{}: timeout waiting for {label}; {}",
                self.name,
                self.state()
            );
            assert!(
                !self
                    .editor
                    .lsp
                    .servers
                    .values()
                    .any(|s| s.status.contains(":LspRestart retries")),
                "{}: server stopped during {label}; {}",
                self.name,
                self.state()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn state(&self) -> Value {
        json!({"message":self.editor.message, "buffer_text":self.editor.document.buffer.text().to_string(),
            "installation": self.editor.lsp.installations.iter().map(|(language, state)| json!({"language":language.id(),"status":state.message,"busy":state.busy,"failed":state.failed})).collect::<Vec<_>>(),
            "servers":self.editor.lsp.servers.values().map(|s|
                json!({"status":s.status,"capabilities":s.capabilities,"logs":s.logs})).collect::<Vec<_>>(),
            "diagnostics":self.editor.lsp.diagnostics.values().map(|d| &d.items).collect::<Vec<_>>()})
    }

    fn cursor(&mut self, marker: &str, token: &str, offset: usize) {
        self.editor.handle_key(Key::Escape);
        let text = self.editor.document.buffer.text().to_string();
        let mut scalar = 0;
        for line in text.split_inclusive('\n') {
            if line.contains(marker) {
                let byte = line.find(token).unwrap();
                self.editor
                    .document
                    .buffer
                    .set_cursor(scalar + line[..byte].chars().count() + offset, true);
                return;
            }
            scalar += line.chars().count();
        }
        panic!("Missing fixture marker {marker}");
    }

    fn command(&mut self, command: &str) {
        self.editor.execute_command(command).unwrap();
        self.wait(command, |e| !e.lsp.pending);
        if self.editor.message_is_error {
            self.report["checks"][command]["status"] = json!("failed");
        }
        assert!(!self.editor.message_is_error, "{command}: {}", self.state());
        if matches!(command, "CodeAction" | "Complete") {
            self.report["checks"][command]["items"] = json!(
                self.editor.lsp.panel.as_ref().map(|p| p
                    .rows
                    .iter()
                    .take(32)
                    .map(|r| &r.label)
                    .collect::<Vec<_>>())
            );
        }
    }

    fn panel_contains(&self, text: &str) -> bool {
        self.editor.lsp.panel.as_ref().is_some_and(|p| {
            p.rows
                .iter()
                .any(|r| r.label.contains(text) || r.detail.contains(text))
        })
    }

    fn comprehension(&mut self) {
        self.cursor("DEFINITION", "greet", 1);
        let cursor = self.editor.document.buffer.cursor();
        let viewport = self.editor.viewport.clone();
        let capabilities = self.editor.lsp.servers[&self.editor.lsp.routes[&self.main]]
            .capabilities
            .clone();
        if lsp::supports(&capabilities, "workspace/symbol") {
            self.command("WorkspaceSymbols Greeter");
            let index = self
                .editor
                .lsp
                .panel
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .position(|r| {
                    r.label.contains("Greeter")
                        && r.detail
                            .contains(self.other.file_name().unwrap().to_str().unwrap())
                })
                .expect("Workspace symbol must identify the fixture Greeter");
            self.editor.lsp.panel.as_mut().unwrap().selected = index;
            self.editor.handle_key(Key::Enter);
            self.wait("workspace_symbol_source", |e| !e.lsp.pending);
            assert!(!self.editor.message_is_error, "{}", self.state());
            assert_eq!(self.editor.document.path(), Some(self.other.as_path()));
            self.editor.execute_command("back").unwrap();
            assert_eq!(self.editor.document.path(), Some(self.main.as_path()));
            assert_eq!(self.editor.document.buffer.cursor(), cursor);
            assert_eq!(self.editor.viewport, viewport);
        } else {
            assert!(
                self.editor
                    .execute_command("WorkspaceSymbols Greeter")
                    .unwrap_err()
                    .to_string()
                    .contains("does not support")
            );
            self.report["checks"]["workspace_symbols"] =
                json!({"status":"unsupported; explicit response verified"});
        }
        if lsp::supports(&capabilities, "textDocument/prepareCallHierarchy") {
            self.command("Calls");
            assert!(self.panel_contains("greet"), "{}", self.state());
            self.editor.handle_key(Key::Char('i'));
            self.wait("incoming_calls", |e| !e.lsp.pending);
            assert!(!self.editor.message_is_error, "{}", self.state());
            assert!(
                self.panel_contains(self.main.file_name().unwrap().to_str().unwrap()),
                "{}",
                self.state()
            );
            self.editor.handle_key(Key::Char('o'));
            self.wait("outgoing_calls", |e| !e.lsp.pending);
            assert!(!self.editor.message_is_error, "{}", self.state());
            assert!(self.panel_contains("greet"), "{}", self.state());
            self.editor.handle_key(Key::Backspace);
            self.editor.handle_key(Key::Enter);
            assert_eq!(self.editor.view, View::Editing);
            self.editor.execute_command("back").unwrap();
            assert_eq!(self.editor.document.path(), Some(self.main.as_path()));
            assert_eq!(self.editor.document.buffer.cursor(), cursor);
            assert_eq!(self.editor.viewport, viewport);
        } else {
            assert!(
                self.editor
                    .execute_command("Calls")
                    .unwrap_err()
                    .to_string()
                    .contains("does not support")
            );
            self.report["checks"]["call_hierarchy"] =
                json!({"status":"unsupported; explicit response verified"});
        }
    }

    fn select_preview(&mut self, label: &str) -> bool {
        self.editor.handle_key(Key::Enter);
        self.wait(label, |e| !e.lsp.pending);
        let preview = !self.editor.message_is_error
            && self
                .editor
                .lsp
                .panel
                .as_ref()
                .is_some_and(|p| p.title.starts_with("Apply"));
        if !preview {
            self.report["checks"][label] =
                json!({"status":"failed", "message":self.editor.message});
        }
        preview
    }

    fn replace(&mut self, old: &str, new: &str) {
        self.editor.handle_key(Key::Escape);
        let text = self.editor.document.buffer.text().to_string();
        let start = text[..text.find(old).unwrap()].chars().count();
        self.editor
            .document
            .buffer
            .replace_range(start..start + old.chars().count(), new);
        self.editor.document.buffer.commit();
    }

    fn error_on_marker(editor: &Editor) -> Option<bool> {
        let line = editor
            .document
            .buffer
            .text()
            .lines()
            .position(|l| l.to_string().contains("DIAGNOSTIC"))? as u32;
        editor.current_diagnostics().map(|d| {
            d.items.iter().any(|d| {
                d.severity == Some(DiagnosticSeverity::ERROR)
                    && d.range.start.line <= line
                    && d.range.end.line >= line
            })
        })
    }

    fn run(&mut self) {
        self.wait("initial_type_diagnostic", |e| {
            Self::error_on_marker(e) == Some(true)
        });
        assert_eq!(
            self.editor.lsp.routes[&self.main].root,
            self.directory.path().canonicalize().unwrap()
        );
        self.report["initialized"] = self.state();

        self.cursor("DEFINITION", "greet", 1);
        self.command("Hover");
        assert!(self.panel_contains("greet"), "{}", self.state());
        self.cursor("DEFINITION", "greet", 1);
        self.command("Definition");
        assert!(
            self.panel_contains(self.other.file_name().unwrap().to_str().unwrap()),
            "{}",
            self.state()
        );
        self.editor.handle_key(Key::Enter);
        assert_eq!(self.editor.document.path(), Some(self.other.as_path()));
        self.editor.open_buffer(&self.main).unwrap();

        self.cursor("DEFINITION", "greet", 1);
        self.command("References");
        assert!(self.panel_contains(self.main.file_name().unwrap().to_str().unwrap()));
        self.editor.handle_key(Key::Escape);
        self.command("Symbols");
        assert!(
            self.editor
                .lsp
                .panel
                .as_ref()
                .is_some_and(|p| !p.rows.is_empty())
        );

        self.comprehension();

        self.cursor("DEFINITION", "greet(", "greet(".len());
        self.command("Signature");
        assert!(self.panel_contains("greet"), "{}", self.state());

        self.editor.handle_key(Key::Escape);
        self.wait("type_diagnostic_for_quick_fix", |e| {
            Self::error_on_marker(e) == Some(true)
        });
        let position = self
            .editor
            .current_diagnostics()
            .unwrap()
            .items
            .iter()
            .find(|d| d.severity == Some(DiagnosticSeverity::ERROR))
            .unwrap()
            .range
            .start;
        let scalar = lsp::scalar(self.editor.document.buffer.text(), position).unwrap();
        self.editor.document.buffer.set_cursor(scalar, false);
        self.command("CodeAction");
        let fix = self.editor.lsp.panel.as_ref().and_then(|panel| {
            // A source action such as Organize Imports may legitimately do
            // nothing here. Select the type-error fix the fixture asks for.
            panel.rows.iter().position(|r| {
                r.label.to_lowercase().contains("change type")
                    || r.label.to_lowercase().contains("change the type")
                    || r.label.to_lowercase().contains("explicit cast")
            })
        });
        if let Some(fix) = fix {
            self.editor.lsp.panel.as_mut().unwrap().selected = fix;
            self.select_preview("quick_fix_preview");
        } else {
            self.report["checks"]["quick_fix_preview"] =
                json!({"status":"failed", "message":"No quick fix for the fixture type error"});
        }
        self.editor.handle_key(Key::Escape);

        let before_completion = self.editor.document.buffer.text().to_string();
        let (member, prefix) = match Language::for_path(&self.main).unwrap() {
            Language::Kotlin => ("message.length", "message.len"),
            Language::Java => ("message.length()", "message.len"),
            Language::CSharp => ("message.Length", "message.Len"),
        };
        self.replace(member, prefix);
        let incomplete = self.editor.document.buffer.text().to_string();
        self.cursor("COMPLETION", prefix, prefix.len());
        self.command("Complete");
        assert!(
            self.panel_contains("length") || self.panel_contains("Length"),
            "{}",
            self.state()
        );
        let panel = self.editor.lsp.panel.as_mut().unwrap();
        panel.selected = panel
            .rows
            .iter()
            .position(|r| {
                matches!(
                    r.label.split(['(', ':', ' ']).next(),
                    Some("length" | "Length")
                )
            })
            .unwrap();
        self.editor.handle_key(Key::Char('d'));
        self.wait("completion_documentation", |e| !e.lsp.pending);
        assert!(!self.editor.message_is_error, "{}", self.state());
        self.editor.handle_key(Key::Enter);
        self.wait("completion_accept", |e| !e.lsp.pending);
        if Language::for_path(&self.main) == Some(Language::CSharp) {
            assert_eq!(
                self.editor.view,
                View::Editing,
                "Plain completion must accept directly"
            );
        }
        let mut previews = 0;
        for step in 0..2 {
            if !self
                .editor
                .lsp
                .panel
                .as_ref()
                .is_some_and(|p| p.title.starts_with("Apply"))
            {
                break;
            }
            if step == 0 {
                assert_eq!(self.editor.document.buffer.text().to_string(), incomplete);
            }
            previews += 1;
            self.editor.handle_key(Key::Enter);
            self.wait(&format!("completion_preview_{step}"), |e| !e.lsp.pending);
        }
        self.report["checks"]["completion_accept"]["previews"] = json!(previews);
        assert!(!self.editor.message_is_error, "{}", self.state());
        assert_eq!(self.editor.view, View::Editing);
        assert!(
            self.editor
                .document
                .buffer
                .text()
                .to_string()
                .contains(member.trim_end_matches("()")),
            "{}",
            self.state()
        );
        assert!(self.editor.document.buffer.undo());
        assert_eq!(self.editor.document.buffer.text().to_string(), incomplete);
        self.editor.handle_key(Key::Escape);
        assert!(self.editor.document.buffer.undo());
        assert_eq!(
            self.editor.document.buffer.text().to_string(),
            before_completion
        );

        let partial = &prefix[..prefix.len() - 1];
        self.replace(member, partial);
        self.cursor("COMPLETION", partial, partial.len());
        self.editor.handle_key(Key::Char('i'));
        self.editor.handle_key(Key::Char('n'));
        self.wait("automatic_completion_popup", |e| {
            e.lsp.completion.is_some() || e.message_is_error
        });
        assert!(!self.editor.message_is_error, "{}", self.state());
        assert_eq!(self.editor.view, View::Editing);
        assert_eq!(self.editor.mode, editor_pro::editor::Mode::Insert);
        let menu = self.editor.lsp.completion.as_mut().unwrap();
        menu.selected = menu
            .items
            .iter()
            .position(|item| {
                matches!(
                    item["label"]
                        .as_str()
                        .unwrap_or("")
                        .split(['(', ':', ' '])
                        .next(),
                    Some("length" | "Length")
                )
            })
            .expect("Expected a length completion");
        let before_accept = self.editor.document.buffer.text().to_string();
        self.editor.handle_key(Key::Tab);
        self.wait("automatic_completion_accept", |e| !e.lsp.pending);
        if Language::for_path(&self.main) == Some(Language::CSharp) {
            assert_eq!(
                self.editor.view,
                View::Editing,
                "Plain completion must accept directly"
            );
        }
        let mut previews = 0;
        for step in 0..2 {
            if !self
                .editor
                .lsp
                .panel
                .as_ref()
                .is_some_and(|p| p.title.starts_with("Apply"))
            {
                break;
            }
            if step == 0 {
                assert_eq!(
                    self.editor.document.buffer.text().to_string(),
                    before_accept
                );
            }
            previews += 1;
            self.editor.handle_key(Key::Enter);
            self.wait(&format!("automatic_completion_preview_{step}"), |e| {
                !e.lsp.pending
            });
        }
        self.report["checks"]["automatic_completion_accept"]["previews"] = json!(previews);
        assert!(!self.editor.message_is_error, "{}", self.state());
        assert_eq!(self.editor.view, View::Editing);
        assert_eq!(self.editor.mode, editor_pro::editor::Mode::Insert);
        assert!(
            self.editor
                .document
                .buffer
                .text()
                .to_string()
                .contains(member.trim_end_matches("()")),
            "{}",
            self.state()
        );
        assert!(self.editor.document.buffer.undo());
        assert_eq!(
            self.editor.document.buffer.text().to_string(),
            before_accept
        );
        self.editor.handle_key(Key::Ctrl('e'));
        self.editor.handle_key(Key::Escape);
        assert!(self.editor.document.buffer.undo());
        assert!(self.editor.document.buffer.undo());
        assert_eq!(
            self.editor.document.buffer.text().to_string(),
            before_completion
        );

        let type_error = if Language::for_path(&self.main) == Some(Language::CSharp) {
            "1L"
        } else {
            "\"wrong\""
        };
        self.replace(type_error, "1");
        self.wait("unsaved_diagnostic_cleared", |e| {
            Self::error_on_marker(e) == Some(false)
        });
        self.report["diagnostic_version_verified"] =
            json!(self.editor.current_diagnostics().unwrap().versioned);

        self.replace("greet(\"reader\")", "greet(  \"reader\"  )");
        let unformatted = self.editor.document.buffer.text().to_string();
        self.command("Format");
        assert!(
            self.editor
                .lsp
                .panel
                .as_ref()
                .is_some_and(|p| p.title.starts_with("Apply"))
        );
        self.editor.handle_key(Key::Enter);
        assert!(!self.editor.message_is_error, "{}", self.state());
        assert_ne!(self.editor.document.buffer.text().to_string(), unformatted);
        assert!(self.editor.document.buffer.undo());
        assert_eq!(self.editor.document.buffer.text().to_string(), unformatted);
        assert!(self.editor.document.buffer.undo());
        self.report["checks"]["format_apply_undo"] = json!({"status":"passed"});

        self.cursor("DEFINITION", "greet", 1);
        self.command("Rename welcome");
        assert!(
            self.editor
                .lsp
                .panel
                .as_ref()
                .is_some_and(|p| p.title.starts_with("Apply"))
        );
        self.editor.handle_key(Key::Escape);
        assert!(
            !self
                .editor
                .document
                .buffer
                .text()
                .to_string()
                .contains("welcome")
        );
        self.command("Rename welcome");
        self.editor.handle_key(Key::Enter);
        assert!(!self.editor.message_is_error, "{}", self.state());
        assert!(
            self.editor
                .document
                .buffer
                .text()
                .to_string()
                .contains("welcome")
        );
        assert_eq!(self.editor.dirty_buffer_count(), 2);
        assert!(self.editor.execute_command("q").is_err());
        assert!(self.editor.document.buffer.undo());
        self.editor.open_buffer(&self.other).unwrap();
        assert!(
            self.editor
                .document
                .buffer
                .text()
                .to_string()
                .contains("welcome")
        );
        assert!(self.editor.document.buffer.undo());
        self.editor.open_buffer(&self.main).unwrap();
        self.report["checks"]["rename_preview_apply_cancel_undo"] = json!({"status":"passed"});

        let epoch = self.editor.lsp.servers.values().next().unwrap().epoch;
        self.command("LspRestart");
        self.wait("restart_unsaved_resync", |e| {
            e.lsp
                .servers
                .values()
                .any(|s| s.epoch != epoch && s.capabilities.is_object())
                && Self::error_on_marker(e) == Some(false)
        });
        self.cursor("DEFINITION", "greet", 1);
        self.command("Hover");
        assert!(self.panel_contains("greet"));
        self.editor.handle_key(Key::Escape);
        assert_eq!(self.editor.view, View::Editing);
        for (path, bytes) in &self.originals {
            assert_eq!(
                &fs::read(path).unwrap(),
                bytes,
                "Changed disk file: {}",
                path.display()
            );
        }
        self.report["checks"]["disk_unchanged"] = json!({"status":"passed"});
        assert!(
            self.report["checks"]
                .as_object()
                .unwrap()
                .values()
                .all(|check| check["status"] == "passed"),
            "Some essential features failed: {}",
            self.report["checks"]
        );
        self.report["status"] = json!("passed");
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        self.report["final"] = self.state();
        if thread::panicking() {
            self.report["status"] = json!("failed");
        }
        if let Some(directory) = std::env::var_os("EDITOR_PRO_LSP_REPORT_DIR") {
            let directory = PathBuf::from(directory);
            let result = fs::create_dir_all(&directory).and_then(|()| {
                fs::write(
                    directory.join(format!("{}.json", self.name)),
                    serde_json::to_vec_pretty(&self.report).unwrap(),
                )
            });
            if let Err(error) = result {
                eprintln!("Cannot write acceptance report: {error}");
            }
        }
        eprintln!("{}: {}", self.name, self.report["status"]);
    }
}

#[test]
#[ignore = "requires pinned official Kotlin LSP and Maven dependencies; see tools/lsp/README.md"]
fn kotlin_maven() {
    Project::new(
        "kotlin-maven",
        "src/main/kotlin/Main.kt",
        "src/main/kotlin/Greeter.kt",
    )
    .run();
}

#[test]
#[ignore = "requires pinned JDT LS and Maven dependencies; see tools/lsp/README.md"]
fn java_maven() {
    Project::new(
        "java-maven",
        "src/main/java/probe/Main.java",
        "src/main/java/probe/Greeter.java",
    )
    .run();
}

#[test]
#[ignore = "requires pinned csharp-ls and .NET SDK; see tools/lsp/README.md"]
fn csharp_project() {
    Project::new("csharp-project", "Main.cs", "Greeter.cs").run();
}

#[test]
#[ignore = "requires pinned official Kotlin LSP, Gradle and JDK 21; see tools/lsp/README.md"]
fn kotlin_gradle() {
    Project::new(
        "kotlin-gradle",
        "app/src/main/kotlin/Main.kt",
        "library/src/main/kotlin/Greeter.kt",
    )
    .run();
}

#[test]
#[ignore = "requires pinned JDT LS, Gradle and JDK 21; see tools/lsp/README.md"]
fn java_gradle() {
    Project::new(
        "java-gradle",
        "app/src/main/java/probe/Main.java",
        "library/src/main/java/probe/Greeter.java",
    )
    .run();
}

#[test]
#[ignore = "requires pinned csharp-ls and .NET SDK; see tools/lsp/README.md"]
fn csharp_solution() {
    Project::new("csharp-solution", "App/Main.cs", "Library/Greeter.cs").run();
}
