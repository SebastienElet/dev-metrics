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
        vec!["github", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
            .args(arguments)
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).contains("Usage:"));
        assert!(stdout(&output).contains("Usage: dev-metrics"));
        assert!(stdout(&output).contains("--config"));
    }
}

#[test]
fn github_requires_credentials_without_gh_or_configured_token() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
        .args([
            "github",
            "--from",
            "2026-10-01",
            "--to",
            "2026-10-07",
            "--user",
            "octocat",
            "--config",
            "config.example.toml",
        ])
        .env("PATH", directory.path())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains("Authentification GitHub absente"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn placeholders_load_explicit_config_without_home() {
    for command in ["report", "schedule"] {
        let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
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
    let directory = home.path().join(".config/dev-metrics");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("config.toml"), "output_dir = 'weekly-notes'").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
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
    let directory = home.path().join(".config/dev-metrics");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("config.toml"), "invalid TOML").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
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
        let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
        .arg("report")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--config"));
}

#[test]
fn github_rejects_missing_dates_invalid_periods_and_unknown_formats() {
    for arguments in [
        vec!["github"],
        vec![
            "github",
            "--from",
            "2026-10-07",
            "--to",
            "2026-10-01",
            "--config",
            "config.example.toml",
        ],
        vec![
            "github",
            "--from",
            "2026-10-01",
            "--to",
            "2026-10-07",
            "--format",
            "yaml",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
            .args(arguments)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(stdout(&output).is_empty());
    }
}
