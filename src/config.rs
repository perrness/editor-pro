//! Optional user settings. Installation and startup never create or rewrite them.

use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::theme::ThemeName;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: ThemeName,
}

pub fn config_path() -> Option<PathBuf> {
    user_config_dir(
        cfg!(target_os = "macos"),
        env::var_os("HOME").as_deref().map(Path::new),
        env::var_os("XDG_CONFIG_HOME").as_deref().map(Path::new),
    )
    .map(|directory| directory.join("editor-pro/config.json"))
}

fn user_config_dir(macos: bool, home: Option<&Path>, xdg: Option<&Path>) -> Option<PathBuf> {
    let home = home.filter(|path| path.is_absolute());
    if macos {
        home.map(|path| path.join("Library/Application Support"))
    } else {
        xdg.filter(|path| path.is_absolute())
            .map(Path::to_path_buf)
            .or_else(|| home.map(|path| path.join(".config")))
    }
}

pub fn load(path: &Path) -> io::Result<Config> {
    let read = || -> io::Result<Config> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(error) => return Err(error),
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected a file",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration exceeds 64 KiB",
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    };
    read().map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_user_directories_never_depend_on_the_binary_or_working_directory() {
        let home = Some(Path::new("/users/test"));
        let xdg = Some(Path::new("/settings"));
        assert_eq!(
            user_config_dir(true, home, xdg),
            Some("/users/test/Library/Application Support".into())
        );
        assert_eq!(user_config_dir(false, home, xdg), Some("/settings".into()));
        for xdg in [None, Some(Path::new("")), Some(Path::new("relative"))] {
            assert_eq!(
                user_config_dir(false, home, xdg),
                Some("/users/test/.config".into())
            );
        }
        assert_eq!(user_config_dir(true, None, xdg), None);
        assert_eq!(user_config_dir(false, None, None), None);
        assert_eq!(user_config_dir(false, Some(Path::new("")), None), None);
        assert_eq!(user_config_dir(false, None, xdg), Some("/settings".into()));
    }
}
