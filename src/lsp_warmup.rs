//! Conservative, bounded hints for the existing LSP manager. This runs on a
//! blocking worker, never on the terminal or protocol runtime thread.
use super::project_root;
use crate::lsp::{Config, Language, ServerKey};
use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) const MAX_WARM_SESSIONS: usize = 2;
const MAX_ENTRIES: usize = 4096;
const MAX_DIRECTORIES: usize = 64;
const MAX_MARKER_BYTES: usize = 64 * 1024;

#[cfg(test)]
#[path = "lsp_warmup_tests.rs"]
mod tests;

struct Scan<'a> {
    cancelled: &'a AtomicBool,
    started: Instant,
    entries: usize,
    directories: usize,
    bytes: usize,
}

impl Scan<'_> {
    fn active(&self) -> bool {
        !self.cancelled.load(Ordering::Relaxed)
            && self.started.elapsed() < Duration::from_secs(1)
            && self.entries <= MAX_ENTRIES
            && self.directories <= MAX_DIRECTORIES
            && self.bytes <= 512 * 1024
    }

    fn entries(&mut self, directory: &Path) -> Vec<(PathBuf, fs::FileType)> {
        self.directories += 1;
        let mut result = Vec::new();
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries {
                self.entries += 1;
                if !self.active() {
                    break;
                }
                if let Ok(entry) = entry
                    && let Ok(kind) = entry.file_type()
                {
                    result.push((entry.path(), kind));
                }
            }
        }
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    fn marker(&mut self, path: &Path) -> String {
        if !self.active() {
            return String::new();
        }
        let mut bytes = Vec::new();
        if let Ok(file) = fs::File::open(path)
            && file
                .take((MAX_MARKER_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .is_ok()
        {
            self.bytes += bytes.len();
            if bytes.len() <= MAX_MARKER_BYTES {
                return String::from_utf8_lossy(&bytes).into_owned();
            }
        }
        String::new()
    }
}

fn build_marker(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(
            "pom.xml"
                | "settings.gradle"
                | "settings.gradle.kts"
                | "build.gradle"
                | "build.gradle.kts"
        )
    ) || matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("sln" | "slnx" | "csproj")
    )
}

fn source_directory(directory: &Path, language: &str) -> bool {
    ["src/main", "src/test", "src"].iter().any(|source| {
        fs::symlink_metadata(directory.join(source).join(language))
            .is_ok_and(|metadata| metadata.is_dir())
    })
}

pub(super) fn discover(
    config: &Config,
    directory: &Path,
    cancelled: &AtomicBool,
) -> Vec<ServerKey> {
    let mut scan = Scan {
        cancelled,
        started: Instant::now(),
        entries: 0,
        directories: 0,
        bytes: 0,
    };
    if !scan.active() {
        return Vec::new();
    }
    let Ok(directory) = directory.canonicalize() else {
        return Vec::new();
    };
    if !directory.is_dir() {
        return Vec::new();
    }
    // Worktree .git files count too. Never inspect an enclosing repository when
    // launched inside a nested one. Without Git, use the nearest build marker.
    let repository = directory
        .ancestors()
        .take(64)
        .find(|ancestor| scan.active() && ancestor.join(".git").exists());
    let root = if let Some(repository) = repository {
        repository.to_path_buf()
    } else {
        directory
            .ancestors()
            .take(64)
            .find(|ancestor| {
                scan.active()
                    && scan
                        .entries(ancestor)
                        .iter()
                        .any(|(path, kind)| kind.is_file() && build_marker(path))
            })
            .unwrap_or(&directory)
            .to_path_buf()
    };
    let mut queue = VecDeque::from([(root, 0)]);
    let mut keys = BTreeSet::new();
    while let Some((directory, depth)) = queue.pop_front() {
        if !scan.active() {
            return Vec::new();
        }
        let entries = scan.entries(&directory);
        let mut kotlin = source_directory(&directory, "kotlin");
        let mut java = source_directory(&directory, "java");
        let mut jvm_build = false;
        let mut csharp = false;
        for (path, kind) in &entries {
            if kind.is_dir() && depth < 2 {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if !name.starts_with('.')
                    && !matches!(
                        name,
                        "build" | "target" | "bin" | "obj" | "node_modules" | "vendor" | "src"
                    )
                    && !path.join(".git").exists()
                {
                    queue.push_back((path.clone(), depth + 1));
                }
            }
            if !kind.is_file() {
                continue;
            }
            match path.extension().and_then(|s| s.to_str()) {
                Some("kt") => kotlin = true,
                Some("java") => java = true,
                Some("sln" | "slnx" | "csproj") => csharp = true,
                _ => {}
            }
            if matches!(
                path.file_name().and_then(|s| s.to_str()),
                Some(
                    "pom.xml"
                        | "build.gradle"
                        | "build.gradle.kts"
                        | "settings.gradle"
                        | "settings.gradle.kts"
                )
            ) {
                jvm_build = true;
                let text = scan.marker(path);
                let kotlin_plugin = text.contains("kotlin-maven-plugin")
                    || text.contains("org.jetbrains.kotlin.")
                    || text.contains("kotlin(\"jvm\")")
                    || text.contains("kotlin('jvm')");
                kotlin |= kotlin_plugin;
                // A Kotlin DSL build file is not Kotlin source. Maven defaults
                // to Java, but aggregator POMs and Kotlin-only builds do not.
                if path.file_name().is_some_and(|n| n == "pom.xml") {
                    java |= !kotlin
                        && !text
                            .split_whitespace()
                            .collect::<String>()
                            .contains("<packaging>pom</packaging>");
                } else {
                    java |= text
                        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                        .any(|token| matches!(token, "java" | "java-library"));
                }
            }
        }
        for (language, detected, extension) in [
            (Language::Kotlin, jvm_build && kotlin, "kt"),
            (Language::Java, jvm_build && java, "java"),
            (Language::CSharp, csharp, "cs"),
        ] {
            if detected
                && scan.active()
                && let Ok(root) = project_root(
                    config,
                    &directory.join(format!(".editor-pro-warmup.{extension}")),
                    language,
                )
            {
                keys.insert(ServerKey { root, language });
            }
        }
        // Broad polyglot/monorepo discovery is deliberately demand-driven.
        if keys.len() > MAX_WARM_SESSIONS {
            return Vec::new();
        }
    }
    if scan.active() {
        keys.into_iter().collect()
    } else {
        Vec::new()
    }
}
