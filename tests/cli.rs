use std::process::{Command, Output};

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn help_works_without_home_or_config() {
    for arguments in [
        vec!["--help"],
        vec!["report", "--help"],
        vec!["schedule", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_devmetrics"))
            .args(arguments)
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).contains("Usage:"));
        assert!(stdout(&output).contains("--config"));
    }
}

#[test]
fn placeholders_load_explicit_config_without_home() {
    for command in ["report", "schedule"] {
        let output = Command::new(env!("CARGO_BIN_EXE_devmetrics"))
            .args(["--config", "config.example.toml", command])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).contains("non implémenté"));
        assert!(stdout(&output).contains("./reports"));
    }
}

#[test]
fn loads_default_config_under_home() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/devmetrics");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("config.toml"), "output_dir = 'weekly-notes'").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devmetrics"))
        .arg("report")
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("weekly-notes"));
}

#[test]
fn config_override_ignores_invalid_default_file() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".config/devmetrics");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("config.toml"), "invalid TOML").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devmetrics"))
        .args(["report", "--config", "config.example.toml"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("./reports"));
}

#[test]
fn config_failures_return_nonzero_without_placeholder_output() {
    let directory = tempfile::tempdir().unwrap();
    for (filename, content) in [
        ("missing.toml", None),
        ("invalid.toml", Some("output_dir = 42")),
    ] {
        let path = directory.path().join(filename);
        if let Some(content) = content {
            std::fs::write(&path, content).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_devmetrics"))
            .arg("report")
            .arg("--config")
            .arg(&path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains(&path.display().to_string()));
    }
}

#[test]
fn missing_home_requires_explicit_config() {
    let output = Command::new(env!("CARGO_BIN_EXE_devmetrics"))
        .arg("report")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--config"));
}
