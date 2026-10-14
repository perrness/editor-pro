use std::{fs, path::Path};

use editor_pro::{
    editor::Editor,
    file::{Document, MAX_FILE_BYTES},
};
use tempfile::tempdir;

#[test]
fn load_save_round_trip_preserves_bom_crlf_and_final_newline() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("source.kt");
    let original = "\u{feff}val café = \"界\"\r\n";
    fs::write(&path, original).unwrap();
    let mut document = Document::open(&path).unwrap();
    document.save(None).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    assert!(!document.buffer.is_modified());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn new_file_and_named_scratch_can_be_saved() {
    let dir = tempdir().unwrap();
    for name in ["new.rs", "with spaces.cs"] {
        let path = dir.path().join(name);
        let mut document = Document::open(&path).unwrap();
        document.buffer.insert("hello");
        document.save(None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }
    let mut document = Document::default();
    document.buffer.insert("scratch");
    let path = dir.path().join("scratch");
    document.save(Some(&path)).unwrap();
    let canonical = path.canonicalize().unwrap();
    assert_eq!(document.path(), Some(canonical.as_path()));
}

#[test]
fn no_final_newline_is_added() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("no-newline");
    fs::write(&path, "x").unwrap();
    let mut document = Document::open(&path).unwrap();
    document.buffer.insert("y");
    document.save(None).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"yx");
}

#[test]
fn external_same_size_change_is_detected_without_losing_either_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "old").unwrap();
    let mut document = Document::open(&path).unwrap();
    document.buffer.insert("local ");
    fs::write(&path, "new").unwrap();
    assert!(document.check_external().unwrap());
    assert!(document.save(None).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(document.buffer.text().to_string(), "local old");
    assert!(document.buffer.is_modified());
    let recovery = dir.path().join("recovered");
    document.save(Some(&recovery)).unwrap();
    assert_eq!(fs::read_to_string(recovery).unwrap(), "local old");
}

#[test]
fn deleted_file_and_newly_created_destination_are_conflicts() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "old").unwrap();
    let mut document = Document::open(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(document.save(None).is_err());
    assert!(!path.exists());
    let mut document = Document::open(&path).unwrap();
    fs::write(&path, "someone else's work").unwrap();
    assert!(document.save(None).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "someone else's work");
}

#[test]
fn save_as_never_overwrites_an_unrelated_existing_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("existing");
    fs::write(&path, "keep").unwrap();
    let mut document = Document::default();
    document.buffer.insert("replace");
    assert!(document.save(Some(&path)).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
    assert!(document.path().is_none());
    assert!(document.buffer.is_modified());
}

#[test]
fn reload_and_open_guard_unsaved_edits_and_retain_state_on_failure() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "disk").unwrap();
    let mut editor = Editor::new(Document::open(&path).unwrap());
    editor.document.buffer.insert("local");
    assert!(editor.execute_command("e").is_err());
    assert!(
        editor
            .execute_command("e! /not/a/real/parent/file")
            .is_err()
    );
    assert_eq!(editor.document.buffer.text().to_string(), "localdisk");
    editor.execute_command("e!").unwrap();
    assert_eq!(editor.document.buffer.text().to_string(), "disk");
    assert!(!editor.document.buffer.is_modified());
}

#[test]
fn invalid_input_is_rejected_before_creating_a_buffer() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file");
    for bytes in [b"\xff".as_slice(), b"text\0binary".as_slice()] {
        fs::write(&path, bytes).unwrap();
        assert!(Document::open(&path).is_err());
    }
    assert!(Document::open(dir.path()).is_err());
    assert!(Document::open(Path::new("")).is_err());
}

#[test]
fn failed_save_keeps_dirty_state_and_original_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "old").unwrap();
    let mut document = Document::open(&path).unwrap();
    document.buffer.insert("new");
    let absent_parent = dir.path().join("missing/file");
    assert!(document.save(Some(&absent_parent)).is_err());
    assert!(document.buffer.is_modified());
    assert_eq!(fs::read_to_string(&path).unwrap(), "old");
}

#[test]
fn oversized_file_is_rejected_without_reading_its_contents() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("large");
    fs::File::create(&path)
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    let error = Document::open(&path).err().unwrap();
    assert!(error.to_string().contains("64 MiB"));
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_FILE_BYTES + 1);
}

#[cfg(unix)]
#[test]
fn executable_permissions_are_preserved_and_readonly_is_respected() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let path = dir.path().join("script");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
    let mut document = Document::open(&path).unwrap();
    document.buffer.insert("new");
    document.save(None).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o751
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    document.buffer.insert("dirty");
    assert!(document.save(None).is_err());
    assert!(document.buffer.is_modified());
    assert_eq!(fs::read_to_string(&path).unwrap(), "newold");
}

#[cfg(unix)]
#[test]
fn symlinks_and_hardlinks_are_not_silently_replaced() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let path = dir.path().join("target");
    let link = dir.path().join("link");
    fs::write(&path, "original").unwrap();
    symlink(&path, &link).unwrap();
    assert!(Document::open(&link).is_err());
    fs::remove_file(&link).unwrap();
    let mut document = Document::open(&path).unwrap();
    fs::hard_link(&path, &link).unwrap();
    document.buffer.insert("edit");
    assert!(document.save(None).is_err());
    assert_eq!(fs::read_to_string(&link).unwrap(), "original");
}
