//! Opt-in startup comparison using real servers, fresh project copies and the
//! production manager. Empty/error responses never count as useful requests.
use editor_pro::{
    lsp::{self, Config, Event, Language, Request, Snapshot, Stamp},
    lsp_worker::{Control, Worker},
};
use ropey::Rope;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

fn copy(source: &Path, target: &Path, digest: &mut Sha256) {
    fs::create_dir_all(target).unwrap();
    let mut entries: Vec<_> = fs::read_dir(source).unwrap().map(Result::unwrap).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        digest.update(entry.file_name().to_string_lossy().as_bytes());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target.join(entry.file_name()), digest);
        } else {
            let bytes = fs::read(entry.path()).unwrap();
            digest.update(&bytes);
            fs::write(target.join(entry.file_name()), bytes).unwrap();
        }
    }
}

fn contains_target(value: &Value, uri: &str) -> bool {
    value["uri"] == uri
        || value["targetUri"] == uri
        || value
            .as_array()
            .is_some_and(|items| items.iter().any(|item| contains_target(item, uri)))
}

fn measure(name: &str, main: &str, target: &str, warm: bool, browse: Duration) -> Value {
    let directory = tempfile::Builder::new()
        .prefix("editor-pro-startup-")
        .tempdir()
        .unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut digest = Sha256::new();
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tools/lsp/fixtures")
            .join(name),
        &root,
        &mut digest,
    );
    fs::write(root.join(".git"), "").unwrap();
    let main = root.join(main);
    let target_uri = lsp::file_uri(&root.join(target)).unwrap();
    let original = fs::read_to_string(&main).unwrap();
    let text = Rope::from_str(&original);
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        if line.contains("DEFINITION") {
            offset += line[..line.find("greet").unwrap()].chars().count() + 1;
            break;
        }
        offset += line.chars().count();
    }
    let language = Language::for_path(&main).unwrap();
    let mut config = Config {
        restart_limit: 0,
        ..Config::default()
    };
    config.installation.directory =
        std::env::var_os("EDITOR_PRO_LSP_INSTALL_DIR").map(PathBuf::from);
    assert!(
        config.installation.directory.is_some()
            || std::env::var_os(language.environment()).is_some(),
        "Set an installed server argv or EDITOR_PRO_LSP_INSTALL_DIR; this benchmark never installs servers"
    );
    let stamp = Stamp {
        buffer_id: 1,
        instance: 1,
        generation: 0,
    };
    let snapshot = Snapshot {
        stamp: stamp.clone(),
        path: main.clone(),
        text: text.clone(),
        saved: 0,
        language,
    };
    let started = Instant::now();
    let mut worker = warm.then(|| Worker::new(config.clone()).unwrap());
    if let Some(worker) = &worker {
        worker.prepare(Some(root.clone()));
    }
    let mut opened = None;
    let mut initialized = None;
    let mut diagnostic = None;
    let mut useful = None;
    let mut ready = false;
    let mut pending = false;
    let mut serial = 0;
    let mut next_request = Instant::now();
    let mut logs = Vec::new();
    let mut phases = Vec::new();
    let mut attempts = Vec::new();
    let mut statuses = Vec::new();
    let mut epochs = std::collections::BTreeSet::new();
    let deadline = browse + Duration::from_secs(240);
    while started.elapsed() < deadline {
        if opened.is_none() && started.elapsed() >= browse {
            let worker = worker.get_or_insert_with(|| Worker::new(config.clone()).unwrap());
            worker.sync(vec![snapshot.clone()]);
            opened = Some(started.elapsed().as_secs_f64());
        }
        if let Some(worker) = &mut worker {
            while let Some(event) = worker.poll() {
                match event {
                    Event::Status {
                        key,
                        epoch,
                        state,
                        capabilities,
                    } => {
                        if key.language != language {
                            continue;
                        }
                        epochs.insert(epoch);
                        if let Some(capabilities) = capabilities {
                            ready = capabilities.is_object();
                            if ready && initialized.is_none() {
                                initialized = Some(started.elapsed().as_secs_f64());
                            }
                        }
                        statuses.push(
                            json!({"seconds": started.elapsed().as_secs_f64(), "message": state.message, "phase": state.phase.label(), "progress": state.progress}),
                        );
                    }
                    Event::Log { message, .. } => {
                        if [
                            "Executable discovery:",
                            "Runtime probe:",
                            "Process spawn:",
                            "Initialize response:",
                            "Initial document synchronization:",
                        ]
                        .iter()
                        .any(|prefix| message.contains(prefix))
                        {
                            phases.push(message.clone());
                        }
                        logs.push(message);
                        if logs.len() > 128 {
                            logs.remove(0);
                        }
                    }
                    Event::Diagnostics { items, .. }
                        if !items.is_empty() && diagnostic.is_none() =>
                    {
                        diagnostic = Some(started.elapsed().as_secs_f64());
                    }
                    Event::Response { result, .. } => {
                        pending = false;
                        let successful = result
                            .as_ref()
                            .is_ok_and(|value| contains_target(value, target_uri.as_str()));
                        attempts.push(json!({"seconds": started.elapsed().as_secs_f64(), "successful": successful, "result": result}));
                        if successful {
                            useful = Some(started.elapsed().as_secs_f64());
                        }
                        next_request = Instant::now() + Duration::from_millis(250);
                    }
                    _ => {}
                }
            }
            if useful.is_some() {
                break;
            }
            if opened.is_some() && ready && !pending && Instant::now() >= next_request {
                serial += 1;
                worker.submit(Control::Request(Request {
                    serial, stamp: stamp.clone(), path: main.clone(), method: "textDocument/definition".into(),
                    params: json!({"textDocument":{"uri":lsp::file_uri(&main).unwrap()}, "position":lsp::position(&text, offset).unwrap()}),
                })).unwrap();
                pending = true;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    let result = json!({
        "fixture": name, "fixture_sha256": format!("{:x}", digest.finalize()),
        "mode": if warm {"warm"} else {"lazy"}, "browse_seconds": browse.as_secs_f64(),
        "file_open_seconds": opened, "initialize_seconds_from_launch": initialized,
        "diagnostic_seconds_from_launch": diagnostic, "useful_request_seconds_from_launch": useful,
        "useful_request_seconds_from_open": useful.zip(opened).map(|(request, open)| request - open),
        "session_epochs": epochs.len(), "phases": phases, "attempts": attempts, "statuses": statuses, "logs": logs,
        "passed": useful.is_some() && epochs.len() == 1,
    });
    drop(worker);
    assert_eq!(fs::read_to_string(main).unwrap(), original);
    result
}

fn compare(name: &str, main: &str, target: &str) {
    let browse = Duration::from_millis(
        std::env::var("EDITOR_PRO_LSP_BROWSE_MS")
            .ok()
            .map(|s| s.parse().unwrap())
            .unwrap_or(10_000),
    );
    let mut results = Vec::new();
    // Alternate order to expose cache/order effects; dependencies may be cached,
    // but every run has a fresh project path and a fresh server/JDT workspace.
    for warm in [false, true, true, false] {
        let result = measure(name, main, target, warm, browse);
        eprintln!(
            "{name}: {} request after open={}s, launch={}s",
            result["mode"],
            result["useful_request_seconds_from_open"],
            result["useful_request_seconds_from_launch"]
        );
        results.push(result);
        if let Some(directory) = std::env::var_os("EDITOR_PRO_LSP_REPORT_DIR") {
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                Path::new(&directory).join(format!("startup-{name}.json")),
                serde_json::to_vec_pretty(&json!({
                    "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
                    "dependency_caches": "shared; not cleared by this harness", "runs": results,
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }
    assert!(
        results.iter().all(|result| result["passed"] == true),
        "Startup failed; inspect startup-{name}.json for failed/empty requests and server logs"
    );
}

#[test]
#[ignore = "real Kotlin Gradle startup benchmark; see tools/lsp/README.md"]
fn kotlin_gradle_startup() {
    compare(
        "kotlin-gradle",
        "app/src/main/kotlin/Main.kt",
        "library/src/main/kotlin/Greeter.kt",
    );
}

#[test]
#[ignore = "real JDT Maven startup benchmark; see tools/lsp/README.md"]
fn java_maven_startup() {
    compare(
        "java-maven",
        "src/main/java/probe/Main.java",
        "src/main/java/probe/Greeter.java",
    );
}

#[test]
#[ignore = "real JDT Gradle startup benchmark; see tools/lsp/README.md"]
fn java_gradle_startup() {
    compare(
        "java-gradle",
        "app/src/main/java/probe/Main.java",
        "library/src/main/java/probe/Greeter.java",
    );
}

#[test]
#[ignore = "real Roslyn solution startup benchmark; see tools/lsp/README.md"]
fn csharp_solution_startup() {
    compare("csharp-solution", "App/Main.cs", "Library/Greeter.cs");
}
