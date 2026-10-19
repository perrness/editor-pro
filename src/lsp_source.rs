//! Server-owned source retrieval. URI schemes and extension payloads stay here,
//! outside the editor, file I/O and ordinary LSP navigation.
use crate::lsp::{Language, MAX_DOCUMENT_BYTES, Request};
use lsp_types::Url;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Original,
    Decompiled,
    Generated,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Dependency source",
            Self::Decompiled => "Decompiled source",
            Self::Generated => "Generated/decompiled source",
        }
    }
}

pub struct Content<'a> {
    pub text: &'a str,
    pub syntax_path: &'static Path,
    pub kind: SourceKind,
    pub protocol_text: Option<&'a str>,
    pub protocol_language: Language,
}

pub fn supports_uri(language: Language, uri: &str) -> bool {
    if uri.len() > 8192 || uri.chars().any(char::is_control) {
        return false;
    }
    let Ok(parsed) = Url::parse(uri) else {
        return false;
    };
    match language {
        Language::CSharp => parsed.scheme() == "csharp",
        Language::Java => {
            parsed.scheme() == "jdt"
                && parsed.host_str() == Some("contents")
                && parsed.query().is_some_and(|q| !q.is_empty())
        }
        Language::Kotlin => matches!(parsed.scheme(), "jar" | "jrt"),
    }
}

pub fn request(language: Language, uri: &str) -> io::Result<(&'static str, Value)> {
    if !supports_uri(language, uri) {
        return Err(io::Error::other("Unsupported virtual source URI"));
    }
    Ok(match language {
        Language::CSharp => ("csharp/metadata", json!({"textDocument":{"uri":uri}})),
        // JDT negotiates this through extendedClientCapabilities, not a standard
        // server capability. Older/other servers can reject it normally.
        Language::Java => ("java/classFileContents", json!({"uri":uri})),
        // JetBrains advertises this command; it returns attached source as well
        // as decompiled PSI text. It must never authorize workspace/applyEdit.
        Language::Kotlin => (
            "workspace/executeCommand",
            json!({"command":"decompile","arguments":[uri]}),
        ),
    })
}

pub fn request_uri(language: Language, request: &Request) -> Option<&str> {
    let uri = match (language, request.method.as_str()) {
        (Language::Java, "java/classFileContents") => request.params["uri"].as_str()?,
        (Language::Kotlin, "workspace/executeCommand")
            if request.params["command"] == "decompile"
                && request.params["arguments"].as_array()?.len() == 1 =>
        {
            request.params["arguments"][0].as_str()?
        }
        _ => return None,
    };
    supports_uri(language, uri).then_some(uri)
}

pub fn content<'a>(language: Language, uri: &str, result: &'a Value) -> io::Result<Content<'a>> {
    let text = match language {
        Language::CSharp => result["source"].as_str(),
        Language::Java => result.as_str(),
        Language::Kotlin => result["code"].as_str(),
    }
    .filter(|text| !text.trim().is_empty())
    .ok_or_else(|| {
        io::Error::other(
            "Language server returned no source text for this dependency/generated location",
        )
    })?;
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(io::Error::other("Virtual source exceeds 2 MiB"));
    }
    if text.contains('\0') {
        return Err(io::Error::other("Virtual source contains NUL bytes"));
    }
    let (syntax_path, kind) = match language {
        Language::CSharp => ("source.cs", SourceKind::Generated),
        Language::Java => (
            "source.java",
            if text.starts_with("// Source code is decompiled from a .class file") {
                SourceKind::Decompiled
            } else {
                SourceKind::Original
            },
        ),
        Language::Kotlin => {
            let syntax = match result["language"].as_str() {
                Some("kotlin") => "source.kt",
                Some("java") => "source.java",
                _ => return Err(io::Error::other("Unsupported dependency source language")),
            };
            let path = Url::parse(uri).map_err(io::Error::other)?;
            let original = path.path().ends_with(".kt")
                || path.path().ends_with(".java")
                || path.path().ends_with(".kts");
            (
                syntax,
                if original {
                    SourceKind::Original
                } else {
                    SourceKind::Decompiled
                },
            )
        }
    };
    let protocol_text = result.get("editorProProtocolText").and_then(Value::as_str);
    if protocol_text.is_some_and(|text| text.len() > MAX_DOCUMENT_BYTES || text.contains('\0')) {
        return Err(io::Error::other("Invalid server declaration text"));
    }
    let protocol_language = if language == Language::Kotlin {
        match result
            .get("editorProProtocolLanguage")
            .unwrap_or(&result["language"])
            .as_str()
        {
            Some("java") => Language::Java,
            Some("kotlin") => Language::Kotlin,
            _ => return Err(io::Error::other("Unsupported server declaration language")),
        }
    } else {
        language
    };
    Ok(Content {
        text,
        syntax_path: Path::new(syntax_path),
        kind,
        protocol_text,
        protocol_language,
    })
}

// Cache only documents with a locally observable backing archive/runtime. Unknown
// JDT handles are fetched again rather than assigned an invented disk identity.
// This is freshness metadata, never a dependency resolver or archive extractor.
fn backing_path(uri: &str) -> Option<PathBuf> {
    let parsed = Url::parse(uri).ok()?;
    let path = match parsed.scheme() {
        "jar" | "jrt" => {
            let inner = uri.split_once(':')?.1.split_once("!/")?.0;
            let file = if inner.starts_with("file:") {
                Url::parse(inner).ok()?
            } else {
                Url::parse(&format!("file:{inner}")).ok()?
            };
            file.to_file_path().ok()?
        }
        "jdt" => {
            // JDT's query is a percent-encoded Java element handle. Decode via
            // URL's file-path conversion, without form-urlencoded '+' handling.
            let query = parsed.query()?;
            let decoded = Url::parse(&format!("file:///{query}"))
                .ok()?
                .to_file_path()
                .ok()?;
            let handle = decoded.to_str()?.strip_prefix('/')?;
            let (_, root) = handle.split_once('/')?;
            let mut chars = root.chars();
            let mut path = String::new();
            while let Some(ch) = chars.next() {
                match ch {
                    '\\' => path.push(chars.next()?),
                    '<' | '`' | '&' | '=' => break,
                    _ => path.push(ch),
                }
            }
            PathBuf::from(path)
        }
        _ => return None,
    };
    path.is_absolute().then_some(path)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    binary: Vec<Option<FileStamp>>,
    related: Vec<Option<FileStamp>>,
}

impl Fingerprint {
    pub fn same_binary(&self, other: Option<&Self>) -> bool {
        other.is_some_and(|other| self.binary == other.binary)
    }
}

fn stamp(path: &Path) -> io::Result<Option<FileStamp>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(FileStamp {
        path: std::fs::canonicalize(path)?,
        size: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
        },
    }))
}

pub(crate) async fn fingerprint(uri: &str) -> Option<Fingerprint> {
    let path = backing_path(uri)?;
    tokio::task::spawn_blocking(move || {
        let mut binary = vec![path.clone()];
        let mut related = Vec::new();
        if path.is_dir() {
            binary.extend([path.join("release"), path.join("lib/modules")]);
            related.push(path.join("lib/src.zip"));
        } else {
            let parent = path.parent()?;
            related.push(parent.to_path_buf());
            if path.file_name()? == "jrt-fs.jar" {
                binary.push(parent.join("modules"));
                related.push(parent.join("src.zip"));
            } else if path.extension().is_some_and(|s| s == "jar") {
                related.push(
                    path.with_file_name(format!("{}-sources.jar", path.file_stem()?.to_str()?)),
                );
            }
        }
        let stamps = |paths: Vec<PathBuf>| {
            paths
                .iter()
                .map(|p| stamp(p))
                .collect::<io::Result<Vec<_>>>()
                .ok()
        };
        let binary = stamps(binary)?;
        binary.first()?.as_ref()?;
        Some(Fingerprint {
            binary,
            related: stamps(related)?,
        })
    })
    .await
    .ok()
    .flatten()
}

struct Entry {
    uri: String,
    value: Value,
    fingerprint: Fingerprint,
    inserted: Instant,
    bytes: usize,
}

#[derive(Default)]
pub(crate) struct Cache(VecDeque<Entry>);

impl Cache {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn get(&mut self, uri: &str, fingerprint: Option<&Fingerprint>) -> Option<Value> {
        let index = self.0.iter().position(|entry| entry.uri == uri)?;
        let entry = self.0.remove(index)?;
        if Some(&entry.fingerprint) != fingerprint
            || entry.inserted.elapsed() >= Duration::from_secs(30)
        {
            return None;
        }
        let value = entry.value.clone();
        self.0.push_back(entry);
        Some(value)
    }

    pub fn insert(
        &mut self,
        language: Language,
        uri: &str,
        value: &Value,
        fingerprint: Option<Fingerprint>,
    ) {
        let Some(fingerprint) = fingerprint else {
            return;
        };
        let Ok(content) = content(language, uri, value) else {
            return;
        };
        // Arbitrary JDT source attachments are opaque to the client. Let JDT
        // revalidate original source; decompilation is the expensive fallback.
        if language == Language::Java && content.kind == SourceKind::Original {
            return;
        }
        self.0.retain(|entry| entry.uri != uri);
        let bytes = content.text.len() + content.protocol_text.map_or(0, str::len);
        while self.0.len() >= 32
            || self.0.iter().map(|e| e.bytes).sum::<usize>() + bytes > 16 * 1024 * 1024
        {
            self.0.pop_front();
        }
        self.0.push_back(Entry {
            uri: uri.into(),
            value: match language {
                Language::Kotlin => json!({"code": content.text, "language": value["language"], "editorProProtocolText": content.protocol_text, "editorProProtocolLanguage": content.protocol_language.id()}),
                _ => Value::String(content.text.into()),
            },
            fingerprint,
            inserted: Instant::now(),
            bytes,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_extensions_preserve_opaque_uris_and_stay_language_scoped() {
        let jdt =
            "jdt://contents/dep.jar/pkg/Thing.class?=project/%5C/tmp%5C/dep+1.jar<pkg(Thing.class";
        let (method, params) = request(Language::Java, jdt).unwrap();
        assert_eq!(method, "java/classFileContents");
        assert_eq!(params["uri"], jdt);
        assert_eq!(backing_path(jdt), Some(PathBuf::from("/tmp/dep+1.jar")));
        let with_attributes = jdt.replace("<pkg", "=/maven.pomderived=/true=/%3Cpkg");
        assert_eq!(
            backing_path(&with_attributes),
            Some(PathBuf::from("/tmp/dep+1.jar"))
        );
        assert!(!supports_uri(Language::Kotlin, jdt));
        assert!(!supports_uri(
            Language::Java,
            "jdt://other/Thing.class?handle"
        ));
        assert!(!supports_uri(Language::Java, "jdt://contents/Thing.class"));
        for uri in [
            "file:///tmp/Thing.java",
            "https://example.com/Thing.java",
            "jar:file:///tmp/a.jar!/Bad\n.class",
        ] {
            assert!(!supports_uri(Language::Java, uri));
            assert!(!supports_uri(Language::Kotlin, uri));
        }
        for uri in [
            "jar:file:///tmp/a%20%2B%20b.jar!/p/Thing.kt",
            "jar:///tmp/a%20%2B%20b.jar!/p/Thing.class",
        ] {
            assert_eq!(backing_path(uri), Some(PathBuf::from("/tmp/a + b.jar")));
            let (_, params) = request(Language::Kotlin, uri).unwrap();
            assert_eq!(params["arguments"], json!([uri]));
        }
        assert!(backing_path("jar:file://remote/tmp/a.jar!/X.kt").is_none());
        assert!(backing_path("jdt://contents/a.jar/p/X.class?=project/a.jar<p(X.class").is_none());
    }

    #[test]
    fn source_cache_bounds_expiry_and_unobservable_dependencies() {
        let mut cache = Cache::default();
        let fingerprint = Fingerprint {
            binary: Vec::new(),
            related: Vec::new(),
        };
        let value = json!({"code":"class Dependency", "language":"kotlin"});
        let uri = |i| format!("jar:file:///dep{i}.jar!/Dependency.kt");
        for i in 0..33 {
            cache.insert(Language::Kotlin, &uri(i), &value, Some(fingerprint.clone()));
        }
        assert_eq!(cache.0.len(), 32);
        assert!(cache.get(&uri(0), Some(&fingerprint)).is_none());
        assert!(cache.get(&uri(1), Some(&fingerprint)).is_some());
        assert!(cache.get(&uri(1), None).is_none());
        cache.0.back_mut().unwrap().inserted = Instant::now() - Duration::from_secs(31);
        assert!(cache.get(&uri(32), Some(&fingerprint)).is_none());
        cache.insert(Language::Kotlin, &uri(100), &value, None);
        assert!(cache.get(&uri(100), Some(&fingerprint)).is_none());
        cache.clear();
        let large = json!({"code":"x".repeat(MAX_DOCUMENT_BYTES), "language":"java"});
        for i in 0..12 {
            cache.insert(Language::Kotlin, &uri(i), &large, Some(fingerprint.clone()));
        }
        assert_eq!(cache.0.len(), 8);
        assert!(cache.get(&uri(3), Some(&fingerprint)).is_none());
    }
}

/// Replace declaration stubs only; original source and a server's full decompiled
/// text always win. Strip unrecognized server fields before adding client metadata.
pub(crate) async fn prepare(
    config: &crate::lsp::Config,
    uri: &str,
    value: Value,
    limit: Duration,
) -> io::Result<Value> {
    let parsed = content(Language::Kotlin, uri, &value)?;
    if parsed.kind != SourceKind::Decompiled || !parsed.text.contains("compiled code") {
        return Ok(json!({"code":parsed.text, "language":value["language"]}));
    }
    if !Url::parse(uri).is_ok_and(|uri| uri.path().ends_with(".class")) {
        return Err(io::Error::other(
            "Language server returned declarations without a JVM class file for this location",
        ));
    }
    let text = crate::lsp_install::decompile(config, uri, limit).await?;
    Ok(
        json!({"code":text, "language":"java", "editorProProtocolText":parsed.text, "editorProProtocolLanguage":value["language"]}),
    )
}

// A full decompiler and the server's declaration view have different coordinates.
// Match identifier tokens and nearby declaration context, excluding literals and
// comments. Ambiguous/unrepresented positions fail rather than sending an offset
// for a different symbol. This supports declarations/types; method-body symbols
// absent from the server's view cannot be resolved by that server.
struct Token<'a> {
    bytes: std::ops::Range<usize>,
    text: &'a str,
}
fn tokens(text: &str) -> io::Result<Vec<Token<'_>>> {
    let mut result = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, c)| *c == '/') {
            chars.next();
            for (_, ch) in chars.by_ref() {
                if ch == '\n' {
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, c)| *c == '*') {
            chars.next();
            let mut previous = ' ';
            for (_, ch) in chars.by_ref() {
                if previous == '*' && ch == '/' {
                    break;
                }
                previous = ch;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            while let Some((_, current)) = chars.next() {
                if current == '\\' {
                    chars.next();
                } else if current == ch {
                    break;
                }
            }
            continue;
        }
        if ch.is_alphanumeric() || ch == '_' || ch == '$' {
            while chars
                .peek()
                .is_some_and(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '$')
            {
                chars.next();
            }
        }
        let end = chars.peek().map_or(text.len(), |(byte, _)| *byte);
        result.push(Token {
            bytes: start..end,
            text: &text[start..end],
        });
        if result.len() > 65536 {
            return Err(io::Error::other(
                "Decompiled navigation token limit exceeded",
            ));
        }
    }
    Ok(result)
}

pub(crate) fn map_position(
    from: &ropey::Rope,
    to: &ropey::Rope,
    position: lsp_types::Position,
) -> io::Result<lsp_types::Position> {
    let scalar = crate::lsp::scalar(from, position)?;
    if scalar == 0 {
        return Ok(lsp_types::Position::new(0, 0));
    }
    let byte = from.char_to_byte(scalar);
    let from_text = from.to_string();
    let to_text = to.to_string();
    let source = tokens(&from_text)?;
    let target = tokens(&to_text)?;
    let missing = || {
        io::Error::other(
            "Language server cannot resolve this decompiled position; use a declaration or return to project source",
        )
    };
    let index = source
        .iter()
        .position(|token| token.bytes.contains(&byte))
        .ok_or_else(missing)?;
    let token = &source[index];
    if !token
        .text
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_' || ch == '$')
    {
        return Err(missing());
    }
    let candidates: Vec<_> = target
        .iter()
        .enumerate()
        .filter(|(_, t)| t.text == token.text)
        .collect();
    let mut best = None;
    let mut best_score = 0;
    let mut tied = false;
    for (candidate, _) in &candidates {
        let mut score = 0;
        for distance in 1..=4 {
            for (a, b) in [
                (index.checked_sub(distance), candidate.checked_sub(distance)),
                (index.checked_add(distance), candidate.checked_add(distance)),
            ] {
                if let (Some(a), Some(b)) =
                    (a.and_then(|i| source.get(i)), b.and_then(|i| target.get(i)))
                    && a.text == b.text
                {
                    score += 5 - distance;
                }
            }
        }
        if best.is_none() || score > best_score {
            best = Some(*candidate);
            best_score = score;
            tied = false;
        } else if score == best_score {
            tied = true;
        }
    }
    let best = best.ok_or_else(missing)?;
    let unique =
        candidates.len() == 1 && source.iter().filter(|t| t.text == token.text).count() == 1;
    if tied || (!unique && best_score < 4) {
        return Err(missing());
    }
    let offset = from_text[token.bytes.start..byte].chars().count();
    crate::lsp::position(to, to.byte_to_char(target[best].bytes.start) + offset)
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use ropey::Rope;

    fn at(text: &Rope, word: &str) -> lsp_types::Position {
        let bytes = text.to_string().find(word).unwrap();
        crate::lsp::position(text, text.byte_to_char(bytes)).unwrap()
    }

    #[tokio::test]
    async fn kotlin_builtin_declarations_without_bytecode_fail_before_running_a_decompiler() {
        let error = prepare(
            &crate::lsp::Config::default(),
            "jar:///tmp/stdlib.jar!/kotlin/kotlin.kotlin_builtins",
            json!({"code":"class String { /* compiled code */ }", "language":"kotlin"}),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("without a JVM class file"));
    }

    #[test]
    fn full_bytecode_positions_map_declarations_without_using_comment_or_literal_names() {
        let server = Rope::from_str(
            "// 😀\r\nclass Library {\r\n fun message(): String { /* compiled code */ }\r\n}\r\n",
        );
        let visible = Rope::from_str(
            "// Library message\n@Metadata(d2={\"message\",\"Library\"})\npublic final class Library {\n public String message() { return \"actual body\"; }\n}\n",
        );
        for name in ["Library", "message", "String"] {
            let source_position = at(&server, name);
            let mapped = map_position(&server, &visible, source_position).unwrap();
            let scalar = crate::lsp::scalar(&visible, mapped).unwrap();
            assert!(visible.slice(scalar..).to_string().starts_with(name));
            assert!(mapped.line >= 2);
            assert_eq!(
                map_position(&visible, &server, mapped).unwrap(),
                source_position
            );
        }
        assert!(map_position(&visible, &server, at(&visible, "actual body")).is_err());
        assert!(map_position(&server, &visible, lsp_types::Position::new(999, 0)).is_err());
    }
}
