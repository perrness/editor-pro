//! Explicit private transcript files, separate from source and review metadata.
use crate::{
    ai::{Conversation, MAX_STORE, error},
    file::absolute_path,
    review::fingerprint,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

fn read(path: &Path) -> io::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_STORE as u64 {
        return Err(error(
            "Conversation must be a regular file of at most 4 MiB",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(error("Hard-linked conversation files are unsupported"));
        }
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_STORE as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STORE {
        return Err(error("Conversation exceeds 4 MiB"));
    }
    Ok(bytes)
}
pub fn load(path: &Path) -> io::Result<(Conversation, String)> {
    let bytes = read(path)?;
    let conversation: Conversation = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    conversation.validate()?;
    Ok((conversation, fingerprint(&bytes)))
}

pub fn save(
    conversation: &Conversation,
    path: &Path,
    expected: Option<&str>,
    protected: &[PathBuf],
) -> io::Result<(PathBuf, String)> {
    conversation.validate()?;
    let path = absolute_path(path)?;
    if path.starts_with(&conversation.root)
        || protected.contains(&path)
        || path.components().any(|c| c.as_os_str() == ".git")
    {
        return Err(error(
            "Save conversations outside the source project and Git metadata",
        ));
    }
    let bytes = serde_json::to_vec_pretty(conversation).map_err(io::Error::other)?;
    if bytes.len() > MAX_STORE {
        return Err(error("Conversation exceeds 4 MiB"));
    }
    let mut lock_name = path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let _lock_file = options
        .open(&lock_path)
        .map_err(|e| error(&format!("Conversation lock: {e}")))?;
    let _lock = Lock(lock_path);
    let check = || -> io::Result<()> {
        match expected {
            Some(hash) if fingerprint(&read(&path)?) == hash => Ok(()),
            Some(_) => Err(error(
                "Conversation changed on disk; save to a new path or reload",
            )),
            None => match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
                Ok(_) => Err(error("Conversation destination exists; choose a new path")),
            },
        }
    };
    check()?;
    let parent = path
        .parent()
        .ok_or_else(|| error("No conversation directory"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    check()?;
    if expected.is_some() {
        temporary.persist(&path).map_err(io::Error::other)?;
    } else {
        temporary
            .persist_noclobber(&path)
            .map_err(io::Error::other)?;
    }
    File::open(parent)?.sync_all().map_err(|e| {
        error(&format!(
            "Conversation written, durability uncertain: {e}; reload or save to a new path"
        ))
    })?;
    Ok((path, fingerprint(&bytes)))
}
struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
