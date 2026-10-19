//! Pinned full-bytecode fallback for servers that expose declaration stubs.
use super::*;

const VERSION: &str = "1.12.0";
const SHA256: &str = "753fd20d0b33ebf20c327d076e437557da0d8b42f121e1983b9cbe2b5b3e0219";
const URL: &str =
    "https://github.com/Vineflower/vineflower/releases/download/1.12.0/vineflower-1.12.0-slim.jar";
const MAX_JAR: u64 = 8 * 1024 * 1024;
static DECOMPILERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

fn verified(path: &Path) -> io::Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_JAR {
        return Err(io::Error::other("Invalid Vineflower artifact"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_JAR + 1).read_to_end(&mut bytes)?;
    if format!("{:x}", Sha256::digest(&bytes)) != SHA256 {
        return Err(io::Error::other("Vineflower checksum mismatch"));
    }
    Ok(true)
}

pub(super) fn ensure(
    root: &Path,
    allow_download: bool,
    guard: &Guard,
    progress: &impl Fn(String),
) -> io::Result<PathBuf> {
    if !root.is_absolute() {
        return Err(io::Error::other(
            "Managed decompiler directory must be absolute",
        ));
    }
    let parent = root.join("decompiler");
    let path = parent.join(format!("vineflower-{VERSION}.jar"));
    if verified(&path)? {
        return Ok(path);
    }
    if !allow_download {
        return Err(io::Error::other(
            "Full bytecode decompiler is not installed; use :LspInstall kotlin",
        ));
    }
    private_directory(&parent)?;
    let lock_path = parent.join("install.lock");
    if fs::symlink_metadata(&lock_path).is_ok_and(|m| !m.is_file()) {
        return Err(io::Error::other("Invalid decompiler install lock"));
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    loop {
        guard.check()?;
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(30))
            }
            Err(e) => return Err(e),
        }
    }
    if verified(&path)? {
        return Ok(path);
    }
    progress(format!(
        "Downloading Vineflower {VERSION} bytecode decompiler"
    ));
    let agent = http::agent(guard.cancel.clone());
    let mut response = agent.get(URL).call().map_err(io::Error::other)?;
    let mut input = response.body_mut().as_reader();
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut bytes = [0; 65536];
    loop {
        guard.check()?;
        let n = input.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_JAR {
            return Err(io::Error::other("Decompiler download exceeds 8 MiB"));
        }
        hash.update(&bytes[..n]);
        temporary.write_all(&bytes[..n])?;
    }
    if format!("{:x}", hash.finalize()) != SHA256 {
        return Err(io::Error::other("Vineflower checksum mismatch"));
    }
    temporary.as_file().sync_all()?;
    guard.check()?;
    temporary.persist(&path).map_err(io::Error::other)?;
    Ok(path)
}

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(crate) async fn decompile(config: &Config, uri: &str, limit: Duration) -> io::Result<String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel = Cancel(cancel.clone());
    let config = config.clone();
    let uri = uri.to_owned();
    let deadline = Instant::now() + limit;
    let permit = DECOMPILERS.acquire().await.map_err(io::Error::other)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let guard = Guard { cancel, deadline };
        let root = config.installation.directory.as_ref().ok_or_else(|| {
            io::Error::other(
                "Full bytecode decompilation requires managed LSP storage; use :LspInstall kotlin",
            )
        })?;
        let jar = ensure(root, config.installation.auto_install, &guard, &|_| {})?;
        let managed_runtime = || {
            let recipe = recipe(Language::Kotlin, &config.installation).ok()?;
            let directory = recipe.destination(root, Language::Kotlin);
            installed(&directory, &recipe)
                .ok()
                .filter(|installed| *installed)?;
            managed_launch(&directory, Language::Kotlin, &recipe)
                .ok()?
                .runtime
                .map(|r| r.program)
        };
        let runtime = resolve(&config, Language::Kotlin)?
            .and_then(|launch| launch.runtime.map(|r| r.program))
            .or_else(managed_runtime)
            .or_else(java)
            .ok_or_else(|| {
                io::Error::other(
                    "Bytecode decompilation needs Java 17+ or the managed Kotlin runtime",
                )
            })?;
        let work = tempfile::Builder::new()
            .prefix("editor-pro-bytecode-")
            .tempdir()?;
        let helper = work.path().join("Decompile.java");
        fs::write(&helper, include_str!("Decompile.java"))?;
        capture(
            &runtime,
            &[
                "-Xmx256m".into(),
                "--class-path".into(),
                jar.into_os_string(),
                helper.into_os_string(),
                uri.into(),
                work.path().as_os_str().into(),
            ],
            &[],
            Some(work.path()),
            &guard,
            limit,
        )?;
        guard.check()?;
        let mut text = String::new();
        File::open(work.path().join("source.java"))?
            .take(crate::lsp::MAX_DOCUMENT_BYTES as u64 + 1)
            .read_to_string(&mut text)?;
        if text.is_empty() || text.len() > crate::lsp::MAX_DOCUMENT_BYTES || text.contains('\0') {
            return Err(io::Error::other("Invalid or oversized decompiler output"));
        }
        Ok(text)
    })
    .await
    .map_err(io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_corrupt_decompiler_never_runs_or_downloads_with_installation_disabled() {
        let root = tempfile::tempdir().unwrap();
        let guard = Guard::new(Arc::new(AtomicBool::new(false)));
        let error = ensure(root.path(), false, &guard, &|_| {
            panic!("unexpected download")
        })
        .unwrap_err();
        assert!(error.to_string().contains(":LspInstall kotlin"));
        assert!(!root.path().join("decompiler").exists());
        fs::create_dir(root.path().join("decompiler")).unwrap();
        let path = root
            .path()
            .join(format!("decompiler/vineflower-{VERSION}.jar"));
        fs::write(path, b"not the pinned executable").unwrap();
        assert!(
            ensure(root.path(), false, &guard, &|_| panic!(
                "unexpected download"
            ))
            .unwrap_err()
            .to_string()
            .contains("checksum mismatch")
        );
    }
}
