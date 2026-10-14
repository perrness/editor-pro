//! Conservative local-file I/O. A failed save never marks the buffer clean.
use std::{
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
};

use tempfile::Builder;

use crate::buffer::Buffer;

pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

pub struct Document {
    pub buffer: Buffer,
    path: Option<PathBuf>,
    baseline: Option<Vec<u8>>,
    pub external_changed: bool,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            buffer: Buffer::new(""),
            path: None,
            baseline: None,
            external_changed: false,
        }
    }
}

impl Document {
    pub fn open(path: &Path) -> io::Result<Self> {
        let path = absolute_path(path)?;
        let baseline = read_optional(&path)?;
        let text = std::str::from_utf8(baseline.as_deref().unwrap_or_default())
            .map_err(|_| io::Error::other("Only UTF-8 text files are supported"))?;
        if text.contains('\0') {
            return Err(io::Error::other(
                "Refusing a binary file containing NUL bytes",
            ));
        }
        Ok(Self {
            buffer: Buffer::new(text),
            path: Some(path),
            baseline,
            external_changed: false,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .map_or_else(|| "[No Name]".into(), |p| p.display().to_string())
    }

    pub fn check_external(&mut self) -> io::Result<bool> {
        let changed = match &self.path {
            Some(path) => read_optional(path)? != self.baseline,
            None => false,
        };
        self.external_changed = changed;
        Ok(changed)
    }

    /// Save-as adopts a new, previously nonexistent path. Existing destinations
    /// are only replaceable when this document has an exact disk baseline.
    pub fn save(&mut self, destination: Option<&Path>) -> io::Result<()> {
        self.buffer.commit();
        let path = match destination {
            Some(path) => absolute_path(path)?,
            None => self
                .path
                .clone()
                .ok_or_else(|| io::Error::other("No file name. Use :w path/to/file"))?,
        };
        let same_path = self.path.as_ref() == Some(&path);
        let expected = if same_path {
            self.baseline.as_ref()
        } else {
            None
        };
        let disk = read_optional(&path)?;
        if disk.as_ref() != expected {
            if same_path {
                self.external_changed = true;
            }
            return Err(io::Error::other(
                "File changed on disk or destination exists. Use :w NEW_PATH to keep edits; :e! reloads",
            ));
        }

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                validate_save_target(&metadata)?;
                Some(metadata)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("No parent directory"))?
            .to_path_buf();
        let mut temporary = Builder::new().prefix(".editor-pro-").tempfile_in(&parent)?;
        if let Some(metadata) = &metadata {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.uid() != temporary.as_file().metadata()?.uid() {
                    return Err(io::Error::other(
                        "Refusing to change file ownership during save",
                    ));
                }
            }
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
        }
        self.buffer.text().write_to(temporary.as_file_mut())?;
        temporary.as_file().sync_all()?;

        // Narrow (but cannot eliminate) the race with non-cooperating writers.
        if read_optional(&path)?.as_ref() != expected {
            if same_path {
                self.external_changed = true;
            }
            return Err(io::Error::other(
                "File changed during save; original left untouched",
            ));
        }
        if metadata.is_some() {
            validate_save_target(&fs::symlink_metadata(&path)?)?;
            temporary.persist(&path).map_err(|error| error.error)?;
        } else {
            temporary
                .persist_noclobber(&path)
                .map_err(|error| error.error)?;
        }

        // Once renamed, this is the new baseline even if directory fsync fails.
        self.path = Some(path);
        self.baseline = Some(self.buffer.text().to_string().into_bytes());
        self.external_changed = false;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                io::Error::other(format!(
                    "File replaced, but directory sync failed: {error}. Save again before quitting"
                ))
            })?;
        self.buffer.mark_saved();
        Ok(())
    }
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::other("Empty file name"));
    }
    // Resolve the parent once, but keep the final component so symlinks can be
    // detected rather than accidentally replaced by an atomic rename.
    let absolute = std::env::current_dir()?.join(path);
    let name = absolute
        .file_name()
        .ok_or_else(|| io::Error::other("Expected a file path"))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| io::Error::other("No parent directory"))?;
    Ok(parent.canonicalize()?.join(name))
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::other(
                "Symlink files are not supported yet; open their real target path",
            ));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(io::Error::other("Expected a regular file"));
        }
        Ok(metadata) if metadata.len() > MAX_FILE_BYTES => {
            return Err(io::Error::other("Milestone 1 supports files up to 64 MiB"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::other("Milestone 1 supports files up to 64 MiB"));
    }
    Ok(Some(bytes))
}

fn validate_save_target(metadata: &Metadata) -> io::Result<()> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::other("Refusing to replace a non-regular file"));
    }
    if metadata.permissions().readonly() {
        return Err(io::Error::other("File is read-only"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(io::Error::other(
                "Refusing to break hard links. Use :w NEW_PATH",
            ));
        }
    }
    Ok(())
}
