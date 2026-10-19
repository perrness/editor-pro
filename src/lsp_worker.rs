//! One concrete language-server manager. Process, framing, discovery and protocol
//! work run on its runtime thread; the editor submits cheap immutable snapshots.
use crate::lsp::*;
use crate::lsp_install::{self, Installer, Launch, State as InstallState};
use crate::lsp_source;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
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
const MAX_SESSIONS: usize = 8;

#[path = "lsp_warmup.rs"]
mod warmup;

fn settings(language: Language) -> Value {
    match language {
        // JDT LS advertises signature help even when its preference is off.
        Language::Java => json!({"java":{"signatureHelp":{"enabled":true}}}),
        Language::CSharp => json!({"csharp":{"useMetadataUris":true}}),
        _ => Value::Null,
    }
}

pub enum Control {
    Install(Language),
    InstallCancel(Language),
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
    preparation: watch::Sender<Option<PathBuf>>,
    commands: mpsc::Sender<Control>,
    results: mpsc::Receiver<Event>,
    stop: watch::Sender<bool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Worker {
    pub fn new(config: Config) -> io::Result<Self> {
        let (documents, docs) = watch::channel(Vec::new());
        let (preparation, projects) = watch::channel(None);
        let (commands, requests) = mpsc::channel(QUEUE);
        let (events, results) = mpsc::channel(128);
        let (stop, shutdown) = watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let thread = thread::Builder::new()
            .name("language-servers".into())
            .spawn(move || {
                runtime.block_on(manager(config, docs, projects, requests, events, shutdown));
            })?;
        Ok(Self {
            documents,
            preparation,
            commands,
            results,
            stop,
            thread: Some(thread),
        })
    }
    pub fn sync(&self, documents: Vec<Snapshot>) {
        self.documents.send_replace(documents);
    }
    /// Replaces any pending discovery; None cancels unused warm sessions.
    pub fn prepare(&self, directory: Option<PathBuf>) -> bool {
        self.preparation.send_if_modified(|current| {
            if *current == directory {
                return false;
            }
            *current = directory;
            true
        })
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

struct Discovery {
    cancelled: Arc<AtomicBool>,
    task: JoinHandle<Vec<ServerKey>>,
}
impl Drop for Discovery {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

async fn manager(
    config: Config,
    mut documents: watch::Receiver<Vec<Snapshot>>,
    mut preparation: watch::Receiver<Option<PathBuf>>,
    mut commands: mpsc::Receiver<Control>,
    events: mpsc::Sender<Event>,
    mut stop: watch::Receiver<bool>,
) {
    let mut routes: BTreeMap<PathBuf, Result<ServerKey, String>> = BTreeMap::new();
    let mut sessions: BTreeMap<ServerKey, Session> = BTreeMap::new();
    let mut installer = Installer::new(config.installation.clone());
    let mut installation_updates = BTreeMap::new();
    let mut discovery: Option<Discovery> = None;
    let mut requested: Option<PathBuf> = None;
    let mut warm = BTreeSet::new();
    let mut warm_installations = BTreeSet::new();
    loop {
        if *stop.borrow() {
            break;
        }
        // Keep at most one filesystem scan alive, even if requests are replaced
        // while a filesystem call is stalled. A cancelled scan cannot publish.
        if discovery.is_none()
            && let Some(directory) = requested.take()
        {
            let cancelled = Arc::new(AtomicBool::new(false));
            let scan_cancel = cancelled.clone();
            let scan_config = config.clone();
            discovery = Some(Discovery {
                cancelled,
                task: tokio::task::spawn_blocking(move || {
                    warmup::discover(&scan_config, &directory, &scan_cancel)
                }),
            });
        }
        let control = tokio::select! {
            biased;
            _ = stop.changed() => break,
            changed = preparation.changed() => {
                if changed.is_err() { break; }
                if let Some(scan) = &discovery { scan.cancelled.store(true, Ordering::Relaxed); }
                warm.clear();
                requested = preparation.borrow_and_update().clone();
                None
            },
            result = async { (&mut discovery.as_mut().unwrap().task).await }, if discovery.is_some() => {
                let scan = discovery.take().unwrap();
                if !scan.cancelled.load(Ordering::Relaxed) {
                    warm = result.unwrap_or_default().into_iter().collect();
                }
                None
            },
            changed = documents.changed() => { if changed.is_err() { break; } None },
            command = commands.recv() => { let Some(command) = command else { break }; Some(command) },
            _ = tokio::time::sleep(Duration::from_millis(100)), if installer.busy() || !installation_updates.is_empty() => None,
        };
        let docs = documents.borrow_and_update().clone();
        if let Some(Control::Install(language)) = &control {
            warm_installations.remove(language);
        }
        if let Some(Control::Install(language)) = &control
            && let Err(error) = installer.start(*language)
        {
            installation_updates.insert(
                *language,
                InstallState {
                    message: error.to_string(),
                    busy: false,
                    failed: true,
                },
            );
        }
        if let Some(Control::InstallCancel(language)) = &control
            && !installer.cancel(*language)
        {
            installation_updates.insert(
                *language,
                InstallState {
                    message: "No installation running".into(),
                    busy: false,
                    failed: false,
                },
            );
        }
        for (language, state) in installer.poll() {
            if !state.busy && !state.failed {
                // Recreate all waiting sessions of this language from the current
                // watch snapshots, not the documents captured before the download.
                let keys: Vec<_> = sessions
                    .keys()
                    .filter(|key| key.language == language)
                    .cloned()
                    .collect();
                for key in keys {
                    if let Some(session) = sessions.remove(&key) {
                        close_session(session).await;
                    }
                }
            }
            installation_updates.insert(language, state);
        }
        // Keep each language's latest update until the UI has room. In particular,
        // a full result queue must never lose completion and leave "installing" stuck.
        installation_updates.retain(|language, state| {
            events
                .try_send(Event::Installation {
                    language: *language,
                    state: state.clone(),
                })
                .is_err()
        });
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
                if let Some(source) = &doc.virtual_source {
                    return Ok(source.key.clone());
                }
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
        // Once a source claims a warm session, its ordinary document lifecycle
        // owns it. Spare warm sessions never consume a slot needed by a source.
        warm.retain(|key| !groups.contains_key(key));
        let available = MAX_SESSIONS.saturating_sub(groups.len());
        warm = warm
            .into_iter()
            .take(available.min(warmup::MAX_WARM_SESSIONS))
            .collect();
        for key in &warm {
            groups.entry(key.clone()).or_default();
        }
        warm_installations.retain(|language| {
            if groups
                .iter()
                .any(|(key, docs)| key.language == *language && !docs.is_empty())
            {
                // Open documents take ownership of this shared installation.
                false
            } else if groups.keys().any(|key| key.language == *language) {
                true
            } else {
                installer.cancel(*language);
                false
            }
        });
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
        // Publish membership before startup events, including documentless
        // sessions. Await capacity so the UI never loses their initial status.
        let keys = groups
            .keys()
            .filter(|key| sessions.contains_key(*key))
            .cloned()
            .chain(
                groups
                    .keys()
                    .filter(|key| !sessions.contains_key(*key))
                    .cloned(),
            )
            .take(MAX_SESSIONS)
            .collect();
        tokio::select! {
            _ = stop.changed() => break,
            result = events.send(Event::Sessions {
                keys,
                warm: warm.iter().cloned().collect(),
                discovering: requested.is_some() || discovery.as_ref().is_some_and(|scan| !scan.cancelled.load(Ordering::Relaxed)),
            }) => if result.is_err() { break; },
        }
        for (key, docs) in groups {
            if *stop.borrow() {
                break;
            }
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
            if sessions.len() >= MAX_SESSIONS {
                for doc in docs {
                    let _ = events.try_send(Event::Unavailable {
                        path: doc.path,
                        message: "LSP limit: eight project/language sessions".into(),
                    });
                }
                continue;
            }
            if matches!(lsp_install::resolve(&config, key.language), Ok(None)) {
                match installer.automatic(key.language) {
                    Ok(started) if started && docs.is_empty() => {
                        warm_installations.insert(key.language);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        installation_updates.insert(
                            key.language,
                            InstallState {
                                message: error.to_string(),
                                busy: false,
                                failed: true,
                            },
                        );
                    }
                }
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
                Control::Restart(_)
                | Control::Doctor(_)
                | Control::Install(_)
                | Control::InstallCancel(_) => None,
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
    drop(discovery);
    for (_, session) in sessions {
        close_session(session).await;
    }
}

async fn close_session(session: Session) {
    session.stop.send_replace(true);
    // Session teardown owns child reaping; no process waits on the input thread.
    let _ = session.task.await;
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

fn command_for(config: &Config, language: Language) -> io::Result<Launch> {
    lsp_install::resolve(config, language)?.ok_or_else(|| {
        io::Error::other(format!(
            "Missing {} language server; :LspInstall {} installs it, :Doctor shows details",
            language.id(),
            language.id()
        ))
    })
}

// Lifecycle states must survive a full UI queue (for example while Help is open).
// Waiting here affects only this session and remains cancellable at shutdown.
async fn status(
    events: &mpsc::Sender<Event>,
    key: &ServerKey,
    epoch: u64,
    state: Status,
    capabilities: Option<Value>,
    stop: &mut watch::Receiver<bool>,
) {
    if *stop.borrow() {
        return;
    }
    let failure = (state.phase == Phase::Failed || state.message.starts_with("LSP stopped:"))
        .then(|| format!("{}: {}", state.phase.label(), state.message));
    tokio::select! {
        _ = stop.changed() => {},
        _ = events.send(Event::Status { key: key.clone(), epoch, state, capabilities }) => {},
    }
    if let Some(message) = failure {
        let _ = events.try_send(Event::Log {
            key: key.clone(),
            message,
        });
    }
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
    let mut epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
    status(
        &events,
        &key,
        epoch,
        Status::default(),
        Some(Value::Null),
        &mut stop,
    )
    .await;
    if *stop.borrow() {
        return;
    }
    let discovery_started = Instant::now();
    let launch = match command_for(&config, key.language) {
        Ok(launch) => launch,
        Err(e) => {
            status(
                &events,
                &key,
                epoch,
                Status::new(
                    Phase::Failed,
                    format!("{e}; :Doctor checks setup, :LspRestart retries"),
                ),
                Some(Value::Null),
                &mut stop,
            )
            .await;
            // Remain addressable until explicit restart, avoiding a spawn loop on every edit.
            if !*stop.borrow() {
                let _ = stop.changed().await;
            }
            return;
        }
    };
    let _ = events.try_send(Event::Log {
        key: key.clone(),
        message: format!(
            "{}\nLaunch argv: {:?}\nExecutable discovery: {:.3}s\n",
            launch.origin,
            launch.argv,
            discovery_started.elapsed().as_secs_f64()
        ),
    });
    status(
        &events,
        &key,
        epoch,
        Status::new(
            Phase::Starting,
            "Checking server runtime; :Doctor shows SDK details",
        ),
        None,
        &mut stop,
    )
    .await;
    if *stop.borrow() {
        return;
    }
    let runtime_started = Instant::now();
    let runtime = tokio::select! {
        _ = stop.changed() => return,
        result = launch.runtime_report() => result,
    };
    if let Err(error) = runtime {
        status(
            &events,
            &key,
            epoch,
            Status::new(
                Phase::Failed,
                format!("{error}; :Doctor checks setup, :LspRestart retries"),
            ),
            Some(Value::Null),
            &mut stop,
        )
        .await;
        if !*stop.borrow() {
            let _ = stop.changed().await;
        }
        return;
    }
    let _ = events.try_send(Event::Log {
        key: key.clone(),
        message: format!(
            "Runtime probe: {:.3}s\n",
            runtime_started.elapsed().as_secs_f64()
        ),
    });
    for attempt in 0..=config.restart_limit {
        if *stop.borrow() {
            return;
        }
        if attempt > 0 {
            epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
        }
        status(
            &events,
            &key,
            epoch,
            Status::new(
                Phase::Starting,
                format!(
                    "Starting {} (attempt {}/{})",
                    key.language.id(),
                    attempt + 1,
                    config.restart_limit.saturating_add(1)
                ),
            ),
            Some(Value::Null),
            &mut stop,
        )
        .await;
        if *stop.borrow() {
            return;
        }
        let launch_started = Instant::now();
        let mut transport = match Transport::start(&launch, &key, &events) {
            Ok(transport) => transport,
            Err(e) => {
                status(&events, &key, epoch,
                    Status::new(Phase::Failed, format!("Launch failed: {e}; :Doctor checks executable/SDK, :LspLog shows errors; :LspRestart retries")),
                    Some(Value::Null), &mut stop).await;
                if !*stop.borrow() {
                    let _ = stop.changed().await;
                }
                return;
            }
        };
        let _ = events.try_send(Event::Log {
            key: key.clone(),
            message: format!(
                "Process spawn: {:.3}s\n",
                launch_started.elapsed().as_secs_f64()
            ),
        });
        let result = tokio::select! {
            _ = stop.changed() => Ok(()),
            result = run(&mut transport, &key, epoch, &config, &mut documents, &mut commands, &events) => result,
        };
        let restarting = attempt < config.restart_limit;
        if !*stop.borrow() {
            // Invalidate capabilities/diagnostics before the bounded process teardown.
            status(&events, &key, epoch,
                Status::new(if restarting { Phase::Starting } else { Phase::Failed }, format!(
                    "LSP stopped: {}; {}. :LspLog shows server errors; :Doctor checks SDK/project setup",
                    result.err().map_or_else(|| "closed".into(), |e| e.to_string()),
                    if restarting { format!("restarting (retry {}/{})", attempt + 1, config.restart_limit) }
                    else { "automatic retries exhausted; :LspRestart retries".into() }
                )), Some(Value::Null), &mut stop).await;
        }
        transport.close().await;
        if *stop.borrow() {
            return;
        }
        if !restarting {
            let _ = stop.changed().await;
            return;
        }
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))) => {}
        }
    }
}

/// Protocol activity belongs to the existing session, never to a second supervisor.
/// Work tokens are independent: finishing one cannot finish another or initialization.
#[derive(Default)]
struct Activity {
    initialized: bool,
    java_starting: bool,
    tasks: BTreeMap<String, ProgressTask>,
    problem: Option<String>,
    state: Status,
}
struct ProgressTask {
    title: String,
    message: String,
    percentage: Option<u64>,
}
fn bounded_message(value: &Value) -> String {
    value.as_str().unwrap_or("").chars().take(512).collect()
}
impl Activity {
    fn observe(&mut self, language: Language, frame: &Value) -> bool {
        let params = &frame["params"];
        match frame["method"].as_str().unwrap_or("") {
            "$/progress" => {
                let token = &params["token"];
                if !(token.is_string() || token.as_i64().is_some()) {
                    return false;
                }
                if token.as_str().is_some_and(|token| token.len() > 1024) {
                    self.problem =
                        Some("Server progress token exceeds 1 KiB; progress is incomplete".into());
                    return true;
                }
                let token = token.to_string();
                let value = &params["value"];
                match value["kind"].as_str().unwrap_or("") {
                    "begin" => {
                        if self.tasks.len() >= QUEUE && !self.tasks.contains_key(&token) {
                            self.problem = Some("Server exceeded 32 concurrent progress tasks; progress is incomplete".into());
                        } else {
                            self.tasks.insert(
                                token,
                                ProgressTask {
                                    title: bounded_message(&value["title"]),
                                    message: bounded_message(&value["message"]),
                                    percentage: value["percentage"].as_u64().filter(|p| *p <= 100),
                                },
                            );
                        }
                    }
                    "report" => {
                        let Some(task) = self.tasks.get_mut(&token) else {
                            return false;
                        };
                        if value["message"].is_string() {
                            task.message = bounded_message(&value["message"]);
                        }
                        if let Some(p) = value["percentage"].as_u64().filter(|p| *p <= 100) {
                            task.percentage = Some(p);
                        }
                    }
                    "end" => {
                        if self.tasks.remove(&token).is_none() {
                            return false;
                        }
                    }
                    _ => return false, // Partial-result progress is not work-done progress.
                }
            }
            "window/logMessage"
            | "window/showMessage"
            | "window/showMessageRequest"
            | "language/actionableNotification"
                if matches!(
                    params["type"]
                        .as_u64()
                        .or_else(|| params["severity"].as_u64()),
                    Some(1 | 2)
                ) =>
            {
                self.problem = Some(bounded_message(&params["message"]));
            }
            "language/status" if language == Language::Java => {
                match params["type"].as_str().unwrap_or("") {
                    "Starting" => self.java_starting = true,
                    "Started" | "ServiceReady" => self.java_starting = false,
                    "Error" => self.problem = Some(bounded_message(&params["message"])),
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }
    async fn publish(
        &mut self,
        key: &ServerKey,
        epoch: u64,
        events: &mpsc::Sender<Event>,
        capabilities: Option<Value>,
    ) -> io::Result<()> {
        let phase = if self.problem.is_some() {
            Phase::Degraded
        } else if !self.tasks.is_empty() || self.java_starting {
            Phase::Indexing
        } else if self.initialized {
            Phase::Ready
        } else {
            Phase::Starting
        };
        if self.state.phase != phase {
            self.state.since = std::time::Instant::now();
            self.state.phase = phase;
        }
        self.state.progress = self
            .tasks
            .values()
            .map(|task| {
                format!(
                    "{}: {}{}",
                    task.title,
                    task.message,
                    task.percentage
                        .map_or(String::new(), |p| format!(" ({p}%, server reported)"))
                )
            })
            .collect();
        self.state.message = if let Some(problem) = &self.problem {
            format!(
                "Server reported: {problem}\nCheck :LspLog for Gradle/Maven import errors and :Doctor for SDK/root setup. Fix the reported cause, then :LspRestart. Warning retained until restart."
            )
        } else if phase == Phase::Indexing {
            format!(
                "Importing/indexing: {}",
                self.state
                    .progress
                    .first()
                    .map_or("Java project startup", String::as_str)
            )
        } else if self.initialized {
            "Protocol ready; no reported work remains. Unreported indexing may continue.".into()
        } else {
            "Waiting for initialize response; :LspLog shows server output".into()
        };
        // The run's outer stop select cancels this wait; do not lose the final state
        // to try_send when progress/log notifications fill the UI queue.
        events
            .send(Event::Status {
                key: key.clone(),
                epoch,
                state: self.state.clone(),
                capabilities,
            })
            .await
            .map_err(|_| io::Error::other("LSP result receiver closed"))
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
    fn start(launch: &Launch, key: &ServerKey, events: &mpsc::Sender<Event>) -> io::Result<Self> {
        let argv = &launch.argv;
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
            command.arg("-data").arg(dir.path().join("workspace"));
            if launch.java_configuration.is_some() {
                command
                    .arg("-configuration")
                    .arg(dir.path().join("configuration"));
            }
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
    source_fingerprint: Option<lsp_source::Fingerprint>,
    source_revision: u64,
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
    let mut activity = Activity::default();
    let initialize_started = Instant::now();
    wire.request(1, "initialize", json!({
        "processId":std::process::id(),"clientInfo":{"name":"editor-pro","version":env!("CARGO_PKG_VERSION")},
        "rootUri":root,"workspaceFolders":[{"uri":root,"name":key.root.file_name().unwrap_or_default().to_string_lossy()}],
        "initializationOptions":{
            "settings":settings(key.language),
            "extendedClientCapabilities": if key.language == Language::Java {
                json!({"classFileContentsSupport":true})
            } else { json!({}) }
        },
        "capabilities":{
            "experimental": if key.language == Language::CSharp {
                json!({"csharp":{"metadataUris":true}})
            } else { json!({}) },
            "general":{"positionEncodings":["utf-16"]},
            "window":{"workDoneProgress":true},
            "workspace":{"applyEdit":true,"workspaceFolders":true,"configuration":true,
                "diagnostics":{"refreshSupport":true},
                "symbol":{"dynamicRegistration":false,"resolveSupport":{"properties":["location.range"]}},
                "workspaceEdit":{"documentChanges":true,"failureHandling":"abort","resourceOperations":[]}},
            "textDocument":{
                "synchronization":{"dynamicRegistration":false,"didSave":true},
                "completion":{"contextSupport":true,"completionItem":{"snippetSupport":false,"documentationFormat":["plaintext","markdown"],
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
        .map_err(|_| io::Error::other("LSP initialize timed out (project import may be slow)"))?
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
        if activity.observe(key.language, &frame) {
            activity.publish(key, epoch, events, None).await?;
        }
        server_message(wire, key, epoch, &frame, &BTreeMap::new(), None, events).await?;
    };
    wire.notify("initialized", json!({})).await?;
    let _ = events.try_send(Event::Log {
        key: key.clone(),
        message: format!(
            "Initialize response: {:.3}s (import/indexing may continue)\n",
            initialize_started.elapsed().as_secs_f64()
        ),
    });
    activity.initialized = true;
    activity
        .publish(key, epoch, events, Some(capabilities.clone()))
        .await?;
    let mut open = BTreeMap::new();
    let mut version = 0;
    let mut next_id = 2;
    let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
    let mut pulled: BTreeMap<PathBuf, i32> = BTreeMap::new();
    let mut action: Option<Pending> = None;
    let mut awaiting_edit: Option<u64> = None;
    let mut timer = tokio::time::interval(Duration::from_millis(50));
    let mut first_sync = true;
    let mut source_cache = lsp_source::Cache::default();
    let mut source_revision = 0;
    let mut deferred_control = None;
    loop {
        let snapshots = source.borrow_and_update().clone();
        let sync_started = Instant::now();
        if synchronize(
            wire,
            &capabilities,
            &mut open,
            snapshots,
            &mut version,
            epoch,
        )
        .await?
            && capabilities["diagnosticProvider"]["interFileDependencies"] == true
        {
            pulled.clear();
        }
        if first_sync && !open.is_empty() {
            first_sync = false;
            let _ = events.try_send(Event::Log {
                key: key.clone(),
                message: format!(
                    "Initial document synchronization: {:.3}s\n",
                    sync_started.elapsed().as_secs_f64()
                ),
            });
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
                .filter(|d| d.snapshot.virtual_source.is_none())
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
                        source_fingerprint: None,
                        source_revision,
                    },
                );
                next_id += 1;
            }
        }
        tokio::select! {
            changed = source.changed() => { changed.map_err(io::Error::other)?; }
            command = async { if deferred_control.is_some() { deferred_control.take() } else { commands.recv().await } } => {
                match command.ok_or_else(|| io::Error::other("LSP command channel closed"))? {
                    Control::Request(mut request) => {
                        awaiting_edit = None;
                        // The watch channel may have changed since the top of the loop.
                        let snapshots = source.borrow_and_update().clone();
                        if synchronize(wire, &capabilities, &mut open, snapshots, &mut version, epoch).await?
                            && capabilities["diagnosticProvider"]["interFileDependencies"] == true {
                            pulled.clear();
                        }
                        let error = if !supports_request(&capabilities, key.language, &request.method, &request.params) {
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
                        if request.params.get("position").is_some()
                            && let Some(document) = open.get(&request.path)
                            && let Some(original) = document.snapshot.virtual_source.as_ref().and_then(|s| s.protocol_text.as_ref()) {
                            let mapped = serde_json::from_value(request.params["position"].clone()).map_err(io::Error::other)
                                .and_then(|position| lsp_source::map_position(&document.snapshot.text, original, position));
                            match mapped {
                                Ok(position) => request.params["position"] = json!(position),
                                Err(error) => {
                                    emit(events, Event::Response { key:key.clone(), epoch, request, documents:open.values().cloned().collect(), result:Err(error.to_string()) }).await?;
                                    continue;
                                }
                            }
                        }
                        let previous: Vec<_> = pending.iter().filter(|(_, p)| p.request.serial != 0).map(|(id,_)| *id).collect();
                        for id in previous { wire.notify("$/cancelRequest",json!({"id":id})).await?; pending.remove(&id); }
                        let documents: Vec<_> = open.values().cloned().collect();
                        let source_uri = lsp_source::request_uri(key.language, &request);
                        let source_fingerprint = if let Some(uri) = source_uri {
                            // Retrieval commands cannot grant edit authority, even
                            // if a preceding code action still has time remaining.
                            action = None;
                            let fingerprint = lsp_source::fingerprint(uri).await;
                            if let Some(result) = source_cache.get(uri, fingerprint.as_ref()) {
                                emit(events, Event::Response { key:key.clone(), epoch, request, documents, result:Ok(result) }).await?;
                                continue;
                            }
                            fingerprint
                        } else { None };
                        if request.method == "workspace/executeCommand" && source_uri.is_none() {
                            action = Some(Pending { request:request.clone(), documents:documents.clone(), deadline:Instant::now()+config.request_timeout, source_fingerprint:None, source_revision });
                        }
                        wire.request(next_id, &request.method, request.params.clone()).await?;
                        pending.insert(next_id, Pending { request, documents, deadline:Instant::now()+config.request_timeout, source_fingerprint, source_revision });
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
                    // Opening a dependency can start ordinary indexing. Evict
                    // for project imports/builds, not every indexing percentage.
                    let import_progress = frame["method"] == "$/progress"
                        && matches!(frame["params"]["value"]["kind"].as_str(), Some("begin" | "end"))
                        && frame["params"]["value"]["title"].as_str()
                            .or_else(|| activity.tasks.get(&frame["params"]["token"].to_string()).map(|task| task.title.as_str()))
                            .is_some_and(|title| {
                                let title = title.to_lowercase();
                                title.contains("import") || title.contains("build")
                            });
                    let activity_changed = activity.observe(key.language, &frame);
                    if frame["method"] == "workspace/textDocumentContent/refresh"
                        || (activity_changed && (import_progress || frame["method"] == "language/status")) {
                        source_cache.clear();
                        source_revision += 1;
                    }
                    if activity_changed {
                        activity.publish(key, epoch, events, None).await?;
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
                    let mut result = frame.get("error").map_or_else(|| Ok(frame["result"].clone()), |e| Err(format!("LSP error: {e}")));
                    if p.request.method == "textDocument/diagnostic" {
                        if let (Ok(value), Some(document)) = (result, open.get(&p.request.path))
                            && value["kind"] == "full" {
                            let items = diagnostics(&value["items"])?;
                            let dependencies = (capabilities["diagnosticProvider"]["interFileDependencies"] == true)
                                .then(|| p.documents.iter().filter(|d| d.snapshot.virtual_source.is_none())
                                    .map(|d| (d.snapshot.path.clone(), d.snapshot.stamp.clone())).collect());
                            emit(events, Event::Diagnostics { key:key.clone(),epoch,document:document.clone(),dependencies,versioned:true,items }).await?;
                        }
                    } else {
                        if key.language == Language::Kotlin
                            && let (Ok(value), Some(uri)) = (&result, lsp_source::request_uri(key.language, &p.request)) {
                            let remaining = p.deadline.saturating_duration_since(Instant::now());
                            result = tokio::select! {
                                prepared = timeout(remaining, lsp_source::prepare(config, uri, value.clone(), remaining)) => {
                                    prepared.map_err(|_| "Bytecode retrieval timed out".to_owned()).and_then(|r| r.map_err(|e| e.to_string()))
                                }
                                command = commands.recv() => {
                                    deferred_control = command;
                                    Err("Bytecode retrieval cancelled".into())
                                }
                                _ = source.changed() => Err("Bytecode retrieval became stale".into()),
                            };
                        }
                        if let (Ok(value), Some(uri)) = (&result, lsp_source::request_uri(key.language, &p.request)) {
                            let current = lsp_source::fingerprint(uri).await;
                            if p.source_fingerprint.as_ref().is_some_and(|before| !before.same_binary(current.as_ref())) {
                                result = Err("Dependency changed while loading source; retry navigation".into());
                            } else if p.source_revision == source_revision && current == p.source_fingerprint {
                                // A server may download sources during retrieval.
                                // Show that result, but cache only stable observations.
                                source_cache.insert(key.language, uri, value, current);
                            }
                        }
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
    epoch: u64,
) -> io::Result<bool> {
    let snapshots: Vec<_> = snapshots
        .into_iter()
        .filter(|s| s.virtual_source.as_ref().is_none_or(|v| v.epoch == epoch))
        .collect();
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
        if open_close
            && open[&path]
                .snapshot
                .virtual_source
                .as_ref()
                .is_none_or(|s| s.key.language == Language::Kotlin)
        {
            wire.notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":open[&path].snapshot.uri()?}}),
            )
            .await?;
        }
        open.remove(&path);
    }
    for snapshot in snapshots {
        // JDT/C# own their virtual text without synchronization. The official
        // Kotlin client opens jar/jrt documents so feature providers can find
        // them. Send its exact declaration text and language, never the fuller
        // bytecode layout or didChange/didSave from these read-only buffers.
        if let Some(source) = &snapshot.virtual_source {
            if source.key.language == Language::Kotlin
                && open_close
                && open
                    .get(&snapshot.path)
                    .is_none_or(|previous| previous.snapshot.stamp != snapshot.stamp)
            {
                if let Some(previous) = open.get(&snapshot.path) {
                    wire.notify(
                        "textDocument/didClose",
                        json!({"textDocument":{"uri":previous.snapshot.uri()?}}),
                    )
                    .await?;
                }
                wire.notify("textDocument/didOpen", json!({"textDocument":{
                    "uri":snapshot.uri()?, "languageId":source.protocol_language.id(), "version":0,
                    "text":source.protocol_text.as_ref().unwrap_or(&snapshot.text).to_string()
                }})).await?;
            }
            open.insert(
                snapshot.path.clone(),
                Versioned {
                    snapshot,
                    version: 0,
                },
            );
            continue;
        }
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
    if method == "$/progress" && matches!(params["value"]["kind"].as_str(), Some("begin" | "end")) {
        let _ = events.try_send(Event::Log {
            key: key.clone(),
            message: format!(
                "Progress {}: {} {}",
                params["value"]["kind"].as_str().unwrap_or(""),
                bounded_message(&params["value"]["title"]),
                bounded_message(&params["value"]["message"])
            ),
        });
    }
    if matches!(
        method,
        "window/logMessage"
            | "window/showMessage"
            | "window/showMessageRequest"
            | "language/status"
            | "language/actionableNotification"
    ) {
        let _ = events.try_send(Event::Log {
            key: key.clone(),
            message: format!(
                "{method}: {}",
                params["message"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(4096)
                    .collect::<String>()
            ),
        });
    }
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
    if method == "textDocument/publishDiagnostics"
        && let Some(uri) = params["uri"].as_str()
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
        "Root discovery: {:?}\nManaged directory: {:?}\nAutomatic installation: {}\n",
        project_root(config, path, language),
        config.installation.directory,
        config.installation.auto_install,
    );
    match command_for(config, language) {
        Ok(launch) => {
            report += &format!(
                "Server source: {}\nLaunch argv: {:?}\n",
                launch.origin, launch.argv
            );
            report += &launch
                .runtime_report()
                .await
                .unwrap_or_else(|e| e.to_string());
        }
        Err(error) => {
            report += &format!(
                "{error}\n{}",
                lsp_install::prerequisite_report(language).await
            );
        }
    }
    report
}
