use std::process::Command;

use tempfile::tempdir;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_editor-pro"))
}

#[test]
fn informational_options_work_without_a_terminal_or_valid_config() {
    let home = tempdir().unwrap();
    let output = binary()
        .arg("--config-path")
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert!(output.status.success());
    let path = home.path().join(if cfg!(target_os = "macos") {
        "Library/Application Support/editor-pro/config.json"
    } else {
        ".config/editor-pro/config.json"
    });
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        path.to_str().unwrap()
    );
    assert!(!path.exists());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "invalid json").unwrap();
    for flag in ["--version", "-V", "--help", "-h"] {
        let output = binary()
            .arg(flag)
            .env("HOME", home.path())
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{flag}: {output:?}");
        assert!(output.stderr.is_empty());
        if matches!(flag, "--help" | "-h") {
            assert!(String::from_utf8_lossy(&output.stdout).contains("F1 or :help [QUERY]"));
        }
        if matches!(flag, "--version" | "-V") {
            assert_eq!(
                output.stdout,
                format!("editor-pro {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
            );
        }
    }
    let output = binary()
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(path.to_str().unwrap()));
    assert!(!output.stdout.contains(&0x1b));
}

#[cfg(unix)]
#[test]
fn ep_symlink_reports_the_package_version() {
    let dir = tempdir().unwrap();
    let ep = dir.path().join("ep");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_editor-pro"), &ep).unwrap();
    let output = Command::new(ep).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        format!("editor-pro {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
}

#[test]
fn a_directory_cannot_be_mixed_with_file_arguments() {
    let dir = tempdir().unwrap();
    let output = binary().arg(dir.path()).arg("file.rs").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Pass one directory"));
    assert!(output.stdout.is_empty());
}
