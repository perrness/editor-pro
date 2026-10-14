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
    }

    pub fn open_buffer(&mut self, path: &Path) -> io::Result<()> {
        let path = absolute_path(path)?;
        if self.document.path() == Some(&path) {
            return self.switch_buffer(self.active_buffer_id);
        }
        if let Some((&id, _)) = self
            .buffers
            .iter()
            .find(|(_, slot)| slot.document.path() == Some(&path))
        {
            return self.switch_buffer(id);
        }
        // Read successfully before changing any active/unsaved state.
        let document = Document::open(&path)?;
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        self.activate(
            id,
            BufferSlot {
                document,
                viewport: Viewport::default(),
            },
        );
        Ok(())
    }

    pub fn new_buffer(&mut self) {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        self.activate(
            id,
            BufferSlot {
                document: Document::default(),
                viewport: Viewport::default(),
            },
        );
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
            self.document = document;
            self.viewport = Viewport::default();
            self.reset_context();
        } else if force {
            // A forced open reloads the requested target, never the document
            // being left behind. Prepare it first so failure cannot lose state.
            let document = Document::open(Path::new(argument))?;
            let path = document.path().unwrap().to_path_buf();
            self.open_buffer(&path)?;
            self.document = document;
            self.viewport = Viewport::default();
            self.reset_context();
        } else {
            self.open_buffer(Path::new(argument))?;
        }
        self.info("Opened");
        Ok(())
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

    pub fn check_external(&mut self) -> bool {
        let mut changed = false;
        let mut message = None;
        for (id, document) in std::iter::once((self.active_buffer_id, &mut self.document)).chain(
            self.buffers
                .iter_mut()
                .map(|(id, slot)| (*id, &mut slot.document)),
        ) {
            let before = document.external_changed;
            match document.check_external() {
                Ok(true) if !before => {
                    message = Some(format!(
                        "Buffer {id} changed on disk. :e! reloads; :w NEW_PATH preserves edits"
                    ))
                }
                Err(error) => {
                    document.external_changed = true;
                    if !before {
                        message = Some(format!("Cannot check buffer {id} on disk: {error}"));
                    }
                }
                _ => {}
            }
            changed |= before != document.external_changed;
        }
        if let Some(message) = message {
            self.error(&message);
        }
        changed
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
