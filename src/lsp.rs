//! LSP values at the protocol boundary. Core positions remain Unicode scalars.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use lsp_types::{Position, Range, Url};
use ropey::Rope;
use serde_json::Value;

pub const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ITEMS: usize = 4096;
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Language {
    Kotlin,
    Java,
    CSharp,
}
impl Language {
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "kt" | "kts" => Some(Self::Kotlin),
            "java" => Some(Self::Java),
            "cs" => Some(Self::CSharp),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Kotlin => "kotlin",
            Self::Java => "java",
            Self::CSharp => "csharp",
        }
    }
    pub fn environment(self) -> &'static str {
        match self {
            Self::Kotlin => "EDITOR_PRO_KOTLIN_LSP",
            Self::Java => "EDITOR_PRO_JAVA_LSP",
            Self::CSharp => "EDITOR_PRO_CSHARP_LSP",
        }
    }
}

#[derive(Clone)]
pub struct Config {
    /// An explicit argv override; no shell interpretation. Wrappers may supply SDKs.
    pub commands: BTreeMap<Language, Vec<OsString>>,
    pub root: Option<PathBuf>,
    pub initialize_timeout: Duration,
    pub request_timeout: Duration,
    pub restart_limit: usize,
    /// Only the executable resolves user settings/paths. Library defaults never download.
    pub installation: crate::lsp_install::Options,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            commands: BTreeMap::new(),
            root: None,
            initialize_timeout: Duration::from_secs(180),
            request_timeout: Duration::from_secs(15),
            restart_limit: 2,
            installation: crate::lsp_install::Options::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub buffer_id: usize,
    pub instance: u64,
    pub generation: u64,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub stamp: Stamp,
    /// Absolute disk path, or an opaque relative routing identity for virtual source.
    /// Only `uri()` may convert this identity to a protocol URI.
    pub path: PathBuf,
    pub text: Rope,
    pub saved: u64,
    pub language: Language,
    pub virtual_source: Option<VirtualSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualSource {
    /// Server declaration text when the visible bytecode fallback has a different layout.
    pub protocol_text: Option<Rope>,
    pub protocol_language: Language,
    pub uri: String,
    pub key: ServerKey,
    pub epoch: u64,
}

impl Snapshot {
    pub fn uri(&self) -> io::Result<Url> {
        match &self.virtual_source {
            Some(source) => Url::parse(&source.uri).map_err(io::Error::other),
            None => file_uri(&self.path),
        }
    }
    pub fn same_document(&self, other: &Self) -> bool {
        self.stamp == other.stamp && self.path == other.path && self.saved == other.saved
    }
}

#[derive(Clone, Debug)]
pub struct Versioned {
    pub snapshot: Snapshot,
    pub version: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServerKey {
    pub root: PathBuf,
    pub language: Language,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub serial: u64,
    pub stamp: Stamp,
    pub path: PathBuf,
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Starting,
    Indexing,
    Ready,
    Degraded,
    Failed,
}
impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Indexing => "Indexing",
            Self::Ready => "Ready",
            Self::Degraded => "Degraded",
            Self::Failed => "Failed",
        }
    }
    pub fn busy(self) -> bool {
        matches!(self, Self::Starting | Self::Indexing)
    }
}

#[derive(Clone, Debug)]
pub struct Status {
    pub phase: Phase,
    /// Worker timestamp, retained across progress reports in the same phase.
    pub since: Instant,
    pub message: String,
    pub progress: Vec<String>,
}
impl Status {
    pub fn new(phase: Phase, message: impl Into<String>) -> Self {
        Self {
            phase,
            since: Instant::now(),
            message: message.into(),
            progress: Vec::new(),
        }
    }
    pub fn summary(&self) -> String {
        if self.phase.busy() {
            format!("{} {}s", self.phase.label(), self.since.elapsed().as_secs())
        } else {
            self.phase.label().into()
        }
    }
}
impl Default for Status {
    fn default() -> Self {
        Self::new(Phase::Starting, "Discovering language server")
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Sessions {
        keys: Vec<ServerKey>,
        warm: Vec<ServerKey>,
        discovering: bool,
    },
    Installation {
        language: Language,
        state: crate::lsp_install::State,
    },
    Route {
        path: PathBuf,
        key: ServerKey,
    },
    Unavailable {
        path: PathBuf,
        message: String,
    },
    Status {
        key: ServerKey,
        epoch: u64,
        state: Status,
        capabilities: Option<Value>,
    },
    Log {
        key: ServerKey,
        message: String,
    },
    Response {
        key: ServerKey,
        epoch: u64,
        request: Request,
        documents: Vec<Versioned>,
        result: Result<Value, String>,
    },
    Diagnostics {
        key: ServerKey,
        epoch: u64,
        document: Versioned,
        dependencies: Option<BTreeMap<PathBuf, Stamp>>,
        versioned: bool,
        items: Vec<lsp_types::Diagnostic>,
    },
    Apply {
        key: ServerKey,
        epoch: u64,
        id: Value,
        request: Request,
        documents: Vec<Versioned>,
        edit: Value,
    },
    Doctor {
        path: PathBuf,
        report: String,
    },
}

pub fn file_uri(path: &Path) -> io::Result<Url> {
    Url::from_file_path(path).map_err(|_| io::Error::other("LSP requires an absolute file path"))
}
pub fn uri_path(uri: &str) -> io::Result<PathBuf> {
    let uri = Url::parse(uri).map_err(io::Error::other)?;
    if uri.scheme() != "file" || uri.query().is_some() || uri.fragment().is_some() {
        return Err(io::Error::other(
            "Only local file: LSP locations are supported",
        ));
    }
    uri.to_file_path()
        .map_err(|_| io::Error::other("Not a local file URI"))
}

pub fn position(text: &Rope, scalar: usize) -> io::Result<Position> {
    if scalar > text.len_chars() {
        return Err(io::Error::other("Position is outside the document"));
    }
    let line = text.char_to_line(scalar);
    let start = text.line_to_char(line);
    let prefix = text.slice(start..scalar);
    if scalar > start
        && text.char(scalar - 1) == '\r'
        && scalar < text.len_chars()
        && text.char(scalar) == '\n'
    {
        return Err(io::Error::other("Position is inside a line ending"));
    }
    Ok(Position::new(
        u32::try_from(line).map_err(io::Error::other)?,
        u32::try_from(prefix.chars().map(char::len_utf16).sum::<usize>())
            .map_err(io::Error::other)?,
    ))
}

pub fn scalar(text: &Rope, position: Position) -> io::Result<usize> {
    let row = position.line as usize;
    if row >= text.len_lines() {
        return Err(io::Error::other("LSP line is outside the document"));
    }
    let start = text.line_to_char(row);
    let mut units = 0;
    let mut chars = 0;
    for ch in text.line(row).chars() {
        if units == position.character {
            return Ok(start + chars);
        }
        if ch == '\n'
            || (ch == '\r'
                && start + chars + 1 < text.len_chars()
                && text.char(start + chars + 1) == '\n')
        {
            break;
        }
        units += ch.len_utf16() as u32;
        chars += 1;
        if units > position.character {
            return Err(io::Error::other(
                "LSP position splits a UTF-16 surrogate pair",
            ));
        }
    }
    if units == position.character {
        Ok(start + chars)
    } else {
        Err(io::Error::other("LSP column is outside the line"))
    }
}

pub fn scalar_range(text: &Rope, range: Range) -> io::Result<std::ops::Range<usize>> {
    let start = scalar(text, range.start)?;
    let end = scalar(text, range.end)?;
    if start > end {
        return Err(io::Error::other("Reversed LSP range"));
    }
    Ok(start..end)
}

/// A single range replacement is valid incremental synchronization. Computing it
/// from immutable Rope snapshots avoids consuming the syntax parser's edit queue.
pub fn change(old: &Rope, new: &Rope) -> io::Result<Value> {
    let mut prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .count();
    // A CRLF pair is indivisible at the protocol boundary too.
    if prefix > 0 && old.char(prefix - 1) == '\r' {
        prefix -= 1;
    }
    let mut suffix = old
        .chars_at(old.len_chars())
        .reversed()
        .zip(new.chars_at(new.len_chars()).reversed())
        .take(old.len_chars().min(new.len_chars()) - prefix)
        .take_while(|(a, b)| a == b)
        .count();
    if suffix > 0
        && old.char(old.len_chars() - suffix) == '\n'
        && old.len_chars() > suffix
        && old.char(old.len_chars() - suffix - 1) == '\r'
    {
        suffix -= 1;
    }
    Ok(serde_json::json!({
        "range": Range::new(position(old, prefix)?, position(old, old.len_chars() - suffix)?),
        "text": new.slice(prefix..new.len_chars() - suffix).to_string()
    }))
}

/// Prefer a containing build boundary to a nested module build file. Never walk
/// through a repository boundary into an unrelated parent project.
pub fn discover_root(path: &Path, language: Language) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("No file parent"))?;
    let mut nearest = None;
    for directory in parent.ancestors().take(64) {
        match language {
            Language::Kotlin | Language::Java => {
                if directory.join("pom.xml").is_file() {
                    return Ok(directory.into());
                }
                if directory.join("settings.gradle").is_file()
                    || directory.join("settings.gradle.kts").is_file()
                {
                    return Ok(directory.into());
                }
                if nearest.is_none()
                    && (directory.join("pom.xml").is_file()
                        || directory.join("build.gradle").is_file()
                        || directory.join("build.gradle.kts").is_file())
                {
                    nearest = Some(directory.to_path_buf());
                }
            }
            Language::CSharp => {
                let mut solutions = 0;
                let mut project = false;
                for (index, entry) in std::fs::read_dir(directory)?.take(4097).enumerate() {
                    if index == 4096 {
                        return Err(io::Error::other(
                            "Root discovery exceeds 4096 directory entries",
                        ));
                    }
                    let entry = entry?;
                    if !entry.file_type()?.is_file() {
                        continue;
                    }
                    match entry.path().extension().and_then(|s| s.to_str()) {
                        Some("sln" | "slnx") => solutions += 1,
                        Some("csproj") => project = true,
                        _ => {}
                    }
                }
                if solutions > 1 {
                    return Err(io::Error::other(
                        "Multiple solution files; set EDITOR_PRO_LSP_ROOT and server argv selecting the intended solution",
                    ));
                }
                if solutions == 1 {
                    return Ok(directory.into());
                }
                if project && nearest.is_none() {
                    nearest = Some(directory.to_path_buf());
                }
            }
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    Ok(nearest.unwrap_or_else(|| parent.into()))
}

pub fn supports(capabilities: &Value, method: &str) -> bool {
    let key = match method {
        "textDocument/completion" => "completionProvider",
        "completionItem/resolve" => {
            return capabilities["completionProvider"]["resolveProvider"] == true;
        }
        "textDocument/hover" => "hoverProvider",
        "textDocument/signatureHelp" => "signatureHelpProvider",
        "textDocument/definition" => "definitionProvider",
        "textDocument/references" => "referencesProvider",
        "textDocument/rename" => "renameProvider",
        "textDocument/codeAction" => "codeActionProvider",
        "codeAction/resolve" => {
            return capabilities["codeActionProvider"]["resolveProvider"] == true;
        }
        "textDocument/formatting" => "documentFormattingProvider",
        "textDocument/documentSymbol" => "documentSymbolProvider",
        "workspace/symbol" => "workspaceSymbolProvider",
        "workspaceSymbol/resolve" => {
            return capabilities["workspaceSymbolProvider"]["resolveProvider"] == true;
        }
        "textDocument/prepareCallHierarchy"
        | "callHierarchy/incomingCalls"
        | "callHierarchy/outgoingCalls" => "callHierarchyProvider",
        "textDocument/diagnostic" => "diagnosticProvider",
        "workspace/executeCommand" => "executeCommandProvider",
        _ => return false,
    };
    capabilities[key] == true || capabilities[key].is_object()
}

pub fn is_csharp_metadata_uri(uri: &str) -> bool {
    uri.len() <= 8192
        && !uri.chars().any(char::is_control)
        && Url::parse(uri).is_ok_and(|uri| uri.scheme() == "csharp")
}

/// csharp-ls negotiates metadata URIs through a client extension, rather than a
/// standard server capability. Keep that exception scoped to C# and its URIs.
pub fn supports_request(
    capabilities: &Value,
    language: Language,
    method: &str,
    params: &Value,
) -> bool {
    if method == "csharp/metadata" {
        language == Language::CSharp
            && params["textDocument"]["uri"]
                .as_str()
                .is_some_and(is_csharp_metadata_uri)
    } else if method == "java/classFileContents" {
        language == Language::Java
            && params["uri"]
                .as_str()
                .is_some_and(|uri| crate::lsp_source::supports_uri(language, uri))
    } else {
        supports(capabilities, method)
    }
}
