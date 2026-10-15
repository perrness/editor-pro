//! Lazy, bounded directory discovery. Open documents are protected from mutations.
use crate::{
    editor::{Editor, View},
    input::Key,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

const LIMIT: usize = 10_000;
#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub depth: usize,
    pub directory: bool,
    pub symlink: bool,
    pub expanded: bool,
}
struct Scan {
    path: PathBuf,
    result: io::Result<(Vec<Entry>, bool)>,
}
#[derive(Default)]
pub struct FileTree {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    top: usize,
    scan: Option<mpsc::Receiver<Scan>>,
    prompt: Option<(char, PathBuf, String)>,
    leader: u8,
}
impl FileTree {
    fn load(&mut self, path: PathBuf, depth: usize) -> io::Result<()> {
        if self.scan.is_some() {
            return Err(io::Error::other("Directory scan in progress"));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("file-tree".into())
            .spawn(move || {
                let result = (|| {
                    if !fs::symlink_metadata(&path)?.is_dir() || path.canonicalize()? != path {
                        return Err(io::Error::other(
                            "Directory changed or became a symlink; refresh tree",
                        ));
                    }
                    let mut entries = Vec::new();
                    let mut limited = false;
                    for entry in fs::read_dir(&path)? {
                        let entry = entry?;
                        if entry.file_name() == ".git" {
                            continue;
                        }
                        if entries.len() == LIMIT {
                            limited = true;
                            break;
                        }
                        let kind = entry.file_type()?;
                        entries.push(Entry {
                            path: entry.path(),
                            depth,
                            directory: kind.is_dir(),
                            symlink: kind.is_symlink(),
                            expanded: false,
                        });
                    }
                    entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
                    Ok((entries, limited))
                })();
                let _ = tx.send(Scan { path, result });
            })?;
        self.scan = Some(rx);
        Ok(())
    }
}
impl Editor {
    pub fn toggle_tree(&mut self, root: Option<&Path>) -> io::Result<()> {
        if self.view == View::Tree {
            self.tree.prompt = None;
            self.tree.leader = 0;
            self.view = View::Editing;
            return Ok(());
        }
        if self.view != View::Editing || self.note_edit.is_some() {
            return Err(io::Error::other("Return to source before opening the tree"));
        }
        let root = match root {
            Some(root) => root.to_path_buf(),
            None if !self.tree.root.as_os_str().is_empty() => self.tree.root.clone(),
            None => std::env::current_dir()?,
        };
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::other("Tree root must be a directory"));
        }
        // Keep at most one live scan, even across repeated toggles.
        if self.tree.scan.is_some() && self.tree.root != root {
            return Err(io::Error::other(
                "Wait for directory scan before changing root",
            ));
        }
        if (self.tree.root != root || self.tree.entries.is_empty()) && self.tree.scan.is_none() {
            self.tree.root = root.clone();
            self.tree.entries.clear();
            self.tree.selected = 0;
            self.tree.load(root, 0)?;
        }
        self.reset_context();
        self.view = View::Tree;
        self.info("Tree: ignored and hidden files included; symlinks are leaves");
        Ok(())
    }
    pub fn tick_tree(&mut self) -> bool {
        let Some(rx) = &self.tree.scan else {
            return false;
        };
        let scan = match rx.try_recv() {
            Ok(scan) => scan,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(_) => {
                self.tree.scan = None;
                self.error("Directory worker stopped");
                return true;
            }
        };
        self.tree.scan = None;
        match scan.result {
            Err(e) => self.error(&e.to_string()),
            Ok((mut entries, limited)) => {
                let index = if scan.path == self.tree.root {
                    Some(0)
                } else {
                    self.tree
                        .entries
                        .iter()
                        .position(|e| e.path == scan.path)
                        .map(|i| {
                            self.tree.entries[i].expanded = true;
                            i + 1
                        })
                };
                if let Some(index) = index {
                    let remaining = LIMIT.saturating_sub(self.tree.entries.len());
                    let truncated = entries.len() > remaining;
                    entries.truncate(remaining);
                    self.tree.entries.splice(index..index, entries);
                    if limited || truncated {
                        self.info("Tree limited to 10,000 entries; collapse folders to free space");
                    }
                }
            }
        }
        true
    }
    fn tree_protect(&self, path: &Path) -> io::Result<()> {
        for doc in std::iter::once(&self.document).chain(self.buffers.values().map(|s| &s.document))
        {
            if doc.path().is_some_and(|p| p.starts_with(path)) {
                return Err(io::Error::other(
                    "Path is used by an open buffer; close it before rename/delete",
                ));
            }
        }
        Ok(())
    }
    /// Mutations never overwrite a destination or recurse through a directory.
    pub fn tree_operation(&mut self, operation: char, source: &Path, name: &str) -> io::Result<()> {
        if self.tree.scan.is_some() {
            return Err(io::Error::other("Wait for directory scan"));
        }
        if source != self.tree.root {
            let parent = source
                .parent()
                .ok_or_else(|| io::Error::other("No parent"))?;
            if !source.starts_with(&self.tree.root) || parent.canonicalize()? != parent {
                return Err(io::Error::other(
                    "Source must stay within tree without symlink parents",
                ));
            }
        }
        if operation == 'd' || operation == 'r' {
            self.tree_protect(source)?;
        }
        match operation {
            'a' | 'A' => {
                let target = source.join(name);
                self.tree_target(&target)?;
                self.tree_protect(&target)?;
                if operation == 'A' {
                    fs::create_dir(target)?;
                } else {
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(target)?;
                }
            }
            'r' => {
                if source == self.tree.root {
                    return Err(io::Error::other("Cannot rename tree root"));
                }
                let target = source
                    .parent()
                    .ok_or_else(|| io::Error::other("No parent"))?
                    .join(name);
                self.tree_target(&target)?;
                self.tree_protect(&target)?;
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                rustix::fs::renameat_with(
                    rustix::fs::CWD,
                    source,
                    rustix::fs::CWD,
                    &target,
                    rustix::fs::RenameFlags::NOREPLACE,
                )?;
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                return Err(io::Error::other(
                    "Atomic no-clobber rename unavailable on this platform",
                ));
                self.navigation.move_tree_path(source, &target);
            }
            'd' => {
                if source == self.tree.root {
                    return Err(io::Error::other("Cannot delete tree root"));
                }
                let kind = fs::symlink_metadata(source)?.file_type();
                if kind.is_dir() {
                    fs::remove_dir(source)?;
                } else {
                    fs::remove_file(source)?;
                }
                self.tree_forget(source);
            }
            _ => return Err(io::Error::other("Unknown tree operation")),
        }
        self.refresh_git();
        self.tree.entries.clear();
        self.tree.selected = 0;
        self.tree.load(self.tree.root.clone(), 0)?;
        self.info("Filesystem operation completed");
        Ok(())
    }
    fn tree_target(&self, target: &Path) -> io::Result<()> {
        if target
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(io::Error::other("Use a path within the tree without '..'"));
        }
        let parent = target
            .parent()
            .ok_or_else(|| io::Error::other("No parent"))?
            .canonicalize()?;
        if !parent.starts_with(&self.tree.root)
            || target.parent() != Some(parent.as_path())
            || target.file_name().is_none()
        {
            return Err(io::Error::other("Destination must stay inside tree root"));
        }
        match fs::symlink_metadata(target) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
            Ok(_) => Err(io::Error::other("Destination already exists")),
        }
    }
    fn tree_forget(&mut self, path: &Path) {
        self.navigation.forget_tree_path(path);
    }
    pub(crate) fn tree_key(&mut self, key: Key) {
        if let Some((op, path, mut value)) = self.tree.prompt.take() {
            match key {
                Key::Escape | Key::Ctrl('c' | '[') => return,
                Key::Enter => {
                    if op == 'd' && value != "yes" {
                        self.info("Deletion cancelled");
                        return;
                    }
                    if value.is_empty() {
                        self.error("Name must not be empty");
                        return;
                    }
                    if let Err(e) = self.tree_operation(op, &path, &value) {
                        self.error(&e.to_string());
                    }
                    return;
                }
                Key::Backspace => crate::text::pop_grapheme(&mut value),
                Key::Char(c) if !c.is_control() => value.push(c),
                _ => {}
            }
            self.tree.prompt = Some((op, path, value));
            return;
        }
        if self.tree.leader > 0 {
            let stage = self.tree.leader;
            self.tree.leader = 0;
            if stage == 1 && key == Key::Char('p') {
                self.tree.leader = 2;
            } else if stage == 2 && key == Key::Char('v') {
                let _ = self.toggle_tree(None);
            }
            return;
        }
        match key {
            Key::Escape | Key::Ctrl('c' | '[') => {
                let _ = self.toggle_tree(None);
            }
            Key::Char(' ') => self.tree.leader = 1,
            Key::Down | Key::Char('j') => {
                self.tree.selected =
                    (self.tree.selected + 1).min(self.tree.entries.len().saturating_sub(1))
            }
            Key::Up | Key::Char('k') => self.tree.selected = self.tree.selected.saturating_sub(1),
            Key::Char('R') if self.tree.scan.is_none() => {
                self.tree.entries.clear();
                self.tree.selected = 0;
                if let Err(e) = self.tree.load(self.tree.root.clone(), 0) {
                    self.error(&e.to_string());
                }
            }
            Key::Char('a' | 'A') => {
                let path = self
                    .tree
                    .entries
                    .get(self.tree.selected)
                    .map(|e| {
                        if e.directory {
                            e.path.clone()
                        } else {
                            e.path.parent().unwrap().to_path_buf()
                        }
                    })
                    .unwrap_or(self.tree.root.clone());
                let Key::Char(op) = key else { unreachable!() };
                self.tree.prompt = Some((op, path, String::new()));
            }
            _ => {
                let Some(entry) = self.tree.entries.get(self.tree.selected).cloned() else {
                    return;
                };
                match key {
                    Key::Char('r' | 'd') => {
                        let Key::Char(op) = key else { unreachable!() };
                        self.tree.prompt = Some((op, entry.path, String::new()));
                    }
                    Key::Left | Key::Char('h') if self.tree.scan.is_none() => {
                        if entry.expanded {
                            let end = self
                                .tree
                                .entries
                                .iter()
                                .enumerate()
                                .skip(self.tree.selected + 1)
                                .find(|(_, e)| e.depth <= entry.depth)
                                .map_or(self.tree.entries.len(), |(i, _)| i);
                            self.tree.entries.drain(self.tree.selected + 1..end);
                            self.tree.entries[self.tree.selected].expanded = false;
                        } else if entry.depth > 0
                            && let Some(i) = self.tree.entries[..self.tree.selected]
                                .iter()
                                .rposition(|e| e.depth < entry.depth)
                        {
                            self.tree.selected = i;
                        }
                    }
                    Key::Right | Key::Char('l') | Key::Enter if entry.directory => {
                        if !entry.expanded {
                            if let Err(e) = self.tree.load(entry.path, entry.depth + 1) {
                                self.error(&e.to_string());
                            }
                        } else {
                            self.tree.selected =
                                (self.tree.selected + 1).min(self.tree.entries.len() - 1);
                        }
                    }
                    Key::Enter if !entry.symlink => {
                        let result = (|| {
                            if !fs::symlink_metadata(&entry.path)?.is_file()
                                || entry.path.canonicalize()? != entry.path
                            {
                                return Err(io::Error::other(
                                    "File changed or became a symlink; refresh tree",
                                ));
                            }
                            self.open_buffer(&entry.path)
                        })();
                        if let Err(e) = result {
                            self.error(&e.to_string());
                        }
                    }
                    Key::Enter => self.error("Symlinks are not followed by the tree"),
                    _ => {}
                }
            }
        }
    }
}

pub fn render(frame: &mut ratatui::Frame<'_>, editor: &mut Editor, theme: &crate::theme::Theme) {
    use ratatui::{layout::Rect, style::Style, widgets::Paragraph};
    let area = frame.area();
    let tree = &mut editor.tree;
    let height = area.height.saturating_sub(3) as usize;
    tree.top = tree.top.min(tree.selected);
    if height > 0 && tree.selected >= tree.top + height {
        tree.top = tree.selected + 1 - height;
    }
    let style = Style::default().fg(theme.foreground).bg(theme.background);
    let title = format!(
        "Tree · {}{}",
        tree.root.display(),
        if tree.scan.is_some() {
            " · loading"
        } else {
            ""
        }
    );
    frame.render_widget(
        Paragraph::new(crate::text::visible_text(&title, 0, area.width as usize)).style(style),
        Rect::new(area.x, area.y, area.width, 1),
    );
    for (row, entry) in tree.entries.iter().skip(tree.top).take(height).enumerate() {
        let marker = if entry.expanded {
            "▾"
        } else if entry.directory {
            "▸"
        } else if entry.symlink {
            "@"
        } else {
            " "
        };
        let text = format!(
            "{}{} {}",
            "  ".repeat(entry.depth),
            marker,
            entry.path.file_name().unwrap_or_default().to_string_lossy()
        );
        frame.render_widget(
            Paragraph::new(crate::text::visible_text(&text, 0, area.width as usize)).style(
                if tree.top + row == tree.selected {
                    style.bg(theme.selection)
                } else {
                    style
                },
            ),
            Rect::new(area.x, area.y + 1 + row as u16, area.width, 1),
        );
    }
    if area.height > 1 {
        let status = if let Some((op, path, value)) = &tree.prompt {
            let label = match op {
                'a' => "New file",
                'A' => "New directory",
                'r' => "Move (relative path)",
                _ => "Delete (type yes)",
            };
            format!("{label} [{}]: {value}", path.display())
        } else {
            editor.message.clone()
        };
        frame.render_widget(
            Paragraph::new(crate::text::visible_text(&status, 0, area.width as usize)).style(
                style.fg(if editor.message_is_error {
                    theme.error
                } else {
                    theme.command
                }),
            ),
            Rect::new(area.x, area.bottom() - 2, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(
                "j/k · h/l fold · Enter open · a/A new · r move · d delete · R scan · Esc close",
            )
            .style(style.fg(theme.muted)),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }
}
