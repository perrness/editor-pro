//! Extract into a new private directory, with links deferred until all regular
//! files are written. Paths and links may never escape that directory.
use super::{Format, Guard};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_ENTRIES: usize = 100_000;
const MAX_EXPANDED: u64 = 4 * 1024 * 1024 * 1024;

pub(super) fn extract(source: &Path, root: &Path, format: Format, guard: &Guard) -> io::Result<()> {
    let mut unpack = Unpack {
        root,
        guard,
        count: 0,
        bytes: 0,
        links: Vec::new(),
    };
    match format {
        Format::Tar => {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(source)?));
            for entry in archive.entries()? {
                let mut entry = entry?;
                let path = entry.path()?.into_owned();
                let kind = entry.header().entry_type();
                let mode = entry.header().mode()?;
                let size = entry.size();
                if kind.is_dir() {
                    unpack.directory(&path)?;
                } else if kind.is_file() {
                    unpack.file(&path, size, mode, &mut entry)?;
                } else if kind.is_symlink() || kind.is_hard_link() {
                    let target = entry
                        .link_name()?
                        .ok_or_else(|| io::Error::other("Missing archive link target"))?
                        .into_owned();
                    unpack.link(&path, target, kind.is_hard_link())?;
                } else {
                    return Err(io::Error::other("Unsupported special file in LSP archive"));
                }
            }
        }
        Format::Zip => {
            let mut archive = zip::ZipArchive::new(File::open(source)?)?;
            if archive.len() > MAX_ENTRIES {
                return Err(io::Error::other("Too many files in LSP archive"));
            }
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i)?;
                let path = PathBuf::from(entry.name());
                if entry.is_symlink() {
                    if entry.size() > 4096 {
                        return Err(io::Error::other("Oversized archive link"));
                    }
                    let mut target = String::new();
                    entry.read_to_string(&mut target)?;
                    unpack.link(&path, PathBuf::from(target), false)?;
                } else if entry.is_dir() {
                    unpack.directory(&path)?;
                } else {
                    let mode = entry.unix_mode().unwrap_or(0o600);
                    if !matches!(mode & 0o170000, 0 | 0o100000) {
                        return Err(io::Error::other("Unsupported ZIP file type"));
                    }
                    unpack.file(&path, entry.size(), mode, &mut entry)?;
                }
            }
        }
        Format::Nuget => unreachable!(),
    }
    unpack.finish()
}

fn safe_path(path: &Path) -> io::Result<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) if !name.to_string_lossy().contains(['\\', ':']) => {
                result.push(name)
            }
            Component::CurDir => {}
            _ => return Err(io::Error::other("Unsafe path in LSP archive")),
        }
    }
    if result.as_os_str().is_empty() {
        return Err(io::Error::other("Empty path in LSP archive"));
    }
    Ok(result)
}

struct Unpack<'a> {
    root: &'a Path,
    guard: &'a Guard,
    count: usize,
    bytes: u64,
    links: Vec<(PathBuf, PathBuf, bool)>,
}
impl Unpack<'_> {
    fn path(&mut self, path: &Path) -> io::Result<PathBuf> {
        self.guard.check()?;
        self.count += 1;
        if self.count > MAX_ENTRIES {
            return Err(io::Error::other("Too many files in LSP archive"));
        }
        Ok(self.root.join(safe_path(path)?))
    }
    fn directory(&mut self, path: &Path) -> io::Result<()> {
        let path = self.path(path)?;
        fs::create_dir_all(path)
    }
    fn file(
        &mut self,
        path: &Path,
        size: u64,
        mode: u32,
        reader: &mut impl Read,
    ) -> io::Result<()> {
        let path = self.path(path)?;
        self.bytes = self
            .bytes
            .checked_add(size)
            .ok_or_else(|| io::Error::other("Archive size overflow"))?;
        if self.bytes > MAX_EXPANDED {
            return Err(io::Error::other("Expanded LSP archive exceeds 4 GiB"));
        }
        fs::create_dir_all(path.parent().unwrap())?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut bytes = [0; 64 * 1024];
        let mut written = 0u64;
        loop {
            self.guard.check()?;
            let n = reader.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > size {
                return Err(io::Error::other("Archive entry exceeds declared size"));
            }
            output.write_all(&bytes[..n])?;
        }
        if written != size {
            return Err(io::Error::other("Truncated archive entry"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600 | (mode & 0o111)))?;
        }
        Ok(())
    }
    fn link(&mut self, path: &Path, target: PathBuf, hard: bool) -> io::Result<()> {
        let path = self.path(path)?;
        if target.as_os_str().len() > 4096 {
            return Err(io::Error::other("Oversized archive link"));
        }
        let mut resolved = if hard {
            PathBuf::new()
        } else {
            path.parent()
                .unwrap()
                .strip_prefix(self.root)
                .unwrap()
                .to_path_buf()
        };
        for component in target.components() {
            match component {
                Component::Normal(name) if !name.to_string_lossy().contains(['\\', ':']) => {
                    resolved.push(name)
                }
                Component::ParentDir if resolved.pop() => {}
                Component::CurDir => {}
                _ => {
                    return Err(io::Error::other(
                        "Archive link escapes installation directory",
                    ));
                }
            }
        }
        self.links.push((
            path,
            if hard {
                self.root.join(resolved)
            } else {
                target
            },
            hard,
        ));
        Ok(())
    }
    fn finish(self) -> io::Result<()> {
        let root = self.root.canonicalize()?;
        for (path, target, hard) in &self.links {
            self.guard.check()?;
            // Links cannot become ancestors of later link writes either.
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if directory == self.root {
                    break;
                }
                if fs::symlink_metadata(directory).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(io::Error::other("Archive link has a symlink parent"));
                }
                parent = directory.parent();
            }
            fs::create_dir_all(path.parent().unwrap())?;
            if *hard {
                if !target.canonicalize()?.starts_with(&root)
                    || !fs::symlink_metadata(target)?.is_file()
                {
                    return Err(io::Error::other("Invalid archive hard link"));
                }
                fs::hard_link(target, path)?;
            } else {
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, path)?;
                #[cfg(not(unix))]
                return Err(io::Error::other("Archive symlinks require Unix"));
            }
        }
        for (path, _, _) in &self.links {
            if !path.canonicalize()?.starts_with(&root) {
                return Err(io::Error::other(
                    "Archive link escapes installation directory",
                ));
            }
        }
        Ok(())
    }
}
