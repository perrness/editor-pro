//! One concrete Codex CLI/I/O worker. No process or transcript I/O runs in tick
//! or render. The latest progress slot coalesces output; cancellation is serial.
use crate::{
    ai::{self, Answer, Config, Conversation, Draft, error},
    ai_store,
};
use serde_json::Value;
use std::{
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
const ERROR_LIMIT: u64 = 256 * 1024;
const EVENT_LIMIT: usize = 1024 * 1024;

pub enum Job {
    Ask {
        config: Config,
        prompt: String,
        draft: Draft,
    },
    Save {
        conversation: Conversation,
        path: PathBuf,
        expected: Option<String>,
        protected: Vec<PathBuf>,
    },
    Load(PathBuf),
}
pub enum Outcome {
    Answer(Answer),
    Saved(PathBuf, String),
    Loaded(Conversation, PathBuf, String),
}
pub struct Progress {
    pub serial: u64,
    pub message: String,
    pub answer: Option<Answer>,
}
pub struct Reply {
    pub serial: u64,
    pub result: io::Result<Outcome>,
}
#[derive(Default)]
struct Queue {
    pending: Option<(u64, Job)>,
    result: Option<Reply>,
    progress: Option<Progress>,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    serial: AtomicU64,
}
pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn new() -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        let thread = thread::Builder::new()
            .name("codex-cli".into())
            .spawn(move || {
                loop {
                    let mut queue = state.queue.lock().unwrap();
                    while queue.pending.is_none() && !queue.closed {
                        queue = state.wake.wait(queue).unwrap();
                    }
                    // Accepted persistence work drains even during shutdown.
                    let Some((serial, job)) = queue.pending.take() else {
                        break;
                    };
                    drop(queue);
                    let result = match job {
                        Job::Ask {
                            config,
                            prompt,
                            draft,
                        } => run(&config, &prompt, &draft, &state, serial).map(Outcome::Answer),
                        Job::Save {
                            conversation,
                            path,
                            expected,
                            protected,
                        } => ai_store::save(&conversation, &path, expected.as_deref(), &protected)
                            .map(|(p, hash)| Outcome::Saved(p, hash)),
                        Job::Load(path) => {
                            ai_store::load(&path).map(|(c, hash)| Outcome::Loaded(c, path, hash))
                        }
                    };
                    let mut queue = state.queue.lock().unwrap();
                    if !queue.closed && state.serial.load(Ordering::Acquire) == serial {
                        queue.result = Some(Reply { serial, result });
                    }
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }
    pub fn submit(&self, job: Job) -> u64 {
        let mut q = self.shared.queue.lock().unwrap();
        let serial = self.shared.serial.fetch_add(1, Ordering::AcqRel) + 1;
        q.pending = Some((serial, job));
        q.result = None;
        q.progress = None;
        self.shared.wake.notify_one();
        serial
    }
    pub fn cancel(&self) {
        let mut q = self.shared.queue.lock().unwrap();
        self.shared.serial.fetch_add(1, Ordering::AcqRel);
        // Callers only cancel inference. Never abandon an accepted save.
        if matches!(q.pending, Some((_, Job::Ask { .. }))) {
            q.pending = None;
        }
        q.result = None;
        q.progress = None;
    }
    pub fn poll(&self) -> (Option<Progress>, Option<Reply>) {
        let mut q = self.shared.queue.lock().unwrap();
        (q.progress.take(), q.result.take())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.shared.queue.lock().unwrap().closed = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn progress(state: &Shared, serial: u64, message: &str, answer: Option<Answer>) {
    let mut q = state.queue.lock().unwrap();
    if state.serial.load(Ordering::Acquire) == serial && !q.closed {
        q.progress = Some(Progress {
            serial,
            message: message.into(),
            answer,
        });
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", self.0.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run(
    config: &Config,
    prompt: &str,
    draft: &Draft,
    state: &Shared,
    serial: u64,
) -> io::Result<Answer> {
    let deadline = Instant::now() + config.timeout;
    let check = || -> io::Result<()> {
        if state.serial.load(Ordering::Acquire) != serial {
            return Err(error("Codex request cancelled"));
        }
        if Instant::now() >= deadline {
            return Err(error("Codex request timed out; conversation retained"));
        }
        Ok(())
    };
    check()?;
    if prompt.len() > ai::MAX_PROMPT {
        return Err(error("AI prompt exceeds 1 MiB"));
    }
    // A private working directory avoids project configuration/instructions and
    // gives the provider no reason to inspect or write the user's working tree.
    let dir = tempfile::tempdir()?;
    let mut input = tempfile::NamedTempFile::new_in(dir.path())?;
    input.write_all(prompt.as_bytes())?;
    let mut schema = tempfile::NamedTempFile::new_in(dir.path())?;
    serde_json::to_writer(&mut schema, &ai::response_schema()).map_err(io::Error::other)?;
    let output = tempfile::NamedTempFile::new_in(dir.path())?;
    let errors = tempfile::NamedTempFile::new_in(dir.path())?;
    let mut reader = File::open(output.path())?;
    let executable = if config.executable.components().count() > 1 {
        config.executable.canonicalize()?
    } else {
        config.executable.clone()
    };
    let mut command = Command::new(executable);
    command
        .current_dir(dir.path())
        .args([
            "exec",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--json",
            "--color",
            "never",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "web_search=\"disabled\"",
            "-c",
            "project_doc_max_bytes=0",
            "-c",
            "mcp_servers={}",
            "--disable",
            "shell_tool",
            "--disable",
            "hooks",
            "--disable",
            "apps",
            "--disable",
            "multi_agent",
            "--disable",
            "remote_plugin",
            "--disable",
            "memories",
        ])
        .arg("--output-schema")
        .arg(schema.path());
    if let Some(model) = &config.model {
        command.arg("--model").arg(model);
    }
    command
        .arg("-")
        .stdin(File::open(input.path())?)
        .stdout(output.as_file().try_clone()?)
        .stderr(errors.as_file().try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut process = Process(command.spawn().map_err(|e| {
        error(&format!(
            "Cannot run Codex CLI: {e}; install codex and run codex login outside the editor"
        ))
    })?);
    let mut events = Events::default();
    let mut pending = Vec::new();
    let mut total = 0_u64;
    let status = loop {
        check()?;
        if output.as_file().metadata()?.len() > OUTPUT_LIMIT
            || errors.as_file().metadata()?.len() > ERROR_LIMIT
        {
            return Err(error("Codex output exceeded its capture limit"));
        }
        let mut chunk = [0_u8; 16384];
        let count = reader.read(&mut chunk)?;
        total += count as u64;
        if total > OUTPUT_LIMIT {
            return Err(error("Codex output exceeds 4 MiB"));
        }
        pending.extend_from_slice(&chunk[..count]);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            if end > EVENT_LIMIT {
                return Err(error("Codex event exceeds 1 MiB"));
            }
            events.accept(&pending[..end], draft, state, serial)?;
            pending.drain(..=end);
        }
        if pending.len() > EVENT_LIMIT {
            return Err(error("Codex event exceeds 1 MiB"));
        }
        if count == 0 {
            if let Some(status) = process.0.try_wait()? {
                // Read once more after observing exit: the last write can race
                // the preceding EOF on a growing regular file.
                let mut tail = Vec::new();
                reader.take(OUTPUT_LIMIT + 1).read_to_end(&mut tail)?;
                if total + tail.len() as u64 > OUTPUT_LIMIT {
                    return Err(error("Codex output exceeds 4 MiB"));
                }
                pending.extend(tail);
                for line in pending.split(|b| *b == b'\n').filter(|s| !s.is_empty()) {
                    events.accept(line, draft, state, serial)?;
                }
                break status;
            }
            thread::sleep(Duration::from_millis(10));
        }
    };
    drop(process);
    check()?;
    if !status.success() {
        let mut detail = String::new();
        File::open(errors.path())?
            .take(4096)
            .read_to_string(&mut detail)?;
        return Err(error(&format!("Codex exited {status}: {detail}")));
    }
    if !events.completed {
        return Err(error("Codex exited without a completed turn"));
    }
    events.answer.ok_or_else(|| {
        error(
            events
                .answer_error
                .as_deref()
                .unwrap_or("Codex completed without a completed structured answer item"),
        )
    })
}

#[derive(Default)]
struct Events {
    completed: bool,
    answer: Option<Answer>,
    answer_error: Option<String>,
}
impl Events {
    fn accept(
        &mut self,
        line: &[u8],
        draft: &Draft,
        state: &Shared,
        serial: u64,
    ) -> io::Result<()> {
        if line.len() > EVENT_LIMIT {
            return Err(error("Codex event exceeds 1 MiB"));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let event: Value = serde_json::from_slice(line)
            .map_err(|e| error(&format!("Malformed Codex JSON event: {e}")))?;
        match event["type"]
            .as_str()
            .ok_or_else(|| error("Codex event has no type"))?
        {
            "thread.started" => progress(state, serial, "Codex connected", None),
            "turn.started" => progress(
                state,
                serial,
                "Codex is considering the supplied context",
                None,
            ),
            "turn.completed" => self.completed = true,
            "turn.failed" | "error" => {
                let detail = event["error"]["message"]
                    .as_str()
                    .or_else(|| event["message"].as_str())
                    .unwrap_or("Unknown provider error");
                return Err(error(&format!(
                    "Codex: {}",
                    detail.chars().take(4096).collect::<String>()
                )));
            }
            "item.started" | "item.updated" | "item.completed" => {
                match event["item"]["type"].as_str() {
                    Some("agent_message") => {
                        let text = event["item"]["text"]
                            .as_str()
                            .ok_or_else(|| error("Codex message has no text"))?;
                        if text.len() > ai::MAX_RESPONSE {
                            return Err(error("Codex answer exceeds 128 KiB"));
                        }
                        // JSONL provides item updates, not a guaranteed token stream.
                        // Display complete structured updates; only a successful
                        // turn/exit can make one actionable.
                        self.answer = None;
                        match Answer::parse(text, draft) {
                            Ok(answer) => {
                                progress(
                                    state,
                                    serial,
                                    "Receiving Codex answer (awaiting completion)",
                                    Some(answer.clone()),
                                );
                                if event["type"] == "item.completed" {
                                    self.answer = Some(answer);
                                    self.answer_error = None;
                                }
                            }
                            Err(e) if event["type"] == "item.completed" => {
                                self.answer_error = Some(format!("Invalid Codex answer: {e}"));
                                progress(
                                    state,
                                    serial,
                                    "Codex message received; waiting for structured answer",
                                    None,
                                );
                            }
                            Err(_) => {}
                        }
                    }
                    Some("reasoning" | "plan" | "todo_list") => {}
                    Some(_) => {
                        return Err(error(
                            "Codex attempted tool use in a context-only request; request stopped",
                        ));
                    }
                    None => return Err(error("Codex item has no type")),
                }
            }
            _ => {} // Additive lifecycle events are not authority to act.
        }
        Ok(())
    }
}
