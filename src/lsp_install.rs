//! Pinned, per-user server installations. All network, archive and SDK work runs
//! outside the terminal and protocol loops. No project controls download recipes.
use crate::lsp::{Config, Language};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::watch;

mod archive;
mod http;
#[cfg(test)]
mod tests;

const DEADLINE: Duration = Duration::from_secs(600);
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;
const KOTLIN_VERSION: &str = "263.6379.0";
const CSHARP_VERSION: &str = "0.28.0";

#[derive(Clone, Default)]
pub struct Options {
    pub directory: Option<PathBuf>,
    pub auto_install: bool,
    #[cfg(test)]
    pub(crate) recipe: Option<Recipe>,
    #[cfg(test)]
    pub(crate) ignore_path: bool,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub message: String,
    pub busy: bool,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug)]
enum Format {
    Tar,
    Zip,
    Nuget,
}

#[derive(Clone, Debug)]
pub(crate) struct Recipe {
    version: String,
    url: String,
    sha256: String,
    format: Format,
}

fn recipe(language: Language, options: &Options) -> io::Result<Recipe> {
    #[cfg(test)]
    if let Some(recipe) = &options.recipe {
        return Ok(recipe.clone());
    }
    let _ = options;
    recipe_for(language, std::env::consts::OS, std::env::consts::ARCH)
}

fn recipe_for(language: Language, os: &str, arch: &str) -> io::Result<Recipe> {
    if !matches!((os, arch), ("linux" | "macos", "x86_64" | "aarch64")) || cfg!(target_env = "musl")
    {
        return Err(io::Error::other(
            "Managed LSP installations support macOS and glibc Linux on x86_64/ARM64; configure an external server on this platform",
        ));
    }
    let (version, url, sha256, format) = match language {
        Language::Kotlin => {
            let (suffix, hash, format) = match (os, arch) {
                ("linux", "x86_64") => (".tar.gz", "ab8ca4455dc2fc5fe1a24db2bccc46c104254d2c465155c4251ee65df8f3f7cc", Format::Tar),
                ("linux", _) => ("-aarch64.tar.gz", "50999901ef8bcfa1e58561b6a8d782a72dea5620fcf92a64130807f8924a56fc", Format::Tar),
                ("macos", "x86_64") => (".sit", "e69e0c9d27b915b2db9ee692ec08d097df1a394d9f21190457ef199f2905f77e", Format::Zip),
                _ => ("-aarch64.sit", "ebef2e13cd4adc4ec9e04084b848000a3ec7a9d2917c64f269574ce2efe9ecad", Format::Zip),
            };
            (KOTLIN_VERSION, format!("https://download.jetbrains.com/language-server/kotlin-server/{KOTLIN_VERSION}/kotlin-server-{KOTLIN_VERSION}{suffix}"), hash, format)
        }
        Language::Java => ("1.60.0-202606262232", "https://download.eclipse.org/jdtls/milestones/1.60.0/jdt-language-server-1.60.0-202606262232.tar.gz".into(), "e94c303d8198f977930803582738771fd18c52c5492878410bf222b1aa81ef1d", Format::Tar),
        Language::CSharp => (CSHARP_VERSION, format!("https://api.nuget.org/v3-flatcontainer/csharp-ls/{CSHARP_VERSION}/csharp-ls.{CSHARP_VERSION}.nupkg"), "8f815e4f22674106a8244beba07e4b4b062e31528e404aa8489312effb8c0e0c", Format::Nuget),
    };
    Ok(Recipe {
        version: version.into(),
        url,
        sha256: sha256.into(),
        format,
    })
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: String,
    sha256: String,
}
impl Recipe {
    fn receipt(&self) -> Receipt {
        Receipt {
            version: self.version.clone(),
            sha256: self.sha256.clone(),
        }
    }
    fn destination(&self, root: &Path, language: Language) -> PathBuf {
        root.join(language.id()).join(format!(
            "{}-{}-{}",
            self.version,
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    }
}

pub(crate) fn executable(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value)
            .map(|directory| directory.join(name))
            .find(|path| is_executable(path))
            .and_then(|path| path.canonicalize().ok())
    })
}
fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}
fn java() -> Option<PathBuf> {
    std::env::var_os("JAVA_HOME")
        .map(|home| Path::new(&home).join("bin/java"))
        .filter(|path| is_executable(path))
        .and_then(|path| path.canonicalize().ok())
        .or_else(|| executable("java"))
}

#[derive(Clone, Debug)]
pub(crate) struct Runtime {
    program: PathBuf,
    minimum: u32,
    dotnet: bool,
}
impl Runtime {
    fn external(language: Language) -> io::Result<Option<Self>> {
        let (program, minimum, dotnet) = match language {
            Language::Kotlin => return Ok(None),
            Language::Java => (java(), 21, false),
            Language::CSharp => (executable("dotnet"), 10, true),
        };
        let program = program.ok_or_else(|| {
            io::Error::other(if dotnet {
                "Install .NET SDK 10+ and make dotnet available on PATH, then retry :LspInstall"
            } else {
                "Install JDK 21+ and set JAVA_HOME or PATH, then retry :LspInstall"
            })
        })?;
        Ok(Some(Self {
            program,
            minimum,
            dotnet,
        }))
    }
    fn check(&self, guard: &Guard) -> io::Result<String> {
        let output = capture(
            &self.program,
            &[if self.dotnet {
                "--list-sdks".into()
            } else {
                "-version".into()
            }],
            &[],
            None,
            guard,
            Duration::from_secs(3),
        )?;
        let compatible = if self.dotnet {
            output.lines().any(|line| {
                line.split('.')
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some_and(|v| v >= self.minimum)
            })
        } else {
            output
                .lines()
                .filter_map(|line| line.split_once("version \"").map(|(_, version)| version))
                .filter_map(|version| {
                    version
                        .split(|ch: char| !ch.is_ascii_digit())
                        .next()?
                        .parse::<u32>()
                        .ok()
                })
                .any(|version| version >= self.minimum)
        };
        if !compatible {
            return Err(io::Error::other(format!(
                "{} requires {} {}+; selected {} reported: {}. Set {} and retry :LspInstall / :LspRestart",
                if self.dotnet {
                    "csharp-ls"
                } else {
                    "Language server"
                },
                if self.dotnet { ".NET SDK" } else { "Java" },
                self.minimum,
                self.program.display(),
                output.trim(),
                if self.dotnet {
                    "PATH"
                } else {
                    "JAVA_HOME or PATH"
                }
            )));
        }
        Ok(format!(
            "Runtime: {}\n{}",
            self.program.display(),
            output.trim()
        ))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Launch {
    pub argv: Vec<OsString>,
    pub origin: String,
    pub java_configuration: Option<PathBuf>,
    runtime: Option<Runtime>,
}
impl Launch {
    pub async fn runtime_report(&self) -> io::Result<String> {
        let Some(runtime) = self.runtime.clone() else {
            return Ok("Runtime supplied by explicit command override".into());
        };
        tokio::task::spawn_blocking(move || {
            runtime.check(&Guard::new(Arc::new(AtomicBool::new(false))))
        })
        .await
        .map_err(io::Error::other)?
    }
}

pub(crate) async fn prerequisite_report(language: Language) -> String {
    match Runtime::external(language) {
        Ok(None) => "The managed Kotlin archive includes its Java 25 runtime".into(),
        Ok(runtime) => Launch {
            argv: Vec::new(),
            origin: String::new(),
            java_configuration: None,
            runtime,
        }
        .runtime_report()
        .await
        .unwrap_or_else(|error| error.to_string()),
        Err(error) => error.to_string(),
    }
}

/// None means genuinely absent, not a malformed override or a broken installation.
pub(crate) fn resolve(config: &Config, language: Language) -> io::Result<Option<Launch>> {
    let override_command = config
        .commands
        .get(&language)
        .cloned()
        .or_else(|| std::env::var_os(language.environment()).map(|value| vec![value]));
    if let Some(mut argv) = override_command {
        if argv.len() == 1 && argv[0].to_string_lossy().starts_with('[') {
            let parsed: Vec<String> =
                serde_json::from_str(&argv[0].to_string_lossy()).map_err(|e| {
                    io::Error::other(format!(
                        "{} must be a JSON argv array: {e}",
                        language.environment()
                    ))
                })?;
            argv = parsed.into_iter().map(OsString::from).collect();
        }
        if argv.is_empty() || argv[0].is_empty() {
            return Err(io::Error::other("Empty language-server command"));
        }
        return Ok(Some(Launch {
            argv,
            origin: format!("Override ({})", language.environment()),
            java_configuration: None,
            runtime: None,
        }));
    }
    let name = match language {
        Language::Kotlin => "intellij-server",
        Language::Java => "jdtls",
        Language::CSharp => "csharp-ls",
    };
    let mut program = executable(name);
    #[cfg(test)]
    if config.installation.ignore_path {
        program = None;
    }
    if let Some(program) = program.take() {
        let runtime = if language == Language::Kotlin {
            let bundled = program
                .canonicalize()
                .ok()
                .and_then(|p| p.parent()?.parent().map(Path::to_path_buf))
                .map(|p| p.join(bundled_java()))
                .filter(|p| is_executable(p));
            Some(Runtime {
                program: bundled.or_else(java).ok_or_else(|| {
                    io::Error::other(
                        "Kotlin requires its bundled runtime or Java 25+; set JAVA_HOME or PATH",
                    )
                })?,
                minimum: 25,
                dotnet: false,
            })
        } else {
            Runtime::external(language)?
        };
        let mut argv = vec![program.into_os_string()];
        if language == Language::Kotlin {
            argv.push("--stdio".into());
        }
        return Ok(Some(Launch {
            argv,
            origin: "PATH".into(),
            java_configuration: None,
            runtime,
        }));
    }
    let Some(root) = &config.installation.directory else {
        return Ok(None);
    };
    let recipe = recipe(language, &config.installation)?;
    let destination = recipe.destination(root, language);
    if !installed(&destination, &recipe)? {
        return Ok(None);
    }
    managed_launch(&destination, language, &recipe).map(Some)
}

fn bundled_java() -> &'static str {
    if cfg!(target_os = "macos") {
        "jbr/Contents/Home/bin/java"
    } else {
        "jbr/bin/java"
    }
}

fn managed_launch(directory: &Path, language: Language, recipe: &Recipe) -> io::Result<Launch> {
    let mut launch = Launch {
        argv: Vec::new(),
        origin: format!("Managed {} ({})", recipe.version, directory.display()),
        java_configuration: None,
        runtime: Runtime::external(language)?,
    };
    match language {
        Language::Kotlin => {
            let home = directory.join(format!("kotlin-server-{}", recipe.version));
            let program = home.join("bin/intellij-server");
            let runtime = home.join(bundled_java());
            if !is_executable(&program) || !is_executable(&runtime) {
                return Err(io::Error::other(
                    "Incomplete managed Kotlin installation; retry :LspInstall",
                ));
            }
            launch.argv = vec![program.into_os_string(), "--stdio".into()];
            launch.runtime = Some(Runtime {
                program: runtime,
                minimum: 25,
                dotnet: false,
            });
        }
        Language::Java => {
            let mut jars: Vec<_> = fs::read_dir(directory.join("plugins"))?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().and_then(|s| s.to_str()).is_some_and(|s| {
                        s.starts_with("org.eclipse.equinox.launcher_") && s.ends_with(".jar")
                    })
                })
                .collect();
            if jars.len() != 1 {
                return Err(io::Error::other("Expected one JDT LS launcher jar"));
            }
            let suffix = match (std::env::consts::OS, std::env::consts::ARCH) {
                ("macos", "aarch64") => "mac_arm",
                ("macos", _) => "mac",
                ("linux", "aarch64") => "linux_arm",
                _ => "linux",
            };
            let configuration = directory.join(format!("config_{suffix}"));
            if !configuration.join("config.ini").is_file() {
                return Err(io::Error::other(
                    "Missing platform configuration in JDT LS archive",
                ));
            }
            launch.argv = vec![
                launch
                    .runtime
                    .as_ref()
                    .unwrap()
                    .program
                    .clone()
                    .into_os_string(),
                "-Declipse.application=org.eclipse.jdt.ls.core.id1".into(),
                "-Dosgi.bundles.defaultStartLevel=4".into(),
                "-Declipse.product=org.eclipse.jdt.ls.core.product".into(),
                "-Dosgi.checkConfiguration=true".into(),
                "-Dosgi.sharedConfiguration.area.readOnly=true".into(),
                "-Dosgi.configuration.cascaded=true".into(),
                format!(
                    "-Dosgi.sharedConfiguration.area={}",
                    configuration.display()
                )
                .into(),
                "-Djdk.xml.maxGeneralEntitySizeLimit=0".into(),
                "-Djdk.xml.totalEntitySizeLimit=0".into(),
                "-Xmx1G".into(),
                "--add-modules=ALL-SYSTEM".into(),
                "--add-opens=java.base/java.util=ALL-UNNAMED".into(),
                "--add-opens=java.base/java.lang=ALL-UNNAMED".into(),
                "-jar".into(),
                jars.remove(0).into_os_string(),
            ];
            launch.java_configuration = Some(configuration);
        }
        Language::CSharp => {
            let dll = directory.join(format!("tools/.store/csharp-ls/{0}/csharp-ls/{0}/tools/net10.0/any/CSharpLanguageServer.dll", recipe.version));
            if !dll.is_file() {
                return Err(io::Error::other(
                    "Incomplete managed csharp-ls installation; retry :LspInstall",
                ));
            }
            launch.argv = vec![
                launch
                    .runtime
                    .as_ref()
                    .unwrap()
                    .program
                    .clone()
                    .into_os_string(),
                "exec".into(),
                "--roll-forward".into(),
                "LatestMajor".into(),
                dll.into_os_string(),
            ];
        }
    }
    Ok(launch)
}

fn installed(directory: &Path, recipe: &Recipe) -> io::Result<bool> {
    let file = match File::open(directory.join("installed.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other("Invalid LSP installation receipt"));
    }
    let receipt: Receipt = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if receipt != recipe.receipt() {
        return Err(io::Error::other(
            "LSP installation receipt does not match the pinned distribution",
        ));
    }
    Ok(true)
}

struct Job {
    cancel: Arc<AtomicBool>,
    state: watch::Receiver<State>,
    thread: thread::JoinHandle<()>,
}
pub(crate) struct Installer {
    options: Options,
    jobs: BTreeMap<Language, Job>,
    attempted: BTreeSet<Language>,
}
impl Installer {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            jobs: BTreeMap::new(),
            attempted: BTreeSet::new(),
        }
    }
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }
    pub fn automatic(&mut self, language: Language) -> io::Result<()> {
        if self.options.auto_install && !self.attempted.contains(&language) {
            self.start(language)?;
        }
        Ok(())
    }
    pub fn start(&mut self, language: Language) -> io::Result<()> {
        if self.jobs.contains_key(&language) {
            return Ok(());
        }
        self.attempted.insert(language);
        let root = self.options.directory.clone().ok_or_else(|| {
            io::Error::other("Cannot locate managed LSP directory; set HOME or XDG_DATA_HOME")
        })?;
        if !root.is_absolute() {
            return Err(io::Error::other("Managed LSP directory must be absolute"));
        }
        let recipe = recipe(language, &self.options)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let guard = Guard::new(cancel.clone());
        let (sender, state) = watch::channel(State {
            message: "Installation queued".into(),
            busy: true,
            failed: false,
        });
        let thread = thread::Builder::new()
            .name(format!("install-{}", language.id()))
            .spawn(move || {
                let progress = |message: String| {
                    sender.send_replace(State {
                        message,
                        busy: true,
                        failed: false,
                    });
                };
                let result = install(&root, language, &recipe, &guard, &progress);
                let state = match result {
                    Ok(()) => State {
                        message: format!(
                            "Installed {} {}; ready to start",
                            language.id(),
                            recipe.version
                        ),
                        busy: false,
                        failed: false,
                    },
                    Err(e) => State {
                        message: format!("{}; :LspInstall {} retries", e, language.id()),
                        busy: false,
                        failed: true,
                    },
                };
                sender.send_replace(state);
            })?;
        self.jobs.insert(
            language,
            Job {
                cancel,
                state,
                thread,
            },
        );
        Ok(())
    }
    pub fn cancel(&self, language: Language) -> bool {
        if let Some(job) = self.jobs.get(&language) {
            job.cancel.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
    pub fn poll(&mut self) -> Vec<(Language, State)> {
        let mut updates = Vec::new();
        let mut finished = Vec::new();
        for (&language, job) in &mut self.jobs {
            if job.state.has_changed().unwrap_or(true) {
                let mut state = job.state.borrow_and_update().clone();
                if job.thread.is_finished() && state.busy {
                    state = State {
                        message: "Installation worker stopped; :LspInstall retries".into(),
                        busy: false,
                        failed: true,
                    };
                }
                if !state.busy {
                    finished.push(language);
                }
                updates.push((language, state));
            }
        }
        for language in finished {
            if let Some(job) = self.jobs.remove(&language) {
                let _ = job.thread.join();
            }
        }
        updates
    }
}
impl Drop for Installer {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        for (_, job) in std::mem::take(&mut self.jobs) {
            let _ = job.thread.join();
        }
    }
}

struct Guard {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}
impl Guard {
    fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            deadline: Instant::now() + DEADLINE,
        }
    }
    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::other("LSP installation cancelled"))
        } else if Instant::now() >= self.deadline {
            Err(io::Error::other("LSP installation timed out"))
        } else {
            Ok(())
        }
    }
}

fn private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::other(
            "Managed installation directory must be a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn install(
    root: &Path,
    language: Language,
    recipe: &Recipe,
    guard: &Guard,
    progress: &impl Fn(String),
) -> io::Result<()> {
    guard.check()?;
    progress("Checking runtime and installation".into());
    if let Some(runtime) = Runtime::external(language)? {
        runtime.check(guard)?;
    }
    private_directory(root)?;
    let parent = root.join(language.id());
    private_directory(&parent)?;
    let lock_path = parent.join("install.lock");
    if fs::symlink_metadata(&lock_path).is_ok_and(|m| !m.is_file()) {
        return Err(io::Error::other("Invalid LSP install lock"));
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    progress("Waiting for installation lock".into());
    loop {
        guard.check()?;
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50))
            }
            Err(e) => return Err(e),
        }
    }
    // OS file locks release on process exit; an interrupted editor cannot leave a stale lock.
    let destination = recipe.destination(root, language);
    if installed(&destination, recipe)? && managed_launch(&destination, language, recipe).is_ok() {
        return Ok(());
    }
    if destination.exists() {
        return Err(io::Error::other(format!(
            "Incomplete installation at {}; remove that directory and retry",
            destination.display()
        )));
    }
    for entry in fs::read_dir(&parent)?.take(128) {
        guard.check()?;
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(".install-")
            && entry.file_type()?.is_dir()
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&parent)?;
    let payload = staging.path().join("payload");
    private_directory(&payload)?;
    let download_path = staging.path().join("download");
    download(recipe, &download_path, guard, progress)?;
    progress("Extracting verified server package".into());
    match recipe.format {
        Format::Tar | Format::Zip => {
            archive::extract(&download_path, &payload, recipe.format, guard)?
        }
        Format::Nuget => install_dotnet(&download_path, staging.path(), &payload, recipe, guard)?,
    }
    guard.check()?;
    let launch = managed_launch(&payload, language, recipe)?;
    if let Some(runtime) = launch.runtime {
        runtime.check(guard)?;
    }
    let mut receipt = File::create(payload.join("installed.json"))?;
    serde_json::to_writer(&mut receipt, &recipe.receipt()).map_err(io::Error::other)?;
    receipt.sync_all()?;
    guard.check()?;
    fs::rename(payload, &destination)?;
    File::open(&parent)?.sync_all()?;
    Ok(())
}

fn download(
    recipe: &Recipe,
    path: &Path,
    guard: &Guard,
    progress: &impl Fn(String),
) -> io::Result<()> {
    progress(format!(
        "Downloading {} (SHA-256 verified before extraction)",
        recipe.version
    ));
    let agent = http::agent(guard.cancel.clone());
    let mut response = agent.get(&recipe.url).call().map_err(io::Error::other)?;
    let length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if length.is_some_and(|n| n > MAX_DOWNLOAD) {
        return Err(io::Error::other("LSP download exceeds 1 GiB"));
    }
    let mut reader = response.body_mut().as_reader();
    let mut file = File::create(path)?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut last = Instant::now();
    let mut bytes = [0; 64 * 1024];
    loop {
        guard.check()?;
        let n = reader.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > MAX_DOWNLOAD {
            return Err(io::Error::other("LSP download exceeds 1 GiB"));
        }
        file.write_all(&bytes[..n])?;
        hash.update(&bytes[..n]);
        if last.elapsed() >= Duration::from_millis(250) {
            progress(format!(
                "Downloading: {} MiB{}",
                count / 1024 / 1024,
                length.map_or(String::new(), |total| format!(
                    " / {} MiB",
                    total / 1024 / 1024
                ))
            ));
            last = Instant::now();
        }
    }
    if length.is_some_and(|length| count != length) {
        return Err(io::Error::other("Incomplete LSP download"));
    }
    if format!("{:x}", hash.finalize()) != recipe.sha256 {
        return Err(io::Error::other("LSP download checksum mismatch"));
    }
    Ok(())
}

fn install_dotnet(
    package: &Path,
    staging: &Path,
    payload: &Path,
    recipe: &Recipe,
    guard: &Guard,
) -> io::Result<()> {
    let sdk = Runtime::external(Language::CSharp)?.unwrap();
    let feed = staging.join("feed");
    private_directory(&feed)?;
    fs::rename(
        package,
        feed.join(format!("csharp-ls.{}.nupkg", recipe.version)),
    )?;
    let config = staging.join("nuget.config");
    fs::write(
        &config,
        "<configuration><packageSources><clear/><add key=\"verified\" value=\"feed\"/></packageSources></configuration>",
    )?;
    let env = [
        (
            "DOTNET_CLI_HOME",
            staging.join("dotnet-home").into_os_string(),
        ),
        ("NUGET_PACKAGES", staging.join("packages").into_os_string()),
        ("DOTNET_CLI_TELEMETRY_OPTOUT", "1".into()),
        ("DOTNET_NOLOGO", "1".into()),
    ];
    capture(
        &sdk.program,
        &[
            "tool".into(),
            "install".into(),
            "csharp-ls".into(),
            "--version".into(),
            recipe.version.clone().into(),
            "--tool-path".into(),
            payload.join("tools").into_os_string(),
            "--configfile".into(),
            config.into_os_string(),
            "--no-cache".into(),
            "--allow-roll-forward".into(),
        ],
        &env,
        Some(staging),
        guard,
        DEADLINE,
    )?;
    Ok(())
}

fn capture(
    program: &Path,
    args: &[OsString],
    env: &[(&str, OsString)],
    cwd: Option<&Path>,
    guard: &Guard,
    limit: Duration,
) -> io::Result<String> {
    let mut file = tempfile::tempfile()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file.try_clone()?);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let start = Instant::now();
    let result = loop {
        let check = guard.check().and_then(|()| {
            if start.elapsed() > limit {
                Err(io::Error::other("Runtime/installer process timed out"))
            } else if file.metadata()?.len() > 1024 * 1024 {
                Err(io::Error::other("Runtime/installer output exceeds 1 MiB"))
            } else {
                Ok(())
            }
        });
        if let Err(e) = check {
            break Err(e);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(30)),
            Err(e) => break Err(e),
        }
    };
    // Clean descendants on both success and failure (including inherited output handles).
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
    let status = result?;
    use std::io::Seek;
    file.rewind()?;
    let mut bytes = Vec::new();
    file.take(8192).read_to_end(&mut bytes)?;
    let output = String::from_utf8_lossy(&bytes).into_owned();
    if !status.success() {
        return Err(io::Error::other(format!(
            "{} exited with {status}: {}",
            program.display(),
            output.trim()
        )));
    }
    Ok(output)
}
