//! Conservative local-file I/O. A failed save never marks the buffer clean.
use std::{
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use tempfile::Builder;

use crate::buffer::Buffer;
use crate::syntax::Syntax;

pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Last background observation, independent of editing/rendering state. Missing
/// and empty files are distinct, and an I/O failure is never a clean sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiskObservation {
    pub content: Result<Option<[u8; 32]>, String>,
    pub changed: bool,
}

pub struct Document {
    pub buffer: Buffer,
    path: Option<PathBuf>,
    baseline: Option<Arc<[u8]>>,
    pub(crate) baseline_generation: u64,
    pub(crate) disk_observation: Option<DiskObservation>,
    pub external_changed: bool,
    pub syntax: Syntax,
    save_generation: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            buffer: Buffer::new(""),
            path: None,
            baseline: None,
            baseline_generation: 0,
            disk_observation: None,
            external_changed: false,
            syntax: Syntax::for_path(None),
            save_generation: 0,
        }
    }
}

impl Document {
    pub fn open(path: &Path) -> io::Result<Self> {
        let path = absolute_path(path)?;
        let baseline = read_optional(&path)?.map(Arc::from);
        Self::from_disk(path, baseline)
    }

    pub(crate) fn from_disk(path: PathBuf, baseline: Option<Arc<[u8]>>) -> io::Result<Self> {
        let text = std::str::from_utf8(baseline.as_deref().unwrap_or_default())
            .map_err(|_| io::Error::other("Only UTF-8 text files are supported"))?;
        if text.contains('\0') {
            return Err(io::Error::other(
                "Refusing a binary file containing NUL bytes",
            ));
        }
        Ok(Self {
            buffer: Buffer::new(text),
            syntax: Syntax::for_path(Some(&path)),
            path: Some(path),
            baseline,
            baseline_generation: 0,
            disk_observation: None,
            external_changed: false,
            save_generation: 0,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn baseline(&self) -> Option<Arc<[u8]>> {
        self.baseline.clone()
    }

    pub fn save_generation(&self) -> u64 {
        self.save_generation
    }

    pub fn sync_syntax(&mut self) {
        let edits = self.buffer.take_edits();
        self.syntax.update(self.buffer.text(), &edits);
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .map_or_else(|| "[No Name]".into(), |p| p.display().to_string())
    }

    pub fn check_external(&mut self) -> io::Result<bool> {
        let changed = match &self.path {
            Some(path) => read_optional(path)?.as_deref() != self.baseline.as_deref(),
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
        if disk.as_deref() != expected.map(AsRef::as_ref) {
            if same_path {
                self.external_changed = true;
            }
            return Err(io::Error::other(
                "File changed on disk or destination exists. :refresh compares/reloads; :w NEW_PATH keeps edits",
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
        if read_optional(&path)?.as_deref() != expected.map(AsRef::as_ref) {
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
        if !same_path {
            self.syntax = Syntax::for_path(Some(&path));
        }
        self.path = Some(path);
        self.baseline = Some(self.buffer.text().to_string().into_bytes().into());
        self.baseline_generation += 1;
        self.disk_observation = None;
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
        self.save_generation += 1;
        Ok(())
    }
}

pub(crate) fn absolute_path(path: &Path) -> io::Result<PathBuf> {
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

pub(crate) fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
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
            return Err(io::Error::other("Supports files up to 64 MiB"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // An agent can replace the entry between metadata and open. Never
        // follow a replacement symlink or block opening a replacement FIFO.
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("Expected a regular file up to 64 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::other("Supports files up to 64 MiB"));
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
