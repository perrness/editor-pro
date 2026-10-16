//! Open documents and their saved viewports. The active slot stays directly on
//! Editor so rendering and edit dispatch do not need a second editing engine.
use crate::{
    editor::{Editor, View, Viewport},
    file::{Document, absolute_path},
    input::Key,
};
use std::{io, path::Path};

pub(crate) struct BufferSlot {
    pub document: Document,
    pub viewport: Viewport,
}
pub struct BufferInfo {
    pub id: usize,
    pub name: String,
    pub modified: bool,
    pub external_changed: bool,
    pub active: bool,
}

impl Editor {
    pub fn buffer_count(&self) -> usize {
        self.buffers.len() + 1
    }

    pub fn dirty_buffer_count(&self) -> usize {
        usize::from(self.document.buffer.is_modified())
            + self
                .buffers
                .values()
                .filter(|slot| slot.document.buffer.is_modified())
                .count()
    }

    pub fn buffer_infos(&self) -> Vec<BufferInfo> {
        let mut infos: Vec<_> = std::iter::once((self.active_buffer_id, &self.document))
            .chain(self.buffers.iter().map(|(id, slot)| (*id, &slot.document)))
            .map(|(id, document)| BufferInfo {
                id,
                name: document.display_name(),
                modified: document.buffer.is_modified(),
                external_changed: document.external_changed,
                active: id == self.active_buffer_id,
            })
            .collect();
        infos.sort_by_key(|info| info.id);
        infos
    }

    pub fn switch_buffer(&mut self, id: usize) -> io::Result<()> {
        let origin = self.source_location();
        self.switch_buffer_unrecorded(id)?;
        self.record_navigation(origin);
        Ok(())
    }

    pub(crate) fn switch_buffer_unrecorded(&mut self, id: usize) -> io::Result<()> {
        if id == self.active_buffer_id {
            self.reset_context();
            return Ok(());
        }
        let slot = self
            .buffers
            .remove(&id)
            .ok_or_else(|| io::Error::other(format!("No buffer {id}")))?;
        self.activate(id, slot);
        Ok(())
    }

    fn activate(&mut self, id: usize, slot: BufferSlot) {
        self.document.buffer.commit();
        let old = BufferSlot {
            document: std::mem::replace(&mut self.document, slot.document),
            viewport: std::mem::replace(&mut self.viewport, slot.viewport),
        };
        self.buffers.insert(self.active_buffer_id, old);
        self.active_buffer_id = id;
        self.reset_context();
        self.remember_file();
    }

    pub fn open_buffer(&mut self, path: &Path) -> io::Result<()> {
        let origin = self.source_location();
        self.open_buffer_unrecorded(path)?;
        self.record_navigation(origin);
        Ok(())
    }

    pub(crate) fn has_open_path(&self, path: &Path) -> bool {
        self.document.path() == Some(path)
            || self
                .buffers
                .values()
                .any(|slot| slot.document.path() == Some(path))
    }

    pub(crate) fn open_buffer_unrecorded(&mut self, path: &Path) -> io::Result<()> {
        // Already-open identities survive deletion of their parent on disk.
        let path = if self.has_open_path(path) {
            path.to_path_buf()
        } else {
            absolute_path(path)?
        };
        if self.document.path() == Some(&path) {
            return self.switch_buffer_unrecorded(self.active_buffer_id);
        }
        if let Some((&id, _)) = self
            .buffers
            .iter()
            .find(|(_, slot)| slot.document.path() == Some(&path))
        {
            return self.switch_buffer_unrecorded(id);
        }
        // Read successfully before changing any active/unsaved state.
        let document = Document::open(&path)?;
        self.open_prepared_buffer(document);
        Ok(())
    }

    pub(crate) fn open_prepared_buffer(&mut self, document: Document) {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        self.activate(
            id,
            BufferSlot {
                document,
                viewport: Viewport::default(),
            },
        );
    }

    pub fn new_buffer(&mut self) {
        let origin = self.source_location();
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        self.activate(
            id,
            BufferSlot {
                document: Document::default(),
                viewport: Viewport::default(),
            },
        );
        self.record_navigation(origin);
    }

    pub fn close_buffer(&mut self, force: bool) -> io::Result<()> {
        self.ensure_can_discard(force)?;
        let next = self
            .buffers
            .keys()
            .copied()
            .find(|id| *id > self.active_buffer_id)
            .or_else(|| self.buffers.keys().next().copied());
        if let Some(id) = next {
            let slot = self.buffers.remove(&id).unwrap();
            self.document = slot.document;
            self.viewport = slot.viewport;
            self.active_buffer_id = id;
        } else {
            self.document = Document::default();
            self.viewport = Viewport::default();
            self.active_buffer_id = self.next_buffer_id;
            self.next_buffer_id += 1;
        }
        self.reset_context();
        self.remember_file();
        Ok(())
    }

    pub(crate) fn cycle_buffer(&mut self, backward: bool) -> io::Result<()> {
        let infos = self.buffer_infos();
        let index = infos.iter().position(|info| info.active).unwrap();
        let next = if backward {
            (index + infos.len() - 1) % infos.len()
        } else {
            (index + 1) % infos.len()
        };
        self.switch_buffer(infos[next].id)
    }

    pub(crate) fn edit_command(&mut self, argument: &str, force: bool) -> io::Result<()> {
        if argument.is_empty() {
            self.ensure_can_discard(force)?;
            let path = self
                .document
                .path()
                .ok_or_else(|| io::Error::other("No file name"))?;
            let document = Document::open(path)?;
            if document.baseline().is_none() && self.document.baseline().is_some() {
                return Err(io::Error::other(
                    "File deleted or renamed; buffer retained. :refresh compares; :w NEW_PATH recovers it",
                ));
            }
            self.replace_reloaded_document(document);
        } else if force {
            // A forced open reloads the requested target, never the document
            // being left behind. Prepare it first so failure cannot lose state.
            let document = Document::open(Path::new(argument))?;
            let path = document.path().unwrap().to_path_buf();
            self.open_buffer(&path)?;
            self.replace_reloaded_document(document);
        } else {
            self.open_buffer(Path::new(argument))?;
        }
        self.info("Opened");
        Ok(())
    }

    pub(crate) fn replace_reloaded_document(&mut self, mut document: Document) {
        let (line, column) = self.document.buffer.position();
        let line = line.min(document.buffer.line_count() - 1);
        let cursor = document.buffer.text().line_to_char(line)
            + column.min(document.buffer.line(line).chars().count());
        document.buffer.set_cursor(cursor, false);
        self.document = document;
        self.viewport.top = self.viewport.top.min(line);
        self.viewport.left = self.viewport.left.min(crate::text::display_column(
            &self.document.buffer.line(line),
            self.document.buffer.position().1,
        ));
        self.reset_context();
    }

    pub(crate) fn save_current(&mut self, destination: Option<&Path>) -> io::Result<()> {
        if let Some(path) = destination {
            let normalized = absolute_path(path)?;
            if let Some((id, _)) = self
                .buffers
                .iter()
                .find(|(_, slot)| slot.document.path() == Some(&normalized))
            {
                return Err(io::Error::other(format!(
                    "Destination belongs to open buffer {id}; switch to it first"
                )));
            }
        }
        self.document.save(destination)?;
        self.remember_file();
        self.refresh_git();
        Ok(())
    }

    pub(crate) fn ensure_all_saved(&self, force: bool) -> io::Result<()> {
        if !force && let Some(info) = self.buffer_infos().into_iter().find(|info| info.modified) {
            return Err(io::Error::other(format!(
                "Unsaved changes in buffer {}: {}. Use :b {} and :w, or :q! to discard all",
                info.id, info.name, info.id
            )));
        }
        Ok(())
    }

    pub(crate) fn buffer_list_key(&mut self, key: Key) {
        let infos = self.buffer_infos();
        let View::Buffers { selected } = &mut self.view else {
            return;
        };
        match key {
            Key::Escape | Key::Ctrl('c' | '[') => self.view = View::Editing,
            Key::Down | Key::Char('j') | Key::Ctrl('n') => {
                *selected = (*selected + 1).min(infos.len() - 1)
            }
            Key::Up | Key::Char('k') | Key::Ctrl('p') => *selected = selected.saturating_sub(1),
            Key::Enter => {
                let id = infos[*selected].id;
                if let Err(error) = self.switch_buffer(id) {
                    self.error(&error.to_string());
                }
            }
            _ => {}
        }
    }
}
