//! One concrete language-server manager. Process, framing, discovery and protocol
//! work run on its runtime thread; the editor submits cheap immutable snapshots.
use crate::lsp::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{Instant, timeout},
};

const QUEUE: usize = 32;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

fn settings(language: Language) -> Value {
    match language {
        // JDT LS advertises signature help even when its preference is off.
        Language::Java => json!({"java":{"signatureHelp":{"enabled":true}}}),
        _ => Value::Null,
    }
}

pub enum Control {
    Request(Request),
    Restart(PathBuf),
    Cancel(PathBuf),
    Doctor(PathBuf),
    ApplyReply {
        key: ServerKey,
        epoch: u64,
        id: Value,
        applied: bool,
        reason: String,
    },
}

pub struct Worker {
    documents: watch::Sender<Vec<Snapshot>>,
    commands: mpsc::Sender<Control>,
    results: mpsc::Receiver<Event>,
    stop: watch::Sender<bool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Worker {
    pub fn new(config: Config) -> io::Result<Self> {
        let (documents, docs) = watch::channel(Vec::new());
        let (commands, requests) = mpsc::channel(QUEUE);
        let (events, results) = mpsc::channel(128);
        let (stop, shutdown) = watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let thread = thread::Builder::new()
            .name("language-servers".into())
            .spawn(move || {
                runtime.block_on(manager(config, docs, requests, events, shutdown));
            })?;
        Ok(Self {
            documents,
            commands,
            results,
            stop,
            thread: Some(thread),
        })
    }
    pub fn sync(&self, documents: Vec<Snapshot>) {
        self.documents.send_replace(documents);
    }
    pub fn submit(&self, command: Control) -> io::Result<()> {
        self.commands
            .try_send(command)
            .map_err(|e| io::Error::other(format!("LSP queue unavailable: {e}")))
    }
    pub fn poll(&mut self) -> Option<Event> {
        self.results.try_recv().ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Session {
    documents: watch::Sender<Vec<Snapshot>>,
    commands: mpsc::Sender<Control>,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

async fn manager(
    config: Config,
    mut documents: watch::Receiver<Vec<Snapshot>>,
    mut commands: mpsc::Receiver<Control>,
    events: mpsc::Sender<Event>,
    mut stop: watch::Receiver<bool>,
) {
    let mut routes: BTreeMap<PathBuf, Result<ServerKey, String>> = BTreeMap::new();
    let mut sessions: BTreeMap<ServerKey, Session> = BTreeMap::new();
    loop {
        let control = tokio::select! {
            _ = stop.changed() => break,
            changed = documents.changed() => { if changed.is_err() { break; } None },
            command = commands.recv() => { let Some(command) = command else { break }; Some(command) },
        };
        let docs = documents.borrow_and_update().clone();
        if let Some(Control::Doctor(path)) = &control {
            let report = doctor(path, &config).await;
            let _ = events.try_send(Event::Doctor {
                path: path.clone(),
                report,
            });
        }
        if let Some(Control::Restart(path)) = &control
            && let Some(Ok(key)) = routes.remove(path)
        {
            routes.retain(|_, value| value.as_ref().ok() != Some(&key));
            if let Some(session) = sessions.remove(&key) {
                close_session(session).await;
            }
        }
        let mut groups: BTreeMap<ServerKey, Vec<Snapshot>> = BTreeMap::new();
        for doc in docs {
            let route = routes.entry(doc.path.clone()).or_insert_with(|| {
                project_root(&config, &doc.path, doc.language)
                    .map(|root| ServerKey {
                        root,
                        language: doc.language,
                    })
                    .map_err(|e| e.to_string())
            });
            match route {
                Ok(key) => {
                    let _ = events.try_send(Event::Route {
                        path: doc.path.clone(),
                        key: key.clone(),
                    });
                    groups.entry(key.clone()).or_default().push(doc);
                }
                Err(message) => {
                    let _ = events.try_send(Event::Unavailable {
                        path: doc.path,
                        message: message.clone(),
                    });
                }
            }
        }
        let obsolete: Vec<_> = sessions
            .keys()
            .filter(|key| !groups.contains_key(*key))
            .cloned()
            .collect();
        for key in obsolete {
            if let Some(session) = sessions.remove(&key) {
                close_session(session).await;
            }
        }
        for (key, docs) in groups {
            if let Some(session) = sessions.get(&key) {
                let unchanged = {
                    let previous = session.documents.borrow();
                    previous.len() == docs.len()
                        && previous.iter().zip(&docs).all(|(a, b)| a.same_document(b))
                };
                if !unchanged {
                    session.documents.send_replace(docs);
                }
                continue;
            }
            if sessions.len() >= 8 {
                for doc in docs {
                    let _ = events.try_send(Event::Unavailable {
                        path: doc.path,
                        message: "LSP limit: eight project/language sessions".into(),
                    });
                }
                continue;
            }
            let (sender, receiver) = watch::channel(docs);
            let (requests, input) = mpsc::channel(QUEUE);
            let (shutdown, stopped) = watch::channel(false);
            let task = tokio::spawn(session_loop(
                key.clone(),
                config.clone(),
                receiver,
                input,
                events.clone(),
                stopped,
            ));
            sessions.insert(
                key,
                Session {
                    documents: sender,
                    commands: requests,
                    stop: shutdown,
                    task,
                },
            );
        }
        if let Some(control) = control {
            let key = match &control {
                Control::Request(request) => {
                    routes.get(&request.path).and_then(|r| r.as_ref().ok())
                }
                Control::Cancel(path) => routes.get(path).and_then(|r| r.as_ref().ok()),
                Control::ApplyReply { key, .. } => Some(key),
                Control::Restart(_) | Control::Doctor(_) => None,
            };
            if let Some(session) = key.and_then(|key| sessions.get(key))
                && let Err(error) = session.commands.try_send(control)
                && let Control::Request(request) = error.into_inner()
            {
                let _ = events.try_send(Event::Unavailable {
                    path: request.path,
                    message:
                        "Language server request queue is full or stopped; :LspRestart retries"
                            .into(),
                });
            }
        }
        routes.retain(|path, _| documents.borrow().iter().any(|d| &d.path == path));
    }
    for (_, session) in sessions {
        close_session(session).await;
    }
}

async fn close_session(session: Session) {
    session.stop.send_replace(true);
    // Session teardown owns child reaping; no process waits on the input thread.
    let _ = session.task.await;
}

fn executable(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value)
            .map(|directory| directory.join(name))
            .find(|path| path.is_file())
    })
}

fn project_root(config: &Config, path: &Path, language: Language) -> io::Result<PathBuf> {
    if let Some(root) = config
        .root
        .clone()
        .or_else(|| std::env::var_os("EDITOR_PRO_LSP_ROOT").map(PathBuf::from))
    {
        let root = root.canonicalize()?;
        if !root.is_dir() || !path.starts_with(&root) {
            return Err(io::Error::other("Source is outside EDITOR_PRO_LSP_ROOT"));
        }
        Ok(root)
    } else {
        discover_root(path, language)
    }
}

fn command_for(config: &Config, language: Language) -> io::Result<Vec<OsString>> {
    let override_command = config
        .commands
        .get(&language)
        .cloned()
        .or_else(|| std::env::var_os(language.environment()).map(|value| vec![value]));
    if let Some(mut command) = override_command {
        if command.len() == 1 && command[0].to_string_lossy().starts_with('[') {
            let parsed: Vec<String> =
                serde_json::from_str(&command[0].to_string_lossy()).map_err(|e| {
                    io::Error::other(format!(
                        "{} must be a JSON argv array: {e}",
                        language.environment()
                    ))
                })?;
            command = parsed.into_iter().map(OsString::from).collect();
        }
        if command.is_empty() || command[0].is_empty() {
            return Err(io::Error::other("Empty language-server command"));
        }
        return Ok(command);
    }
    let (server, sdk, arguments): (&str, &str, &[&str]) = match language {
        Language::Kotlin => ("intellij-server", "java", &["--stdio"]),
        Language::Java => ("jdtls", "java", &[]),
        Language::CSharp => ("csharp-ls", "dotnet", &[]),
    };
    let program = executable(server).ok_or_else(|| {
        io::Error::other(format!(
            "Missing {server} on PATH; install it or set {} to a JSON argv array",
            language.environment()
        ))
    })?;
    let sdk_found = executable(sdk).is_some()
        || (sdk == "java"
            && std::env::var_os("JAVA_HOME")
                .is_some_and(|home| Path::new(&home).join("bin/java").is_file()));
    if !sdk_found {
        return Err(io::Error::other(format!(
            "Missing {sdk} runtime/SDK; :Doctor shows discovery details"
        )));
    }
    Ok(std::iter::once(program.into_os_string())
        .chain(arguments.iter().map(OsString::from))
        .collect())
}

fn status(
    events: &mpsc::Sender<Event>,
    key: &ServerKey,
    epoch: u64,
    message: String,
    capabilities: Option<Value>,
) {
    let _ = events.try_send(Event::Status {
        key: key.clone(),
        epoch,
        message,
        capabilities,
    });
}

async fn session_loop(
    key: ServerKey,
    config: Config,
    mut documents: watch::Receiver<Vec<Snapshot>>,
    mut commands: mpsc::Receiver<Control>,
    events: mpsc::Sender<Event>,
    mut stop: watch::Receiver<bool>,
) {
    static EPOCH: AtomicU64 = AtomicU64::new(1);
    let argv = match command_for(&config, key.language) {
        Ok(argv) => argv,
        Err(e) => {
            status(
                &events,
                &key,
                EPOCH.fetch_add(1, Ordering::Relaxed),
                e.to_string(),
                Some(Value::Null),
            );
            // Remain addressable until explicit restart, avoiding a spawn loop on every edit.
            let _ = stop.changed().await;
            return;
        }
    };
    let _ = events.try_send(Event::Log {
        key: key.clone(),
        message: format!("Launch argv: {argv:?}\n"),
    });
    for attempt in 0..=config.restart_limit {
        if *stop.borrow() {
            return;
        }
        let epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
        status(
            &events,
            &key,
            epoch,
            format!("Starting {} (attempt {})", key.language.id(), attempt + 1),
            Some(Value::Null),
        );
        let mut transport = match Transport::start(&argv, &key, &events) {
            Ok(transport) => transport,
            Err(e) => {
                status(
                    &events,
                    &key,
                    epoch,
                    format!("Launch failed: {e}; :LspRestart retries"),
                    Some(Value::Null),
                );
                let _ = stop.changed().await;
                return;
            }
        };
        let result = tokio::select! {
            _ = stop.changed() => Ok(()),
            result = run(&mut transport, &key, epoch, &config, &mut documents, &mut commands, &events) => result,
        };
        transport.close().await;
        if *stop.borrow() {
            return;
        }
        status(
            &events,
            &key,
            epoch,
            format!(
                "LSP stopped: {}; {}",
                result
                    .err()
                    .map_or_else(|| "closed".into(), |e| e.to_string()),
                if attempt < config.restart_limit {
                    "restarting"
                } else {
                    ":LspRestart retries"
                }
            ),
            Some(Value::Null),
        );
        if attempt == config.restart_limit {
            let _ = stop.changed().await;
            return;
        }
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))) => {}
        }
    }
}

struct Transport {
    child: Child,
    input: Option<ChildStdin>,
    frames: mpsc::Receiver<io::Result<Value>>,
    reader: JoinHandle<()>,
    stderr: JoinHandle<()>,
    _workspace: Option<tempfile::TempDir>,
    open_uris: BTreeSet<String>,
}
impl Transport {
    fn start(argv: &[OsString], key: &ServerKey, events: &mpsc::Sender<Event>) -> io::Result<Self> {
        let mut command = Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .current_dir(&key.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // A private JDT workspace avoids concurrent editors sharing mutable indexes.
        let workspace = if key.language == Language::Java {
            let dir = tempfile::Builder::new()
                .prefix("editor-pro-jdtls-")
                .tempdir()?;
            command.arg("-data").arg(dir.path());
            Some(dir)
        } else {
            None
        };
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Missing LSP stdin"))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Missing LSP stdout"))?;
        let mut errors = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Missing LSP stderr"))?;
        let (sender, frames) = mpsc::channel(QUEUE);
        let reader = tokio::spawn(async move {
            loop {
                let frame = read_frame(&mut output).await;
                let failed = frame.is_err();
                if sender.send(frame).await.is_err() || failed {
                    break;
                }
            }
        });
        let events = events.clone();
        let key = key.clone();
        let stderr = tokio::spawn(async move {
            let mut bytes = [0; 4096];
            while let Ok(count) = errors.read(&mut bytes).await {
                if count == 0 {
                    break;
                }
                let _ = events.try_send(Event::Log {
                    key: key.clone(),
                    message: String::from_utf8_lossy(&bytes[..count]).into_owned(),
                });
            }
        });
        Ok(Self {
            child,
            input: Some(input),
            frames,
            reader,
            stderr,
            _workspace: workspace,
            open_uris: BTreeSet::new(),
        })
    }
    async fn send(&mut self, message: Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(&message)?;
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(io::Error::other("LSP outgoing message exceeds 8 MiB"));
        }
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| io::Error::other("LSP stdin is closed"))?;
        timeout(WRITE_TIMEOUT, async {
            input
                .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
                .await?;
            input.write_all(&bytes).await?;
            input.flush().await
        })
        .await
        .map_err(|_| io::Error::other("Language server stdin timed out"))?
    }
    async fn notify(&mut self, method: &str, params: Value) -> io::Result<()> {
        let uri = params["textDocument"]["uri"].as_str().map(str::to_owned);
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await?;
        if let Some(uri) = uri {
            if method == "textDocument/didOpen" {
                self.open_uris.insert(uri);
            } else if method == "textDocument/didClose" {
                self.open_uris.remove(&uri);
            }
        }
        Ok(())
    }
    async fn request(&mut self, id: u64, method: &str, params: Value) -> io::Result<()> {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
    }
    async fn reply(&mut self, id: Value, result: Value) -> io::Result<()> {
        self.send(json!({"jsonrpc":"2.0","id":id,"result":result}))
            .await
    }
    async fn close(&mut self) {
        let _ = timeout(Duration::from_millis(400), async {
            for uri in self.open_uris.clone() {
                self.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
                    .await?;
            }
            self.request(0, "shutdown", Value::Null).await?;
            while let Some(frame) = self.frames.recv().await {
                if frame?["id"] == 0 {
                    break;
                }
            }
            self.notify("exit", Value::Null).await
        })
        .await;
        // EOF lets stdio servers finish their transport loop after exit. Tokio's
        // ChildStdin::shutdown does not close the pipe on Unix; drop it explicitly.
        drop(self.input.take());
        let pid = self.child.id();
        let _ = timeout(Duration::from_millis(200), self.child.wait()).await;
        #[cfg(unix)]
        if let Some(pid) = pid {
            let _ = timeout(
                Duration::from_secs(1),
                Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{pid}")])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status(),
            )
            .await;
        }
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        self.reader.abort();
        self.stderr.abort();
    }
}

pub async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Value> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= 8192 {
            return Err(io::Error::other("LSP header exceeds 8 KiB"));
        }
        header.push(reader.read_u8().await?);
    }
    let header = std::str::from_utf8(&header).map_err(io::Error::other)?;
    let mut length = None;
    for line in header.split("\r\n").filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("Malformed LSP header"))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(io::Error::other("Duplicate Content-Length"));
            }
            length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
        }
    }
    let length = length
        .filter(|len| *len > 0 && *len <= MAX_MESSAGE_BYTES)
        .ok_or_else(|| io::Error::other("Missing or oversized LSP Content-Length (8 MiB limit)"))?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    let value: Value = serde_json::from_slice(&bytes)?;
    if !value.is_object() || value["jsonrpc"] != "2.0" {
        return Err(io::Error::other("Expected a JSON-RPC 2.0 object"));
    }
    Ok(value)
}

struct Pending {
    request: Request,
    documents: Vec<Versioned>,
    deadline: Instant,
}

async fn run(
    wire: &mut Transport,
    key: &ServerKey,
    epoch: u64,
    config: &Config,
    source: &mut watch::Receiver<Vec<Snapshot>>,
    commands: &mut mpsc::Receiver<Control>,
    events: &mpsc::Sender<Event>,
) -> io::Result<()> {
    let root = file_uri(&key.root)?;
    wire.request(1, "initialize", json!({
        "processId":std::process::id(),"clientInfo":{"name":"editor-pro","version":env!("CARGO_PKG_VERSION")},
        "rootUri":root,"workspaceFolders":[{"uri":root,"name":key.root.file_name().unwrap_or_default().to_string_lossy()}],
        "initializationOptions":{"settings":settings(key.language)},
        "capabilities":{
            "general":{"positionEncodings":["utf-16"]},
            "window":{"workDoneProgress":true},
            "workspace":{"applyEdit":true,"workspaceFolders":true,"configuration":true,
                "diagnostics":{"refreshSupport":true},
                "symbol":{"dynamicRegistration":false,"resolveSupport":{"properties":["location.range"]}},
                "workspaceEdit":{"documentChanges":true,"failureHandling":"abort","resourceOperations":[]}},
            "textDocument":{
                "synchronization":{"dynamicRegistration":false,"didSave":true},
                "completion":{"completionItem":{"snippetSupport":false,"documentationFormat":["plaintext","markdown"],
                    "resolveSupport":{"properties":["documentation","detail","additionalTextEdits","command"]}}},
                "hover":{"contentFormat":["plaintext","markdown"]},
                "signatureHelp":{"signatureInformation":{"documentationFormat":["plaintext","markdown"],"parameterInformation":{"labelOffsetSupport":true}}},
                "definition":{"linkSupport":true},
                "documentSymbol":{"hierarchicalDocumentSymbolSupport":true},
                "callHierarchy":{"dynamicRegistration":false},
                "publishDiagnostics":{"versionSupport":true,"relatedInformation":true},
                "diagnostic":{"dynamicRegistration":false},
                "codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["","quickfix","refactor","source"]}},
                    "resolveSupport":{"properties":["edit"]}}
            }
        }
    })).await?;
    let deadline = Instant::now() + config.initialize_timeout;
    let capabilities = loop {
        let frame = timeout(
            deadline.saturating_duration_since(Instant::now()),
            wire.frames.recv(),
        )
        .await
        .map_err(|_| io::Error::other("LSP initialize/import timed out"))?
        .ok_or_else(|| io::Error::other("Language server closed stdout"))??;
        if frame["id"] == 1 && frame.get("method").is_none() {
            if let Some(error) = frame.get("error") {
                return Err(io::Error::other(format!("Initialize failed: {error}")));
            }
            let capabilities = frame["result"]["capabilities"].clone();
            let _ = events.try_send(Event::Log {
                key: key.clone(),
                message: format!("Server info: {}\n", frame["result"]["serverInfo"]),
            });
            if !capabilities.is_object() {
                return Err(io::Error::other("Initialize omitted server capabilities"));
            }
            if capabilities
                .get("positionEncoding")
                .is_some_and(|v| v != "utf-16")
            {
                return Err(io::Error::other(
                    "Server selected unsupported position encoding; UTF-16 required",
                ));
            }
            break capabilities;
        }
        server_message(wire, key, epoch, &frame, &BTreeMap::new(), None, events).await?;
    };
    wire.notify("initialized", json!({})).await?;
    status(
        events,
        key,
        epoch,
        "Ready".into(),
        Some(capabilities.clone()),
    );
    let mut open = BTreeMap::new();
    let mut version = 0;
    let mut next_id = 2;
    let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut pulled: BTreeMap<PathBuf, i32> = BTreeMap::new();
    let mut action: Option<Pending> = None;
    let mut awaiting_edit: Option<u64> = None;
    let mut timer = tokio::time::interval(Duration::from_millis(50));
    loop {
        let snapshots = source.borrow_and_update().clone();
        if synchronize(wire, &capabilities, &mut open, snapshots, &mut version).await?
            && capabilities["diagnosticProvider"]["interFileDependencies"] == true
        {
            pulled.clear();
        }
        pulled.retain(|path, _| open.contains_key(path));
        let obsolete: Vec<_> = pending
            .iter()
            .filter(|(_, p)| !documents_match(&p.documents, &open))
            .map(|(id, _)| *id)
            .collect();
        for id in obsolete {
            wire.notify("$/cancelRequest", json!({"id":id})).await?;
            if let Some(p) = pending.remove(&id)
                && p.request.method == "textDocument/diagnostic"
            {
                pulled.remove(&p.request.path);
            }
        }
        if action.as_ref().is_some_and(|p| {
            !documents_match(&p.documents, &open)
                || (awaiting_edit != Some(p.request.serial) && p.deadline <= Instant::now())
        }) {
            action = None;
            awaiting_edit = None;
        }
        if supports(&capabilities, "textDocument/diagnostic") {
            for document in open
                .values()
                .filter(|d| pulled.get(&d.snapshot.path) != Some(&d.version))
                .cloned()
                .collect::<Vec<_>>()
            {
                if pending.len() >= QUEUE {
                    break;
                }
                let request = Request {
                    serial: 0,
                    stamp: document.snapshot.stamp.clone(),
                    path: document.snapshot.path.clone(),
                    method: "textDocument/diagnostic".into(),
                    params: json!({"textDocument":{"uri":document.snapshot.uri()?}}),
                };
                wire.request(next_id, &request.method, request.params.clone())
                    .await?;
                pulled.insert(document.snapshot.path.clone(), document.version);
                pending.insert(
                    next_id,
                    Pending {
                        request,
                        documents: open.values().cloned().collect(),
                        deadline: Instant::now() + config.request_timeout,
                    },
                );
                next_id += 1;
            }
        }
        tokio::select! {
            changed = source.changed() => { changed.map_err(io::Error::other)?; }
            command = commands.recv() => {
                match command.ok_or_else(|| io::Error::other("LSP command channel closed"))? {
                    Control::Request(request) => {
                        awaiting_edit = None;
                        // The watch channel may have changed since the top of the loop.
                        let snapshots = source.borrow_and_update().clone();
                        if synchronize(wire, &capabilities, &mut open, snapshots, &mut version).await?
                            && capabilities["diagnosticProvider"]["interFileDependencies"] == true {
                            pulled.clear();
                        }
                        let error = if !supports(&capabilities, &request.method) {
                            Some(format!("Server does not support {}", request.method))
                        } else if !open.values().any(|d| d.snapshot.stamp == request.stamp && d.snapshot.path == request.path) {
                            Some("Request became stale before synchronization".into())
                        } else if request.method == "workspace/executeCommand" && !capabilities["executeCommandProvider"]["commands"].as_array()
                            .is_some_and(|items| items.contains(&request.params["command"])) {
                            Some("Server did not advertise this command".into())
                        } else { None };
                        if let Some(error) = error {
                            emit(events, Event::Response { key:key.clone(), epoch, request, documents:Vec::new(), result:Err(error) }).await?;
                            continue;
                        }
                        let previous: Vec<_> = pending.iter().filter(|(_, p)| p.request.serial != 0).map(|(id,_)| *id).collect();
                        for id in previous { wire.notify("$/cancelRequest",json!({"id":id})).await?; pending.remove(&id); }
                        let documents: Vec<_> = open.values().cloned().collect();
                        if request.method == "workspace/executeCommand" {
                            action = Some(Pending { request:request.clone(), documents:documents.clone(), deadline:Instant::now()+config.request_timeout });
                        }
                        wire.request(next_id, &request.method, request.params.clone()).await?;
                        pending.insert(next_id, Pending { request, documents, deadline:Instant::now()+config.request_timeout });
                        next_id += 1;
                    }
                    Control::ApplyReply { epoch: reply_epoch, id, applied, reason, .. } if reply_epoch == epoch => {
                        wire.reply(id, json!({"applied":applied,"failureReason":reason})).await?;
                        if let Some(serial) = awaiting_edit.take() {
                            for p in pending.values_mut().filter(|p|p.request.serial == serial) {
                                p.deadline = Instant::now()+config.request_timeout;
                            }
                        }
                    }
                    Control::Cancel(_) => {
                        let ids: Vec<_> = pending.iter().filter(|(_,p)|p.request.serial != 0).map(|(id,_)|*id).collect();
                        for id in ids {
                            wire.notify("$/cancelRequest",json!({"id":id})).await?;
                            pending.remove(&id);
                        }
                        action = None;
                        awaiting_edit = None;
                    }
                    _ => {}
                }
            }
            frame = wire.frames.recv() => {
                let frame = frame.ok_or_else(|| io::Error::other("Language server closed stdout"))??;
                if frame.get("method").is_some() {
                    if frame["method"] == "workspace/applyEdit" && frame.get("id").is_some()
                        && let Some(p) = &action && p.deadline > Instant::now() && documents_match(&p.documents,&open) {
                        awaiting_edit = Some(p.request.serial);
                    }
                    server_message(wire,key,epoch,&frame,&open,action.as_ref(),events).await?;
                    if frame["method"] == "workspace/diagnostic/refresh" {
                        // A refresh can supersede an in-flight report even when all
                        // document versions are unchanged (for example after indexing).
                        let ids: Vec<_> = pending.iter().filter(|(_, p)| p.request.method == "textDocument/diagnostic")
                            .map(|(id, _)| *id).collect();
                        for id in ids {
                            wire.notify("$/cancelRequest", json!({"id":id})).await?;
                            pending.remove(&id);
                        }
                        pulled.clear();
                    }
                } else if let Some(id) = frame["id"].as_u64() && let Some(p) = pending.remove(&id) {
                    if !documents_match(&p.documents, &open) { continue; }
                    let result = frame.get("error").map_or_else(|| Ok(frame["result"].clone()), |e| Err(format!("LSP error: {e}")));
                    if p.request.method == "textDocument/diagnostic" {
                        if let (Ok(value), Some(document)) = (result, open.get(&p.request.path))
                            && value["kind"] == "full" {
                            let items = diagnostics(&value["items"])?;
                            let dependencies = (capabilities["diagnosticProvider"]["interFileDependencies"] == true)
                                .then(|| p.documents.iter().map(|d| (d.snapshot.path.clone(), d.snapshot.stamp.clone())).collect());
                            emit(events, Event::Diagnostics { key:key.clone(),epoch,document:document.clone(),dependencies,versioned:true,items }).await?;
                        }
                    } else {
                        emit(events, Event::Response { key:key.clone(),epoch,request:p.request,documents:p.documents,result }).await?;
                    }
                }
            }
            _ = timer.tick() => {
                let expired: Vec<_> = pending.iter().filter(|(_,p)| p.deadline <= Instant::now() && awaiting_edit != Some(p.request.serial)).map(|(id,_)| *id).collect();
                for id in expired {
                    wire.notify("$/cancelRequest", json!({"id":id})).await?;
                    if let Some(p) = pending.remove(&id) && p.request.serial != 0 {
                        emit(events, Event::Response { key:key.clone(),epoch,request:p.request,documents:p.documents,result:Err("Language-server request timed out".into()) }).await?;
                    }
                }
            }
        }
    }
}

fn documents_match(documents: &[Versioned], open: &BTreeMap<PathBuf, Versioned>) -> bool {
    documents.iter().all(|d| {
        open.get(&d.snapshot.path)
            .is_some_and(|now| now.snapshot.stamp == d.snapshot.stamp)
    })
}

async fn synchronize(
    wire: &mut Transport,
    capabilities: &Value,
    open: &mut BTreeMap<PathBuf, Versioned>,
    snapshots: Vec<Snapshot>,
    version: &mut i32,
) -> io::Result<bool> {
    let changed = snapshots.len() != open.len()
        || snapshots.iter().any(|snapshot| {
            open.get(&snapshot.path)
                .is_none_or(|d| !d.snapshot.same_document(snapshot))
        });
    let sync = &capabilities["textDocumentSync"];
    let kind = sync
        .as_u64()
        .or_else(|| sync["change"].as_u64())
        .unwrap_or(0);
    let open_close = sync.is_number() || sync["openClose"] == true;
    if kind > 2 {
        return Err(io::Error::other("Unknown textDocumentSync kind"));
    }
    let closed: Vec<_> = open
        .keys()
        .filter(|path| !snapshots.iter().any(|s| &s.path == *path))
        .cloned()
        .collect();
    for path in closed {
        if open_close {
            wire.notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":file_uri(&path)?}}),
            )
            .await?;
        }
        open.remove(&path);
    }
    for snapshot in snapshots {
        let previous = open.get(&snapshot.path);
        let saved_changed = previous.is_some_and(|p| p.snapshot.saved != snapshot.saved);
        let replacement = previous.is_some_and(|p| {
            p.snapshot.stamp.instance != snapshot.stamp.instance
                || p.snapshot.stamp.buffer_id != snapshot.stamp.buffer_id
        });
        if replacement && open_close {
            wire.notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":snapshot.uri()?}}),
            )
            .await?;
        }
        // Versions belong to a document. Keep a session high-water mark only
        // for new/reopened paths so delayed reports cannot match a later open.
        let document_version;
        if previous.is_none() || replacement {
            document_version = previous
                .map_or(*version, |p| p.version)
                .checked_add(1)
                .ok_or_else(|| io::Error::other("LSP version exhausted; restart server"))?;
            if open_close {
                wire.notify("textDocument/didOpen",json!({"textDocument":{
                    "uri":snapshot.uri()?,"languageId":snapshot.language.id(),"version":document_version,"text":snapshot.text.to_string()
                }})).await?;
            }
        } else if let Some(previous) = previous
            && previous.snapshot.stamp != snapshot.stamp
        {
            if kind == 0 {
                return Err(io::Error::other(
                    "Server cannot synchronize unsaved changes (sync kind None)",
                ));
            }
            document_version = previous
                .version
                .checked_add(1)
                .ok_or_else(|| io::Error::other("LSP version exhausted"))?;
            let content = if kind == 1 {
                json!({"text":snapshot.text.to_string()})
            } else {
                change(&previous.snapshot.text, &snapshot.text)?
            };
            wire.notify("textDocument/didChange",json!({"textDocument":{"uri":snapshot.uri()?,"version":document_version},"contentChanges":[content]})).await?;
        } else {
            if let Some(previous) = previous
                && previous.snapshot.saved != snapshot.saved
                && (sync["save"] == true || sync["save"].is_object())
            {
                let mut params = json!({"textDocument":{"uri":snapshot.uri()?}});
                if sync["save"]["includeText"] == true {
                    params["text"] = json!(snapshot.text.to_string());
                }
                wire.notify("textDocument/didSave", params).await?;
            }
            if let Some(previous) = open.get_mut(&snapshot.path) {
                previous.snapshot = snapshot;
            }
            continue;
        }
        *version = (*version).max(document_version);
        let document = Versioned {
            snapshot,
            version: document_version,
        };
        if saved_changed && (sync["save"] == true || sync["save"].is_object()) {
            let mut params = json!({"textDocument":{"uri":document.snapshot.uri()?}});
            if sync["save"]["includeText"] == true {
                params["text"] = json!(document.snapshot.text.to_string());
            }
            wire.notify("textDocument/didSave", params).await?;
        }
        open.insert(document.snapshot.path.clone(), document);
    }
    Ok(changed)
}

fn diagnostics(value: &Value) -> io::Result<Vec<lsp_types::Diagnostic>> {
    let items = value
        .as_array()
        .ok_or_else(|| io::Error::other("Invalid diagnostics"))?;
    items
        .iter()
        .take(MAX_ITEMS)
        .map(|v| serde_json::from_value(v.clone()).map_err(io::Error::other))
        .collect()
}

async fn server_message(
    wire: &mut Transport,
    key: &ServerKey,
    epoch: u64,
    frame: &Value,
    open: &BTreeMap<PathBuf, Versioned>,
    action: Option<&Pending>,
    events: &mpsc::Sender<Event>,
) -> io::Result<()> {
    let method = frame["method"].as_str().unwrap_or("");
    let params = &frame["params"];
    if let Some(id) = frame.get("id") {
        let result = match method {
            "workspace/configuration" => json!(
                params["items"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|item| {
                        let settings = settings(key.language);
                        item["section"]
                            .as_str()
                            .filter(|section| !section.is_empty())
                            .map_or(settings.clone(), |section| {
                                section
                                    .split('.')
                                    .fold(&settings, |value, name| &value[name])
                                    .clone()
                            })
                    })
                    .collect::<Vec<_>>()
            ),
            "workspace/workspaceFolders" => {
                json!([{"uri":file_uri(&key.root)?,"name":key.root.file_name().unwrap_or_default().to_string_lossy()}])
            }
            "window/workDoneProgress/create"
            | "window/showMessageRequest"
            | "workspace/diagnostic/refresh" => Value::Null,
            "workspace/applyEdit" => {
                if let Some(action) = action
                    && action.deadline > Instant::now()
                    && documents_match(&action.documents, open)
                {
                    emit(
                        events,
                        Event::Apply {
                            key: key.clone(),
                            epoch,
                            id: id.clone(),
                            request: action.request.clone(),
                            documents: action.documents.clone(),
                            edit: params["edit"].clone(),
                        },
                    )
                    .await?;
                    return Ok(());
                }
                json!({"applied":false,"failureReason":"No current user-selected action or completion command; unsolicited edits are refused"})
            }
            _ => {
                wire.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("Unsupported client method: {method}")}})).await?;
                return Ok(());
            }
        };
        return wire.reply(id.clone(), result).await;
    }
    match method {
        "textDocument/publishDiagnostics" => {
            if let Some(uri) = params["uri"].as_str()
                && let Ok(path) = uri_path(uri)
                && let Some(document) = open.get(&path)
            {
                let report_version = params["version"].as_i64();
                if report_version.is_some_and(|v| v != i64::from(document.version)) {
                    return Ok(());
                }
                emit(
                    events,
                    Event::Diagnostics {
                        key: key.clone(),
                        epoch,
                        document: document.clone(),
                        dependencies: None,
                        versioned: report_version.is_some(),
                        items: diagnostics(&params["diagnostics"])?,
                    },
                )
                .await?;
            }
        }
        "$/progress" => {
            let value = &params["value"];
            let message = if value["kind"] == "end" {
                "Ready".into()
            } else {
                format!(
                    "Indexing: {} {} {}",
                    value["title"].as_str().unwrap_or(""),
                    value["message"].as_str().unwrap_or(""),
                    value["percentage"]
                        .as_u64()
                        .map_or(String::new(), |p| format!("{p}%"))
                )
            };
            status(events, key, epoch, message, None);
        }
        "window/logMessage" | "window/showMessage" => {
            let _ = events.try_send(Event::Log {
                key: key.clone(),
                message: params["message"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(4096)
                    .collect(),
            });
        }
        _ => {}
    }
    Ok(())
}

async fn emit(events: &mpsc::Sender<Event>, event: Event) -> io::Result<()> {
    timeout(Duration::from_millis(500), events.send(event))
        .await
        .map_err(|_| io::Error::other("LSP result queue saturated"))?
        .map_err(|_| io::Error::other("LSP result receiver closed"))
}

async fn doctor(path: &Path, config: &Config) -> String {
    let Some(language) = Language::for_path(path) else {
        return "Unsupported language".into();
    };
    let mut report = format!(
        "Root discovery: {:?}\nServer discovery: {:?}\nJAVA_HOME: {:?}\n",
        project_root(config, path, language),
        command_for(config, language),
        std::env::var_os("JAVA_HOME")
    );
    let sdk = if language == Language::CSharp {
        "dotnet"
    } else {
        "java"
    };
    let program = if sdk == "java" {
        std::env::var_os("JAVA_HOME")
            .map(|home| Path::new(&home).join("bin/java"))
            .filter(|p| p.is_file())
            .or_else(|| executable(sdk))
    } else {
        executable(sdk)
    };
    let Some(program) = program else {
        return format!("{report}Missing {sdk} on PATH / JAVA_HOME\n");
    };
    report += &format!("Runtime executable: {}\n", program.display());
    let output = async {
        let mut file = tempfile::tempfile()?;
        let mut command = Command::new(program);
        command
            .arg(if sdk == "java" {
                "-version"
            } else {
                "--list-sdks"
            })
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(file.try_clone()?)
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        if timeout(Duration::from_secs(3), child.wait()).await.is_err() {
            child.kill().await?;
            return Err(io::Error::other("SDK probe timed out"));
        }
        use std::io::{Read, Seek};
        file.rewind()?;
        let mut bytes = Vec::new();
        file.take(8192).read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(String::from_utf8_lossy(&bytes).into_owned())
    }
    .await;
    report += &output.unwrap_or_else(|e| format!("SDK probe failed: {e}"));
    report
}
