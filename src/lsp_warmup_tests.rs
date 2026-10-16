use super::*;

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn detect(root: &Path) -> Vec<ServerKey> {
    discover(&Config::default(), root, &AtomicBool::new(false))
}

#[test]
fn gradle_dsl_does_not_imply_kotlin_and_modules_share_the_settings_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(&root, ".git", "gitdir: elsewhere");
    write(
        &root,
        "settings.gradle.kts",
        "include(\"app\", \"library\")",
    );
    write(&root, "app/build.gradle.kts", "plugins { java }");
    write(
        &root,
        "library/build.gradle.kts",
        "plugins { `java-library` }",
    );
    fs::create_dir_all(root.join("app/src/main/java/probe")).unwrap();
    assert_eq!(
        detect(&root.join("app/src/main/java/probe")),
        vec![ServerKey {
            root: root.clone(),
            language: Language::Java
        }]
    );
    write(&root, "app/build.gradle.kts", "plugins { kotlin(\"jvm\") }");
    assert_eq!(detect(&root).len(), 2);
    fs::remove_dir_all(root.join("app/src")).unwrap();
    fs::remove_dir_all(root.join("library")).unwrap();
    assert_eq!(
        detect(&root),
        vec![ServerKey {
            root,
            language: Language::Kotlin
        }]
    );
}

#[test]
fn unrecognized_and_script_only_directories_remain_lazy() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".git", "");
    for file in ["Main.kt", "Main.java", "Main.cs"] {
        write(dir.path(), file, "");
        assert!(detect(dir.path()).is_empty(), "{file}");
        fs::remove_file(dir.path().join(file)).unwrap();
    }
    for file in ["build.gradle.kts", "settings.gradle.kts"] {
        write(dir.path(), file, "");
        assert!(detect(dir.path()).is_empty(), "{file}");
    }
}

#[test]
fn nearest_maven_modules_and_nested_repository_boundaries_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(&root, ".git", "");
    write(
        &root,
        "pom.xml",
        "<project><packaging>pom</packaging></project>",
    );
    write(&root, "java/pom.xml", "<project/>");
    write(
        &root,
        "kotlin/pom.xml",
        "<artifactId>kotlin-maven-plugin</artifactId>",
    );
    write(&root, "nested/.git", "");
    write(&root, "nested/Project.csproj", "");
    let keys = detect(&root);
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&ServerKey {
        root: root.join("java"),
        language: Language::Java
    }));
    assert!(keys.contains(&ServerKey {
        root: root.join("kotlin"),
        language: Language::Kotlin
    }));
    assert_eq!(
        detect(&root.join("nested")),
        vec![ServerKey {
            root: root.join("nested"),
            language: Language::CSharp
        }]
    );
}

#[test]
fn solution_roots_ambiguity_and_explicit_overrides_match_lazy_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(&root, ".git", "");
    write(&root, "Project.slnx", "");
    write(&root, "App/App.csproj", "");
    write(&root, "Library/Library.csproj", "");
    assert_eq!(
        detect(&root),
        vec![ServerKey {
            root: root.clone(),
            language: Language::CSharp
        }]
    );
    write(&root, "Other.sln", "");
    assert!(detect(&root).is_empty());
    let mut config = Config {
        root: Some(root.clone()),
        ..Config::default()
    };
    assert_eq!(
        discover(&config, &root, &AtomicBool::new(false)),
        vec![ServerKey {
            root: root.clone(),
            language: Language::CSharp
        }]
    );
    config.root = Some(root.join("App"));
    assert_eq!(
        discover(&config, &root, &AtomicBool::new(false)),
        vec![ServerKey {
            root: root.join("App"),
            language: Language::CSharp
        }]
    );
}

#[test]
fn broad_polyglot_projects_and_scan_limits_fall_back_to_demand() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".git", "");
    write(dir.path(), "pom.xml", "<project/>");
    write(dir.path(), "kotlin/pom.xml", "kotlin-maven-plugin");
    write(dir.path(), "dotnet/Project.csproj", "");
    assert!(detect(dir.path()).is_empty());
    fs::remove_dir_all(dir.path().join("dotnet")).unwrap();
    assert_eq!(detect(dir.path()).len(), 2);
    for index in 0..MAX_ENTRIES {
        write(dir.path(), &format!("file-{index}"), "");
    }
    assert!(detect(dir.path()).is_empty());
}

#[test]
fn cancellation_and_ignored_or_deep_builds_do_not_produce_candidates() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".git", "");
    for name in [
        "build/pom.xml",
        ".cache/pom.xml",
        "node_modules/pom.xml",
        "a/b/c/pom.xml",
    ] {
        write(dir.path(), name, "<project/>");
    }
    assert!(detect(dir.path()).is_empty());
    write(dir.path(), "pom.xml", "<project/>");
    assert!(discover(&Config::default(), dir.path(), &AtomicBool::new(true)).is_empty());
    assert_eq!(detect(dir.path()).len(), 1);
}

#[cfg(unix)]
#[test]
fn directory_symlinks_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(dir.path(), ".git", "");
    write(outside.path(), "pom.xml", "<project/>");
    std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();
    assert!(detect(dir.path()).is_empty());
}
