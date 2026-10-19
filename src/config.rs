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
    pub lsp: LspSettings,
    pub editor: EditorSettings,
    pub mouse: MouseSettings,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseSettings {
    pub enabled: bool,
    pub hover_enabled: bool,
    pub hover_delay_ms: u64,
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            hover_enabled: true,
            hover_delay_ms: 400,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorSettings {
    pub relative_line_numbers: bool,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LspSettings {
    pub auto_install: bool,
    pub warmup: bool,
}
impl Default for LspSettings {
    fn default() -> Self {
        Self {
            auto_install: true,
            warmup: true,
        }
    }
}

pub fn lsp_directory() -> Option<PathBuf> {
    user_data_dir(
        cfg!(target_os = "macos"),
        env::var_os("HOME").as_deref().map(Path::new),
        env::var_os("XDG_DATA_HOME").as_deref().map(Path::new),
    )
    .map(|directory| directory.join("editor-pro/lsp"))
}

fn user_data_dir(macos: bool, home: Option<&Path>, xdg: Option<&Path>) -> Option<PathBuf> {
    let home = home.filter(|path| path.is_absolute());
    if macos {
        home.map(|path| path.join("Library/Application Support"))
    } else {
        xdg.filter(|path| path.is_absolute())
            .map(Path::to_path_buf)
            .or_else(|| home.map(|path| path.join(".local/share")))
    }
}

pub fn config_path() -> Option<PathBuf> {
    user_config_dir(
        env::var_os("HOME").as_deref().map(Path::new),
        env::var_os("XDG_CONFIG_HOME").as_deref().map(Path::new),
    )
    .map(|directory| directory.join("editor-pro/config.json"))
}

fn user_config_dir(home: Option<&Path>, xdg: Option<&Path>) -> Option<PathBuf> {
    let home = home.filter(|path| path.is_absolute());
    xdg.filter(|path| path.is_absolute())
        .map(Path::to_path_buf)
        .or_else(|| home.map(|path| path.join(".config")))
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
    fn managed_servers_use_native_data_directories_with_absolute_fallbacks() {
        let home = Some(Path::new("/users/test"));
        let xdg = Some(Path::new("/data"));
        assert_eq!(
            user_data_dir(true, home, xdg),
            Some("/users/test/Library/Application Support".into())
        );
        assert_eq!(user_data_dir(false, home, xdg), Some("/data".into()));
        for xdg in [None, Some(Path::new("")), Some(Path::new("relative"))] {
            assert_eq!(
                user_data_dir(false, home, xdg),
                Some("/users/test/.local/share".into())
            );
        }
        assert_eq!(user_data_dir(true, None, xdg), None);
        assert_eq!(user_data_dir(false, None, xdg), Some("/data".into()));
        assert_eq!(
            user_data_dir(false, Some(Path::new("relative")), None),
            None
        );
    }

    #[test]
    fn config_uses_xdg_or_dot_config_with_absolute_directories_only() {
        let home = Some(Path::new("/users/test"));
        let xdg = Some(Path::new("/settings"));
        assert_eq!(user_config_dir(home, xdg), Some("/settings".into()));
        for xdg in [None, Some(Path::new("")), Some(Path::new("relative"))] {
            assert_eq!(
                user_config_dir(home, xdg),
                Some("/users/test/.config".into())
            );
        }
        for home in [None, Some(Path::new("")), Some(Path::new("relative"))] {
            assert_eq!(user_config_dir(home, None), None);
            assert_eq!(user_config_dir(home, xdg), Some("/settings".into()));
        }
    }
}
