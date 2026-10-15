use std::fs;

use editor_pro::{config, theme::ThemeName};
use tempfile::tempdir;

#[test]
fn missing_config_uses_defaults_without_creating_directories() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("editor-pro/config.json");
    assert_eq!(config::load(&path).unwrap().theme, ThemeName::TokyoNight);
    assert!(config::load(&path).unwrap().lsp.auto_install);
    assert!(!config::load(&path).unwrap().editor.relative_line_numbers);
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn config_loads_theme_without_rewriting_user_bytes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    let bytes = b"{\r\n  \"theme\": \"gruvbox\"\r\n}\r\n";
    fs::write(&path, bytes).unwrap();
    assert_eq!(config::load(&path).unwrap().theme, ThemeName::Gruvbox);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::write(&path, "{}").unwrap();
    assert_eq!(config::load(&path).unwrap().theme, ThemeName::TokyoNight);
}

#[test]
fn invalid_settings_report_the_path_without_overwriting_them() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    for text in [
        "{",
        r#"{"theme":"missing"}"#,
        r#"{"them":"gruvbox"}"#,
        r#"{"theme":null}"#,
        r#"{"lsp":{"auto_install":"true"}}"#,
        r#"{"lsp":{"install":false}}"#,
        r#"{"editor":{"relative_line_numbers":"true"}}"#,
        r#"{"editor":{"relative_line_numbers":null}}"#,
        r#"{"editor":{"relative_numbers":true}}"#,
        r#"{"editor":false}"#,
    ] {
        fs::write(&path, text).unwrap();
        let error = config::load(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains(&path.display().to_string()));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
    fs::write(&path, vec![b' '; 64 * 1024 + 1]).unwrap();
    assert!(
        config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("64 KiB")
    );
    assert!(config::load(dir.path()).is_err());
}

#[test]
fn automatic_installation_can_be_disabled_without_rewriting_settings() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    let bytes = b"{\"theme\":\"gruvbox\",\"lsp\":{\"auto_install\":false}}\n";
    fs::write(&path, bytes).unwrap();
    let config = config::load(&path).unwrap();
    assert!(!config.lsp.auto_install);
    assert_eq!(config.theme, ThemeName::Gruvbox);
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn editor_preferences_preserve_theme_lsp_and_user_bytes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    for enabled in [false, true] {
        let text = format!(
            r#"{{"theme":"catppuccin","editor":{{"relative_line_numbers":{enabled}}},"lsp":{{"auto_install":false}}}}"#
        );
        fs::write(&path, &text).unwrap();
        let settings = config::load(&path).unwrap();
        assert_eq!(settings.editor.relative_line_numbers, enabled);
        assert_eq!(settings.theme, ThemeName::Catppuccin);
        assert!(!settings.lsp.auto_install);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
    for text in ["{}", r#"{"editor":{}}"#, r#"{"theme":"gruvbox"}"#] {
        fs::write(&path, text).unwrap();
        assert!(!config::load(&path).unwrap().editor.relative_line_numbers);
    }
}

#[test]
fn invalid_relative_number_type_identifies_file_and_json_location() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    fs::write(
        &path,
        r#"{
  "editor": {"relative_line_numbers": "yes"}
}"#,
    )
    .unwrap();
    let error = config::load(&path).unwrap_err().to_string();
    assert!(error.contains(&path.display().to_string()));
    assert!(error.contains("expected a boolean"), "{error}");
    assert!(error.contains("line 2 column"), "{error}");
}
