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
        vec!["schema", "--help"],
        vec!["github", "--help"],
        vec!["bitbucket", "--help"],
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
        assert!(stdout(&output).contains("Exemple"));
        assert!(stdout(&output).contains("Codes de sortie :"));
        assert!(output.stderr.is_empty());
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
    assert_eq!(output.status.code(), Some(4));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "AUTHENTICATION_ERROR");
    assert_eq!(error["details"]["source"], "github");
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains("Authentification GitHub absente"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn report_loads_explicit_config_without_home() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
        .args(["--config", "config.example.toml", "report"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("HOME")
        .env("PATH", directory.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("Authentification GitHub absente"));
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
        .env("PATH", home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
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
        .env("PATH", home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    assert!(stdout(&output).is_empty());
}

#[test]
fn config_failures_return_structured_errors_without_stdout() {
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
        assert_eq!(output.status.code(), Some(3));
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "CONFIGURATION_ERROR");
        assert!(error["details"].is_object());
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
    assert_eq!(output.status.code(), Some(3));
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

#[test]
fn bitbucket_resolves_workspace_and_token_without_printing_partial_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    for (config, args, env, expected) in [
        (
            "output_dir='reports'",
            vec![],
            None,
            "Workspace Bitbucket absent",
        ),
        (
            "output_dir='reports'\n[bitbucket]\nworkspace='team'",
            vec![],
            None,
            "Authentification Bitbucket absente",
        ),
        (
            "output_dir='reports'\n[bitbucket]\nworkspace='bad/path'",
            vec!["--workspace", "team"],
            None,
            "Authentification Bitbucket absente",
        ),
        (
            "output_dir='reports'\n[bitbucket]\nworkspace='team'\ntoken='configured-secret'",
            vec![],
            Some("invalid secret"),
            "Token Bitbucket invalide",
        ),
        (
            "output_dir='reports'\n[bitbucket]\nworkspace='team'",
            vec!["--workspace", "../bad"],
            Some("env-secret"),
            "Workspace Bitbucket invalide",
        ),
    ] {
        std::fs::write(&path, config).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dev-metrics"));
        command
            .args(["bitbucket", "--from", "2026-10-01", "--to", "2026-10-07"])
            .args(args)
            .arg("--config")
            .arg(&path)
            .env_remove("BITBUCKET_TOKEN");
        if let Some(token) = env {
            command.env("BITBUCKET_TOKEN", token);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(stdout(&output).is_empty());
        assert!(stderr(&output).contains(expected), "{}", stderr(&output));
        assert!(!stderr(&output).contains("secret"));
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        let expected_code = if expected.starts_with("Workspace") {
            "CONFIGURATION_ERROR"
        } else {
            "AUTHENTICATION_ERROR"
        };
        assert_eq!(error["code"], expected_code);
        assert_eq!(error["details"]["source"], "bitbucket");
        assert_eq!(
            output.status.code(),
            Some(if expected.starts_with("Workspace") {
                3
            } else {
                4
            })
        );
    }
}

#[test]
fn schema_is_available_without_configuration_and_matches_the_embedded_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
        .arg("schema")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        schema,
        serde_json::from_str::<serde_json::Value>(dev_metrics::metrics::REPORT_SCHEMA).unwrap()
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn argument_errors_are_json_on_stderr_with_exit_two_and_no_stdout() {
    for arguments in [
        vec![],
        vec!["unknown"],
        vec!["report", "--format", "yaml"],
        vec!["report", "--source", "unknown"],
        vec!["report", "--source", "bitbucket", "--github-user", "alice"],
        vec!["report", "--workspace", "team"],
        vec!["report", "--bitbucket-user", "{alice}"],
        vec!["report", "--until", "0000-01-15"],
        vec!["report", "--until", "2026-02-30"],
        vec!["report", "--since", "2026-10-30", "--until", "2026-10-01"],
        vec!["report", "--since", "0000-01-01", "--until", "0000-01-01"],
        vec!["report", "--until", "2026-10-30T12:00:00Z"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
            .args(&arguments)
            .env_remove("HOME")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["code"].as_str().unwrap().starts_with("INVALID_"));
        assert!(!error["message"].as_str().unwrap().is_empty());
        assert!(error["details"].is_object());
    }
}
