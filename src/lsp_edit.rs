//! Validate a complete server edit before changing any source buffer. Applying a
//! preview only makes ordinary, independently undoable unsaved buffer edits.
use crate::{
    buffers::BufferSlot,
    editor::{Editor, Viewport},
    file::{Document, absolute_path},
    lsp::{MAX_DOCUMENT_BYTES, MAX_ITEMS, Stamp, Versioned, scalar_range, uri_path},
};
use lsp_types::TextEdit;
use ropey::Rope;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

pub struct EditPlan {
    pub files: Vec<PlannedFile>,
    pub edit_count: usize,
}
pub struct PlannedFile {
    pub path: PathBuf,
    pub before: Rope,
    pub after: Rope,
    pub edits: Vec<(std::ops::Range<usize>, String)>,
    pub(crate) stamp: Option<Stamp>,
    pub(crate) unopened: Option<Document>,
}

impl Editor {
    pub fn lsp_edit_guards(&self) -> BTreeMap<PathBuf, Stamp> {
        std::iter::once((self.active_buffer_id, &self.document))
            .chain(self.buffers.iter().map(|(id, slot)| (*id, &slot.document)))
            .filter_map(|(id, doc)| doc.path().map(|path| (path.to_path_buf(), stamp(id, doc))))
            .collect()
    }

    pub fn prepare_lsp_edit(
        &self,
        value: &Value,
        root: &Path,
        versions: &[Versioned],
        guards: &BTreeMap<PathBuf, Stamp>,
    ) -> io::Result<EditPlan> {
        if self.note_edit.is_some() {
            return Err(error("Finish the review note before applying source edits"));
        }
        let mut groups: BTreeMap<PathBuf, (Option<i32>, Vec<TextEdit>)> = BTreeMap::new();
        if value.get("changes").is_some_and(|v| !v.is_null())
            && value.get("documentChanges").is_some_and(|v| !v.is_null())
        {
            return Err(error(
                "Workspace edit supplied both changes and documentChanges",
            ));
        }
        if let Some(changes) = value.get("changes").filter(|v| !v.is_null()) {
            let changes = changes
                .as_object()
                .ok_or_else(|| error("Invalid workspace changes"))?;
            for (uri, edits) in changes {
                add_edits(&mut groups, uri, None, edits)?;
            }
        } else if let Some(changes) = value.get("documentChanges").filter(|v| !v.is_null()) {
            for item in changes
                .as_array()
                .ok_or_else(|| error("Invalid documentChanges"))?
            {
                if item.get("kind").is_some() {
                    return Err(error(
                        "LSP create/rename/delete file operations are unsupported",
                    ));
                }
                let uri = item["textDocument"]["uri"]
                    .as_str()
                    .ok_or_else(|| error("Missing edited URI"))?;
                let version = if item["textDocument"]["version"].is_null() {
                    None
                } else {
                    Some(
                        serde_json::from_value::<i32>(item["textDocument"]["version"].clone())
                            .map_err(io::Error::other)?,
                    )
                };
                add_edits(&mut groups, uri, version, &item["edits"])?;
            }
        } else {
            return Err(error("Server supplied no text edits"));
        }
        if groups.len() > 32 {
            return Err(error("LSP edit exceeds 32 files"));
        }
        let mut files = Vec::new();
        let mut edit_count = 0;
        let current = self.lsp_edit_guards();
        for (path, (version, edits)) in groups {
            if !path.starts_with(root) {
                return Err(error("LSP edit is outside the server project root"));
            }
            let current_stamp = current.get(&path);
            if current_stamp != guards.get(&path) {
                return Err(error(
                    "An affected buffer changed or was opened/closed since the request",
                ));
            }
            if let Some(stamp) = current_stamp
                && !versions
                    .iter()
                    .any(|d| d.snapshot.path == path && &d.snapshot.stamp == stamp)
            {
                return Err(error(
                    "The server did not synchronize an affected open buffer; save/close that target and retry",
                ));
            }
            if let Some(version) = version
                && !versions.iter().any(|d| {
                    d.snapshot.path == path
                        && d.version == version
                        && Some(&d.snapshot.stamp) == current_stamp
                })
            {
                return Err(error(
                    "Workspace edit has an obsolete or unknown document version",
                ));
            }
            let unopened = if current_stamp.is_none() {
                if !path.is_file() {
                    return Err(error(
                        "LSP text edits require an existing file; resource operations are unsupported",
                    ));
                }
                Some(Document::open(&path)?)
            } else {
                None
            };
            let doc = if let Some(stamp) = current_stamp {
                self.lsp_document(stamp.buffer_id)
                    .ok_or_else(|| error("Edited buffer was closed"))?
            } else {
                unopened.as_ref().unwrap()
            };
            let before = doc.buffer.text().clone();
            if before.len_bytes() > MAX_DOCUMENT_BYTES {
                return Err(error("LSP edit target exceeds 2 MiB"));
            }
            let mut replacements = Vec::new();
            for edit in edits {
                if edit.new_text.contains('\0') {
                    return Err(error("LSP edit contains NUL"));
                }
                replacements.push((scalar_range(&before, edit.range)?, edit.new_text));
            }
            replacements.sort_by_key(|(range, _)| (range.start, range.end));
            for pair in replacements.windows(2) {
                if pair[0].0.end > pair[1].0.start || pair[0].0.start == pair[1].0.start {
                    return Err(error("Overlapping or ambiguous LSP text edits"));
                }
            }
            edit_count += replacements.len();
            if edit_count > MAX_ITEMS {
                return Err(error("LSP edit exceeds 4096 replacements"));
            }
            let mut after = before.clone();
            for (range, text) in replacements.iter().rev() {
                after.remove(range.clone());
                after.insert(range.start, text);
                if after.len_bytes() > MAX_DOCUMENT_BYTES {
                    return Err(error("LSP edit result exceeds 2 MiB"));
                }
            }
            files.push(PlannedFile {
                path,
                before,
                after,
                edits: replacements,
                stamp: current_stamp.cloned(),
                unopened,
            });
        }
        if edit_count == 0 {
            return Err(error("Server returned no edits"));
        }
        Ok(EditPlan { files, edit_count })
    }

    pub fn apply_lsp_edit(&mut self, plan: EditPlan) -> io::Result<()> {
        self.apply_edit_plan(plan)
    }

    /// Shared final transaction for validated LSP and AI previews. The callers
    /// establish provenance; this boundary rechecks every buffer and disk file.
    pub fn apply_edit_plan(&mut self, mut plan: EditPlan) -> io::Result<()> {
        if self.note_edit.is_some() {
            return Err(error("Finish the review note before applying source edits"));
        }
        // All guards, including disk baselines, are checked before the first splice.
        let current = self.lsp_edit_guards();
        for file in &mut plan.files {
            if current.get(&file.path) != file.stamp.as_ref() {
                return Err(error("Edit preview is stale; request it again"));
            }
            let document = if let Some(stamp) = &file.stamp {
                self.lsp_document_mut(stamp.buffer_id)
                    .ok_or_else(|| error("Edited buffer was closed"))?
            } else {
                file.unopened
                    .as_mut()
                    .ok_or_else(|| error("Missing prepared document"))?
            };
            if document.buffer.text() != &file.before || document.check_external()? {
                return Err(error(
                    "Edit target changed in memory or on disk; no edits applied",
                ));
            }
        }
        let mut changed = 0;
        for file in plan.files {
            if file.before == file.after {
                continue;
            }
            let id = if let Some(stamp) = file.stamp {
                stamp.buffer_id
            } else {
                let id = self.next_buffer_id;
                self.next_buffer_id += 1;
                self.buffers.insert(
                    id,
                    BufferSlot {
                        document: file.unopened.unwrap(),
                        viewport: Viewport::default(),
                    },
                );
                id
            };
            let insert = self.mode == crate::editor::Mode::Insert && id == self.active_buffer_id;
            let document = self.lsp_document_mut(id).unwrap();
            let old_cursor = document.buffer.cursor();
            let mut cursor = old_cursor as isize;
            for (range, text) in &file.edits {
                if old_cursor < range.start {
                    break;
                }
                if old_cursor <= range.end {
                    cursor +=
                        range.start as isize - old_cursor as isize + text.chars().count() as isize;
                    break;
                }
                cursor += text.chars().count() as isize - range.len() as isize;
            }
            document.buffer.commit();
            document.buffer.replace_range(
                0..document.buffer.text().len_chars(),
                &file.after.to_string(),
            );
            document.buffer.set_cursor(cursor.max(0) as usize, insert);
            document.buffer.commit();
            changed += 1;
        }
        self.search.invalidate();
        self.refresh_git();
        self.info(&format!(
            "Applied {} edits to {changed} buffers; unsaved, undo with u in each buffer",
            plan.edit_count
        ));
        Ok(())
    }

    pub(crate) fn lsp_document(&self, id: usize) -> Option<&Document> {
        if id == self.active_buffer_id {
            Some(&self.document)
        } else {
            self.buffers.get(&id).map(|slot| &slot.document)
        }
    }
    fn lsp_document_mut(&mut self, id: usize) -> Option<&mut Document> {
        if id == self.active_buffer_id {
            Some(&mut self.document)
        } else {
            self.buffers.get_mut(&id).map(|slot| &mut slot.document)
        }
    }
}

fn stamp(id: usize, doc: &Document) -> Stamp {
    Stamp {
        buffer_id: id,
        instance: doc.buffer.instance(),
        generation: doc.buffer.generation(),
    }
}
fn error(message: &str) -> io::Error {
    io::Error::other(message)
}
fn add_edits(
    groups: &mut BTreeMap<PathBuf, (Option<i32>, Vec<TextEdit>)>,
    uri: &str,
    version: Option<i32>,
    value: &Value,
) -> io::Result<()> {
    let path = absolute_path(&uri_path(uri)?)?;
    if groups.contains_key(&path) {
        return Err(error(
            "Multiple document edits for the same file are unsupported",
        ));
    }
    let items = value
        .as_array()
        .ok_or_else(|| error("Invalid text edits"))?;
    if items.iter().any(|item| item.get("annotationId").is_some()) {
        return Err(error("Annotated workspace edits are unsupported"));
    }
    if items.len() > MAX_ITEMS {
        return Err(error("LSP edit exceeds 4096 replacements"));
    }
    let edits = items
        .iter()
        .map(|v| serde_json::from_value(v.clone()).map_err(io::Error::other))
        .collect::<io::Result<Vec<_>>>()?;
    groups.insert(path, (version, edits));
    Ok(())
}
