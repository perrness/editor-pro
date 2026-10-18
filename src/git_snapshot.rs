//! Snapshot collection and comparisons run inside the existing Git/review workers.
use super::*;
use crate::{
    review::{ContentId, Identity},
    review_snapshot::{self, Baseline, Capture, File as CapturedFile, History},
};
use std::collections::{BTreeMap, BTreeSet};

fn history<'a>(request: &'a Request, repo: &Repository) -> Result<&'a History, String> {
    request
        .review
        .as_ref()
        .filter(|s| s.repository == *repo)
        .and_then(|s| s.history.as_ref())
        .ok_or_else(|| "No review snapshot for this worktree; use :review snapshot".into())
}
fn buffer_for<'a>(request: &'a Request, path: &Path) -> Option<&'a BufferVersion> {
    if request.path == path && request.buffer.is_some() {
        request.buffer.as_ref()
    } else {
        request
            .review_buffers
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b)
    }
}
#[derive(PartialEq, Eq)]
struct LiveTree {
    files: Vec<CapturedFile>,
    disk: Vec<(PathBuf, ContentId, String)>,
    external: Vec<PathBuf>,
}
fn live_tree(
    runner: &Runner<'_>,
    request: &Request,
    repo: &Repository,
) -> Result<LiveTree, String> {
    let names = runner.git(
        &repo.root,
        &refs(&args(&[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])),
    )?;
    let mut paths = BTreeSet::new();
    for name in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        paths.insert(path(name)?);
    }
    if let Ok(history) = history(request, repo) {
        paths.extend(history.baseline.files.iter().map(|f| f.path.clone()));
        paths.extend(
            history
                .checkpoints
                .iter()
                .flat_map(|c| c.files.iter().map(|f| f.path.clone())),
        );
    }
    for (path, buffer) in &request.review_buffers {
        if buffer.dirty
            && let Ok(relative) = path.strip_prefix(&repo.root)
        {
            paths.insert(relative.to_path_buf());
        }
    }
    if request.buffer.as_ref().is_some_and(|b| b.dirty) {
        paths.insert(
            request
                .path
                .strip_prefix(&repo.root)
                .map_err(err)?
                .to_path_buf(),
        );
    }
    if paths.len() > review_snapshot::MAX_FILES {
        return Err("Review tree exceeds 512 paths (including deleted baseline paths)".into());
    }
    let mut tree = LiveTree {
        files: Vec::new(),
        disk: Vec::new(),
        external: Vec::new(),
    };
    let mut bytes = 0;
    for relative in paths {
        runner.check()?;
        if !review_snapshot::valid_path(&relative) {
            return Err("Unsupported path in review snapshot".into());
        }
        // Reject symlinked parents too: captures must stay inside this worktree.
        let mut absolute = repo.root.clone();
        for component in relative.components() {
            absolute.push(component);
            match fs::symlink_metadata(&absolute) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!(
                        "Snapshot cannot include symlink {}",
                        display_path(&relative)
                    ));
                }
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(err(e)),
                _ => {}
            }
        }
        let disk_request = Request {
            path: absolute.clone(),
            buffer: None,
            comparison: Comparison::Head,
            inspect: true,
            review: None,
            review_generation: 0,
            review_buffers: Vec::new(),
        };
        let (disk, _, present) =
            working(&disk_request).map_err(|e| format!("{}: {e}", display_path(&relative)))?;
        let mode = working_mode(&absolute);
        let buffer = buffer_for(request, &absolute);
        if let Some(buffer) = buffer
            && (!present || buffer.text.bytes().ne(disk.iter().copied()))
        {
            tree.external.push(relative.clone());
        }
        tree.disk.push((
            relative.clone(),
            ContentId::new(&disk, present),
            mode.clone(),
        ));
        let dirty = buffer.is_some_and(|b| b.dirty);
        let content = if dirty {
            let buffer = buffer.unwrap();
            if buffer.text.len_bytes() > CONTENT_LIMIT {
                return Err("Snapshot buffer exceeds 2 MiB".into());
            }
            buffer.text.to_string().into_bytes()
        } else if present {
            disk
        } else {
            continue;
        };
        bytes += content.len();
        if bytes > review_snapshot::TREE_LIMIT {
            return Err("Review tree exceeds 8 MiB".into());
        }
        tree.files
            .push(CapturedFile::new(relative, content, mode, None, dirty));
    }
    Ok(tree)
}
fn resolve(
    runner: &Runner<'_>,
    repo: &Repository,
    comparison: &Comparison,
) -> Result<Option<String>, String> {
    let value = match comparison {
        Comparison::Head => "HEAD",
        Comparison::Revision(value) => value,
        Comparison::Index => return Ok(None),
        _ => return Err("Start snapshots from HEAD, index or a selected revision; staged comparisons are not supported".into()),
    };
    let result = runner.run(
        &repo.root,
        &refs(&args(&[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{value}^{{commit}}"),
        ])),
    )?;
    if !result.success {
        if *comparison == Comparison::Head {
            // Only an unborn branch is an empty baseline, not arbitrary Git failure.
            let head = runner.run(&repo.root, &refs(&args(&["symbolic-ref", "-q", "HEAD"])))?;
            if head.success {
                let exists = runner.run(
                    &repo.root,
                    &refs(&args(&[
                        "show-ref",
                        "--verify",
                        "--quiet",
                        std::str::from_utf8(line(&head.bytes)).map_err(err)?,
                    ])),
                )?;
                if exists.code == Some(1) {
                    return Ok(None);
                }
            }
        }
        return Err(format!("Cannot resolve snapshot base: {}", result.error));
    }
    Ok(Some(
        std::str::from_utf8(line(&result.bytes))
            .map_err(err)?
            .into(),
    ))
}
fn baseline(
    runner: &Runner<'_>,
    repo: &Repository,
    comparison: &Comparison,
) -> Result<Baseline, String> {
    let resolved_base = resolve(runner, repo, comparison)?;
    let names = if *comparison == Comparison::Index {
        runner.git(&repo.root, &refs(&args(&["ls-files", "--stage", "-z"])))?
    } else if let Some(oid) = &resolved_base {
        runner.git(&repo.root, &refs(&args(&["ls-tree", "-r", "-z", oid])))?
    } else {
        Vec::new()
    };
    let mut entries = Vec::new();
    for record in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        runner.check()?;
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("Invalid Git tree entry")?;
        let path = path(&record[tab + 1..])?;
        if !review_snapshot::valid_path(&path) {
            return Err("Unsupported snapshot path".into());
        }
        if entries.len() == review_snapshot::MAX_FILES {
            return Err("Review baseline exceeds 512 files".into());
        }
        let fields: Vec<_> = record[..tab].split(|b| *b == b' ').collect();
        if fields.len() != 3 {
            return Err("Malformed Git tree entry".into());
        }
        if !matches!(fields[0], b"100644" | b"100755") {
            return Err("Unsupported Git content: symlink, tree or submodule".into());
        }
        let oid = if *comparison == Comparison::Index {
            if fields[2] != b"0" {
                return Err("Unresolved conflict in review baseline".into());
            }
            fields[1]
        } else {
            if fields[1] != b"blob" {
                return Err("Unsupported Git object in review baseline".into());
            }
            fields[2]
        };
        if !matches!(oid.len(), 40 | 64) || !oid.iter().all(u8::is_ascii_hexdigit) {
            return Err("Invalid Git object ID in review baseline".into());
        }
        entries.push(TreeEntry {
            path,
            mode: std::str::from_utf8(fields[0]).map_err(err)?.into(),
            oid: std::str::from_utf8(oid).map_err(err)?.into(),
        });
    }
    let mut files = baseline_files(runner, repo, entries)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Baseline {
        comparison: comparison.clone(),
        resolved_base,
        files,
    })
}

struct TreeEntry {
    path: PathBuf,
    mode: String,
    oid: String,
}

fn batch_header(output: &mut &[u8], oid: &str) -> Result<usize, String> {
    let end = output
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("Missing Git batch header")?;
    let header = std::str::from_utf8(&output[..end]).map_err(err)?;
    let fields: Vec<_> = header.split(' ').collect();
    if fields.len() != 3 || fields[0] != oid || fields[1] != "blob" {
        return Err("Invalid or missing Git blob in review baseline".into());
    }
    let size: usize = fields[2].parse().map_err(err)?;
    if size > CONTENT_LIMIT {
        return Err("Review baseline file exceeds 2 MiB".into());
    }
    *output = &output[end + 1..];
    Ok(size)
}

fn baseline_files(
    runner: &Runner<'_>,
    repo: &Repository,
    entries: Vec<TreeEntry>,
) -> Result<Vec<CapturedFile>, String> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    // Tree/index listings already supply exact object IDs and modes. Only IDs
    // enter the line-delimited batch protocol, so raw filenames stay lossless.
    let mut input = String::new();
    for entry in &entries {
        input.push_str(&entry.oid);
        input.push('\n');
    }
    let sizes = runner.git_with_input(
        &repo.root,
        &refs(&args(&["cat-file", "--batch-check"])),
        input.as_bytes(),
    )?;
    let mut output = sizes.as_slice();
    let mut expected = Vec::new();
    let mut total = 0;
    for entry in &entries {
        runner.check()?;
        let size = batch_header(&mut output, &entry.oid)?;
        total += size;
        if total > review_snapshot::TREE_LIMIT {
            return Err("Review baseline exceeds 8 MiB".into());
        }
        expected.push(size);
    }
    if !output.is_empty() {
        return Err("Unexpected Git batch size output".into());
    }
    // Check both per-file and aggregate sizes before asking Git for any bytes.
    let contents = runner.git_with_input(
        &repo.root,
        &refs(&args(&["cat-file", "--batch"])),
        input.as_bytes(),
    )?;
    let mut output = contents.as_slice();
    let mut files = Vec::new();
    for (entry, size) in entries.into_iter().zip(expected) {
        runner.check()?;
        if batch_header(&mut output, &entry.oid)? != size
            || output.len() <= size
            || output[size] != b'\n'
        {
            return Err("Incomplete or changed Git blob in review baseline".into());
        }
        files.push(CapturedFile::new(
            entry.path,
            output[..size].to_vec(),
            entry.mode,
            Some(entry.oid),
            false,
        ));
        output = &output[size + 1..];
    }
    if !output.is_empty() {
        return Err("Unexpected Git batch content output".into());
    }
    Ok(files)
}
/// Called by the review I/O worker. Captured buffers are immutable request-time
/// Ropes; disk is sampled twice. This is deliberately not a filesystem transaction.
pub fn capture_review(request: &Request, config: &Config) -> Result<(Repository, Capture), String> {
    let current = AtomicU64::new(0);
    let runner = Runner {
        config,
        serial: 0,
        current: &current,
        deadline: Instant::now() + config.timeout,
        overrides: Vec::new(),
    };
    let mut discovery = request.clone();
    discovery.inspect = false;
    discovery.review = None;
    let snapshot = inspect_with(&runner, &discovery)?.ok_or("No repository")?;
    let repo = snapshot.repository;
    let store = request
        .review
        .as_ref()
        .filter(|s| s.repository == repo)
        .ok_or("Load this worktree's review store before capturing")?;
    if snapshot.entries.iter().any(|e| e.conflict) {
        return Err("Resolve conflicts before creating a review snapshot".into());
    }
    let base = match &store.history {
        Some(history) => history.baseline.clone(),
        None => baseline(&runner, &repo, &request.comparison)?,
    };
    let tree = live_tree(&runner, request, &repo)?;
    if tree != live_tree(&runner, request, &repo)? {
        return Err("Files changed during review capture; retry :review snapshot".into());
    }
    if store.history.is_none() && base != baseline(&runner, &repo, &request.comparison)? {
        return Err("Git base changed during review capture; retry :review snapshot".into());
    }
    runner.check()?;
    Ok((
        repo,
        Capture {
            baseline: base,
            files: tree.files,
            expected_latest: store.history.as_ref().map_or(0, |h| h.latest()),
        },
    ))
}

pub(super) fn inspect_tree(
    runner: &Runner<'_>,
    request: &Request,
    snapshot: &mut Snapshot,
) -> Result<(), String> {
    let history = history(request, &snapshot.repository)?;
    let base = history.files(&request.comparison)?;
    if snapshot.entries.iter().any(|e| e.conflict) {
        return Err("Resolve conflicts before inspecting review snapshots".into());
    }
    let live = live_tree(runner, request, &snapshot.repository)?;
    let old: BTreeMap<_, _> = base.iter().map(|f| (f.path.clone(), f)).collect();
    let new: BTreeMap<_, _> = live.files.iter().map(|f| (f.path.clone(), f)).collect();
    // Git's explicit rename, or a unique exact-content move of a missing path.
    // Similarity guesses never supply annotation identity.
    let mut renames = BTreeMap::new();
    for entry in &snapshot.entries {
        if let Some(from) = &entry.original
            && old.contains_key(from)
            && !new.contains_key(from)
            && new.contains_key(&entry.path)
            && !old.contains_key(&entry.path)
        {
            renames.insert(entry.path.clone(), from.clone());
        }
    }
    if request.comparison == Comparison::ReviewFull
        && let Some(oid) = &history.baseline.resolved_base
    {
        let result = runner.run(
            &snapshot.repository.root,
            &refs(&args(&[
                "diff",
                "--name-status",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--find-renames",
                "--ignore-submodules=all",
                oid,
                "--",
            ])),
        )?;
        // Git rename hints are optional: the saved baseline must remain usable
        // even after the original commit has been pruned from the object store.
        let names = if result.success {
            result.bytes
        } else {
            Vec::new()
        };
        let mut records = names.split(|b| *b == 0).filter(|s| !s.is_empty());
        while let Some(status) = records.next() {
            let from = path(records.next().ok_or("Missing changed path")?)?;
            if matches!(status[0], b'R' | b'C') {
                let to = path(records.next().ok_or("Missing renamed path")?)?;
                if status[0] == b'R'
                    && old.contains_key(&from)
                    && !new.contains_key(&from)
                    && new.contains_key(&to)
                    && !old.contains_key(&to)
                {
                    renames.insert(to, from);
                }
            }
        }
    }
    let reference: BTreeMap<_, _> = if request.comparison == Comparison::ReviewFull {
        history
            .checkpoints
            .last()
            .unwrap()
            .files
            .iter()
            .map(|f| (f.path.clone(), f))
            .collect()
    } else {
        old.clone()
    };
    for (to, file) in &new {
        if old.contains_key(to) || renames.contains_key(to) || file.bytes().is_empty() {
            continue;
        }
        let candidates: Vec<_> = reference
            .iter()
            .filter(|(from, f)| !new.contains_key(*from) && f.same_content(file))
            .collect();
        let targets = new
            .iter()
            .filter(|(p, f)| !reference.contains_key(*p) && f.same_content(file))
            .count();
        if candidates.len() == 1 && targets == 1 && !renames.values().any(|p| p == candidates[0].0)
        {
            renames.insert(to.clone(), candidates[0].0.clone());
        }
    }
    let mut changes = Vec::new();
    for path in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
        if renames.values().any(|from| from == path) {
            continue;
        }
        let from = renames.get(path).unwrap_or(path);
        let previous = old.get(from);
        let current = new.get(path);
        if from == path
            && previous
                .zip(current)
                .is_some_and(|(a, b)| a.same_content(b))
        {
            continue;
        }
        let status = match (previous, current) {
            (None, _) => 'A',
            (_, None) => 'D',
            _ if from != path => 'R',
            _ => 'M',
        };
        changes.push(Entry {
            path: path.clone(),
            original: (from != path).then(|| from.clone()),
            index: '.',
            worktree: status,
            conflict: false,
        });
    }
    snapshot.review_files = Some(live.files);
    use sha2::{Digest, Sha256};
    let mut revision = Sha256::new();
    for (path, content, mode) in &live.disk {
        let path = path.as_os_str().as_encoded_bytes();
        revision.update((path.len() as u64).to_le_bytes());
        revision.update(path);
        revision.update([u8::from(content.present)]);
        revision.update(content.sha256.as_bytes());
        revision.update(mode.as_bytes());
    }
    snapshot.disk_revision = Some(format!("{:x}", revision.finalize()));
    snapshot.external_paths = live.external;
    snapshot.review_changes = changes;
    snapshot.review_renames = renames;
    Ok(())
}

pub(super) fn comparison(
    runner: &Runner<'_>,
    request: &Request,
    snapshot: &Snapshot,
) -> Result<Diff, String> {
    let repo = &snapshot.repository;
    let history = history(request, repo)?;
    let base = history.files(&request.comparison)?;
    let files = snapshot
        .review_files
        .as_ref()
        .ok_or("Review tree unavailable")?;
    let relative = request.path.strip_prefix(&repo.root).map_err(err)?;
    let old_path = snapshot
        .review_renames
        .get(relative)
        .map_or(relative, PathBuf::as_path);
    let old = base.iter().find(|f| f.path == old_path);
    let new = files.iter().find(|f| f.path == relative);
    let old_bytes = old.map_or(&b""[..], |f| f.bytes());
    let new_bytes = new.map_or(&b""[..], |f| f.bytes());
    let mut diff = compare_bytes(runner, old_bytes, new_bytes)?;
    let base_label = match request.comparison {
        Comparison::ReviewSince(id) => format!("review checkpoint #{id}"),
        _ => format!(
            "review full {}@{}",
            history.baseline.comparison.label(),
            history
                .baseline
                .resolved_base
                .as_deref()
                .map(short)
                .unwrap_or_else(|| "captured index/unborn".into())
        ),
    };
    diff.left_label = format!("{base_label}:{}", display_path(old_path));
    diff.right_label = format!(
        "{}:{}{}",
        if new.is_some_and(|f| f.unsaved) {
            "UNSAVED buffer"
        } else {
            "live disk"
        },
        display_path(relative),
        if snapshot.external_paths.iter().any(|p| p == relative) {
            " [disk differs; source buffer retained]"
        } else {
            ""
        }
    );
    diff.source_available = new.is_some();
    diff.renamed_from = (old_path != relative).then(|| old_path.to_path_buf());
    diff.review_base =
        (request.comparison == Comparison::ReviewFull).then(|| history.baseline.comparison.clone());
    diff.identity = Some(Identity {
        repository: repo.clone(),
        comparison: request.comparison.clone(),
        resolved_base: if request.comparison == Comparison::ReviewFull {
            history.baseline.resolved_base.clone()
        } else {
            None
        },
        old_path: old_path.into(),
        new_path: relative.into(),
        old: ContentId::new(old_bytes, old.is_some()),
        new: ContentId::new(new_bytes, new.is_some()),
        old_blob: old.map(|f| {
            if request.comparison == Comparison::ReviewFull {
                f.blob.clone().unwrap_or_else(|| f.sha256.clone())
            } else {
                f.sha256.clone()
            }
        }),
        new_blob: None,
        old_mode: old.map(|f| f.mode.clone()),
        new_mode: new.map(|f| f.mode.clone()),
    });
    diff.identify_locations();
    // Live disk coordinates need the same explicit mapping as staged diffs when
    // an already-open source retains older bytes. Never reload that document.
    if let Some(buffer) = buffer_for(request, &request.path)
        && buffer.text.bytes().ne(new_bytes.iter().copied())
    {
        let mapping = compare_bytes(runner, new_bytes, buffer.text.to_string().as_bytes())?;
        for row in &mut diff.rows {
            row.anchor = mapping.map_old(row.anchor);
        }
    }
    runner.check()?;
    Ok(diff)
}
