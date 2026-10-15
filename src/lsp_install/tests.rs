use super::*;
use crate::{editor::Editor, file::Document};
use std::{net::TcpListener, sync::atomic::AtomicUsize};

struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(body: Vec<u8>, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/server", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let thread = thread::spawn(move || {
            'connections: while !stopped.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                // Accepted sockets inherit O_NONBLOCK on macOS, unlike Linux.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut bytes = [0; 4096];
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let n = match socket.read(&mut bytes) {
                        Ok(0) | Err(_) => continue 'connections,
                        Ok(n) => n,
                    };
                    request.extend_from_slice(&bytes[..n]);
                    if request.len() > 16 * 1024 {
                        continue 'connections;
                    }
                }
                count.fetch_add(1, Ordering::Relaxed);
                let _ = write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                for chunk in body.chunks(32) {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(delay);
                    if socket.write_all(chunk).is_err() {
                        break;
                    }
                }
                // Let the client consume the response before closing. macOS can
                // reject socket timeout changes after a peer closes immediately.
                // Content-Length lets the client finish without waiting for EOF.
                while !stopped.load(Ordering::Relaxed) {
                    match socket.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.take().unwrap().join();
    }
}

#[test]
fn http_fixture_waits_for_complete_request_headers() {
    let body = b"fixture";
    let server = Server::new(body.to_vec(), Duration::ZERO);
    let address = server
        .url
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut socket = std::net::TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(25)))
        .unwrap();
    socket
        .write_all(b"GET /server HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    let error = socket.read(&mut [0; 1]).unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
    assert_eq!(server.requests.load(Ordering::Relaxed), 0);
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket.write_all(b"\r\n").unwrap();
    let expected = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\nfixture",
        body.len()
    );
    let mut response = vec![0; expected.len()];
    socket.read_exact(&mut response).unwrap();
    assert_eq!(response, expected.as_bytes());
    assert_eq!(server.requests.load(Ordering::Relaxed), 1);
}

fn tarball(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    for (path, bytes, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        tar.append_data(&mut header, path, *bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}
fn package(log: &Path) -> Vec<u8> {
    let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/lsp/fake_server.py");
    let launcher = format!(
        "#!/bin/sh\nexec python3 '{}' --log '{}' \"$@\"\n",
        fake.display(),
        log.display()
    );
    tarball(&[
        (
            "kotlin-server-test/bin/intellij-server",
            launcher.as_bytes(),
            0o755,
        ),
        (
            &format!("kotlin-server-test/{}", bundled_java()),
            b"#!/bin/sh\nprintf 'openjdk version \"25.0.1\"\\n'\n",
            0o755,
        ),
    ])
}
fn test_recipe(body: &[u8], url: String) -> Recipe {
    Recipe {
        version: "test".into(),
        url,
        sha256: format!("{:x}", Sha256::digest(body)),
        format: Format::Tar,
    }
}
fn options(root: &Path, recipe: Recipe) -> Options {
    Options {
        directory: Some(root.to_path_buf()),
        auto_install: true,
        recipe: Some(recipe),
        ignore_path: true,
    }
}
fn guard() -> Guard {
    Guard::new(Arc::new(AtomicBool::new(false)))
}
fn wait(editor: &mut Editor, condition: impl Fn(&Editor) -> bool) {
    let start = Instant::now();
    loop {
        editor.tick();
        if condition(editor) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "Timed out: {:?}; {}",
            editor.lsp.installations,
            editor.message
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn first_install_starts_all_projects_with_current_unsaved_snapshots_and_reuses_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let log = root.join("protocol.jsonl");
    let body = package(&log);
    let server = Server::new(body.clone(), Duration::from_millis(25));
    let recipe = test_recipe(&body, server.url.clone());
    let data = root.join("managed servers");
    let mut paths = Vec::new();
    for project in ["one", "two"] {
        let dir = root.join(project);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("pom.xml"), "<project/>").unwrap();
        let path = dir.join("Main.kt");
        fs::write(&path, "bad value\r\n😀\r\n").unwrap();
        paths.push(path);
    }
    let mut editor = Editor::new(Document::open(&paths[0]).unwrap());
    editor.lsp.config.installation = options(&data, recipe.clone());
    editor.open_buffer(&paths[1]).unwrap();
    wait(&mut editor, |_| server.requests.load(Ordering::Relaxed) > 0);
    editor.document.buffer.insert("unsaved ");
    let current = editor.document.buffer.text().to_string();
    wait(&mut editor, |e| {
        e.lsp.servers.len() == 2 && e.lsp.servers.values().all(|s| s.capabilities.is_object())
    });
    assert_eq!(server.requests.load(Ordering::Relaxed), 1);
    assert_eq!(editor.document.buffer.text().to_string(), current);
    wait(&mut editor, |_| {
        fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("textDocument/didOpen"))
            .count()
            == 2
    });
    let messages: Vec<serde_json::Value> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m["method"] == "textDocument/didOpen"
                && m["params"]["textDocument"]["text"] == current)
    );
    for path in &paths {
        assert_eq!(fs::read_to_string(path).unwrap(), "bad value\r\n😀\r\n");
    }
    drop(editor);
    drop(server);
    let mut editor = Editor::new(Document::open(&paths[0]).unwrap());
    editor.lsp.config.installation = options(&data, recipe);
    wait(&mut editor, |e| {
        e.lsp.servers.values().any(|s| s.capabilities.is_object())
    });
    assert!(editor.lsp.installations.is_empty());
}

#[test]
fn cancellation_cleans_staging_and_explicit_retry_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let body = package(&temp.path().join("log"));
    let server = Server::new(body.clone(), Duration::from_millis(40));
    let recipe = test_recipe(&body, server.url.clone());
    let root = temp.path().join("servers");
    let mut installer = Installer::new(options(&root, recipe.clone()));
    installer.start(Language::Kotlin).unwrap();
    while server.requests.load(Ordering::Relaxed) == 0 {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(installer.cancel(Language::Kotlin));
    let mut states = Vec::new();
    let start = Instant::now();
    while installer.busy() {
        states.extend(installer.poll());
        thread::sleep(Duration::from_millis(10));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    assert!(states.last().unwrap().1.failed);
    assert!(!recipe.destination(&root, Language::Kotlin).exists());
    assert_eq!(fs::read_dir(root.join("kotlin")).unwrap().count(), 1); // persistent lock only
    installer.automatic(Language::Kotlin).unwrap();
    assert!(!installer.busy());
    installer.start(Language::Kotlin).unwrap();
    while installer.busy() {
        states.extend(installer.poll());
        thread::sleep(Duration::from_millis(10));
        assert!(start.elapsed() < Duration::from_secs(8));
    }
    assert!(
        !states.last().unwrap().1.failed,
        "{}",
        states.last().unwrap().1.message
    );
    assert!(installed(&recipe.destination(&root, Language::Kotlin), &recipe).unwrap());
}

#[test]
fn checksum_and_network_errors_leave_no_active_installation() {
    let temp = tempfile::tempdir().unwrap();
    let body = package(&temp.path().join("log"));
    let server = Server::new(body.clone(), Duration::ZERO);
    let mut recipe = test_recipe(&body, server.url.clone());
    recipe.sha256 = "00".repeat(32);
    let root = temp.path().join("servers");
    assert!(
        install(&root, Language::Kotlin, &recipe, &guard(), &|_| {})
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    assert!(!recipe.destination(&root, Language::Kotlin).exists());
    drop(server);
    assert!(install(&root, Language::Kotlin, &recipe, &guard(), &|_| {}).is_err());
    assert_eq!(fs::read_dir(root.join("kotlin")).unwrap().count(), 1);
}

#[test]
fn independent_installers_share_os_lock_and_one_download() {
    let temp = tempfile::tempdir().unwrap();
    let body = package(&temp.path().join("log"));
    let server = Server::new(body.clone(), Duration::from_millis(5));
    let recipe = test_recipe(&body, server.url.clone());
    let root = temp.path().join("servers");
    let a = root.clone();
    let ar = recipe.clone();
    let thread = thread::spawn(move || install(&a, Language::Kotlin, &ar, &guard(), &|_| {}));
    install(&root, Language::Kotlin, &recipe, &guard(), &|_| {}).unwrap();
    thread.join().unwrap().unwrap();
    assert_eq!(server.requests.load(Ordering::Relaxed), 1);
}

#[test]
fn disabled_policy_and_explicit_overrides_never_download() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Main.kt");
    fs::write(&path, "text").unwrap();
    let body = package(&temp.path().join("log"));
    let server = Server::new(body.clone(), Duration::ZERO);
    let mut editor = Editor::new(Document::open(&path).unwrap());
    editor.lsp.config.installation = options(
        &temp.path().join("servers"),
        test_recipe(&body, server.url.clone()),
    );
    editor.lsp.config.installation.auto_install = false;
    wait(&mut editor, |e| !e.lsp.servers.is_empty());
    assert_eq!(server.requests.load(Ordering::Relaxed), 0);
    editor.execute_command("LspInstall").unwrap();
    wait(&mut editor, |e| {
        e.lsp.servers.values().any(|s| s.capabilities.is_object())
    });
    assert_eq!(server.requests.load(Ordering::Relaxed), 1);
    drop(editor);
    let mut editor = Editor::new(Document::open(&path).unwrap());
    editor.lsp.config.installation = options(
        &temp.path().join("other"),
        test_recipe(&body, server.url.clone()),
    );
    editor
        .lsp
        .config
        .commands
        .insert(Language::Kotlin, vec!["/missing/override".into()]);
    wait(&mut editor, |e| {
        e.lsp
            .servers
            .values()
            .any(|s| s.status.contains("Launch failed"))
    });
    assert_eq!(server.requests.load(Ordering::Relaxed), 1);
    assert!(!temp.path().join("other").exists());
    editor.execute_command("LspInstallCancel").unwrap();
    wait(&mut editor, |e| {
        e.lsp.installations.values().all(|s| !s.busy)
    });
}

#[test]
fn runtime_checks_reject_old_failed_and_hanging_processes() {
    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("runtime");
    use std::os::unix::fs::PermissionsExt;
    for (text, message) in [
        (
            "#!/bin/sh\nprintf 'openjdk version \"17.0.1\"\\n'\n",
            "requires Java 21",
        ),
        ("#!/bin/sh\nexit 7\n", "exited with"),
        ("#!/bin/sh\nsleep 60\n", "timed out"),
    ] {
        fs::write(&script, text).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime {
            program: script.clone(),
            minimum: 21,
            dotnet: false,
        };
        assert!(
            runtime
                .check(&guard())
                .unwrap_err()
                .to_string()
                .contains(message)
        );
    }
    fs::write(
        &script,
        "#!/bin/sh\nprintf '8.0.100 [/sdk]\\n10.0.402 [/sdk]\\n'\n",
    )
    .unwrap();
    assert!(
        Runtime {
            program: script,
            minimum: 10,
            dotnet: true
        }
        .check(&guard())
        .is_ok()
    );
}

#[test]
fn zip_traversal_and_escaping_links_are_rejected_and_internal_links_work() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("test.zip");
    for target in [
        "../escape",
        "/absolute",
        "nested/../../escape",
        "C:\\escape",
    ] {
        let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
        zip.start_file(target, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        let dir = tempfile::tempdir_in(temp.path()).unwrap();
        assert!(archive::extract(&source, dir.path(), Format::Zip, &guard()).is_err());
    }
    for target in ["../../escape", "/absolute"] {
        let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
        zip.add_symlink(
            "nested/link",
            target,
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        let dir = tempfile::tempdir_in(temp.path()).unwrap();
        assert!(archive::extract(&source, dir.path(), Format::Zip, &guard()).is_err());
    }
    let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
    zip.start_file(
        "real",
        zip::write::SimpleFileOptions::default().unix_permissions(0o755),
    )
    .unwrap();
    zip.write_all(b"value").unwrap();
    zip.add_symlink(
        "nested/link",
        "../real",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    let dir = tempfile::tempdir_in(temp.path()).unwrap();
    archive::extract(&source, dir.path(), Format::Zip, &guard()).unwrap();
    assert_eq!(fs::read(dir.path().join("nested/link")).unwrap(), b"value");
    assert!(is_executable(&dir.path().join("real")));
}

#[test]
fn extraction_rejects_duplicate_files_truncation_and_cancelled_work() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("archive");
    let body = tarball(&[("same", b"one", 0o600), ("same", b"two", 0o600)]);
    fs::write(&source, &body).unwrap();
    let output = temp.path().join("output");
    fs::create_dir(&output).unwrap();
    assert!(archive::extract(&source, &output, Format::Tar, &guard()).is_err());
    fs::write(&source, &body[..body.len() / 2]).unwrap();
    assert!(archive::extract(&source, &output, Format::Tar, &guard()).is_err());
    let guard = guard();
    guard.cancel.store(true, Ordering::Relaxed);
    fs::write(&source, body).unwrap();
    assert!(archive::extract(&source, &output, Format::Tar, &guard).is_err());
}

#[test]
fn platform_catalog_has_pinned_recipes_and_rejects_unsupported_platforms() {
    for os in ["linux", "macos"] {
        for arch in ["x86_64", "aarch64"] {
            for language in [Language::Kotlin, Language::Java, Language::CSharp] {
                let recipe = recipe_for(language, os, arch).unwrap();
                assert!(recipe.url.starts_with("https://"));
                assert_eq!(recipe.sha256.len(), 64);
            }
        }
    }
    assert!(recipe_for(Language::Kotlin, "windows", "x86_64").is_err());
    assert!(recipe_for(Language::Java, "linux", "riscv64").is_err());
}
