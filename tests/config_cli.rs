use std::path::Path;
use std::process::{Command, Output, Stdio};

fn command(path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dev-metrics"));
    command.arg("--config").arg(path).arg("config");
    for name in [
        "DEV_METRICS_GITHUB_TOKEN",
        "DEV_METRICS_BITBUCKET_USERNAME",
        "DEV_METRICS_BITBUCKET_TOKEN",
        "DEV_METRICS_BITBUCKET_WORKSPACE",
        "BITBUCKET_TOKEN",
    ] {
        command.env_remove(name);
    }
    command
}

fn run(path: &Path, args: &[&str]) -> Output {
    command(path)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn error(output: &Output) -> serde_json::Value {
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).unwrap()
}

#[test]
fn non_interactive_init_creates_private_parseable_config_and_refuses_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/config.toml");
    let first = run(
        &path,
        &[
            "init",
            "--non-interactive",
            "--github-token",
            "fake-github-token",
            "--bitbucket-username",
            "alice",
            "--bitbucket-workspace",
            "team",
            "--metrics-excluded-files",
            "*.snap,generated/*",
            "--output-dir",
            "reports",
        ],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    let config = dev_metrics::config::Config::parse(&saved).unwrap();
    assert_eq!(
        config.github.unwrap().token.as_deref(),
        Some("fake-github-token")
    );
    assert_eq!(config.bitbucket.unwrap().workspace.as_deref(), Some("team"));
    assert!(saved.contains("excluded_files"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let second = run(&path, &["init", "--non-interactive"]);
    assert_eq!(error(&second)["code"], "CONFIGURATION_ERROR");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    let forced = run(&path, &["init", "--non-interactive", "--force"]);
    assert!(forced.status.success());
    assert!(dev_metrics::config::Config::parse(&std::fs::read_to_string(&path).unwrap()).is_ok());
}

#[test]
fn set_get_unset_preserve_other_keys_and_mask_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        "output_dir = 'original'\n[bitbucket]\nworkspace = 'team'\n",
    )
    .unwrap();
    assert!(
        run(&path, &["set", "github.token", "fake-secret"])
            .status
            .success()
    );
    assert_eq!(
        json(&run(&path, &["get", "github.token", "--format", "json"]))["value"],
        "[masqué]"
    );
    assert_eq!(
        json(&run(
            &path,
            &["get", "github.token", "--show-secrets", "--format", "json"]
        ))["value"],
        "fake-secret"
    );
    let list = json(&run(&path, &["list", "--format", "json"]));
    assert_eq!(list["github.token"]["value"], "[masqué]");
    assert_eq!(list["bitbucket.workspace"]["value"], "team");
    assert_eq!(list["bitbucket.workspace"]["source"], "file");
    assert!(run(&path, &["unset", "github.token"]).status.success());
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        dev_metrics::config::Config::parse(&saved)
            .unwrap()
            .output_dir
            .unwrap(),
        Path::new("original")
    );
    assert!(saved.contains("workspace = \"team\""));
    assert_eq!(
        json(&run(&path, &["get", "github.token", "--format", "json"]))["source"],
        "unset"
    );
}

#[test]
fn environment_precedes_file_and_legacy_bitbucket_token_is_supported() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "[github]\ntoken='file-secret'\n[bitbucket]\ntoken='file-bitbucket'\nworkspace='file-team'\n").unwrap();
    let output = command(&path)
        .args(["list", "--format", "json"])
        .env("DEV_METRICS_GITHUB_TOKEN", "env-secret")
        .env("DEV_METRICS_BITBUCKET_WORKSPACE", "env-team")
        .env("BITBUCKET_TOKEN", "legacy-secret")
        .output()
        .unwrap();
    let values = json(&output);
    assert_eq!(values["github.token"]["source"], "env");
    assert_eq!(values["github.token"]["value"], "[masqué]");
    assert_eq!(values["bitbucket.workspace"]["value"], "env-team");
    assert_eq!(values["bitbucket.token"]["source"], "env");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret"));
}

#[test]
fn invalid_keys_values_and_file_return_config_error_without_secret_echo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    for args in [
        vec!["set", "unknown", "value"],
        vec!["set", "github.token", "bad secret"],
        vec!["set", "bitbucket.workspace", "bad/path"],
        vec!["get", "unknown"],
        vec!["unset", "unknown"],
    ] {
        let output = run(&path, &args);
        let details = error(&output);
        assert_eq!(details["code"], "CONFIGURATION_ERROR");
        assert!(details["details"].is_object());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("bad secret"));
    }
    std::fs::write(&path, "output_dir = 42").unwrap();
    assert_eq!(
        error(&run(&path, &["list", "--format", "json"]))["code"],
        "CONFIGURATION_ERROR"
    );
}

#[test]
fn path_json_does_not_require_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent.toml");
    let output = run(&path, &["path", "--format", "json"]);
    assert!(output.status.success());
    assert_eq!(json(&output)["path"], path.display().to_string());
}

#[test]
fn interactive_init_requires_confirmation_and_accepts_answers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "output_dir='original'").unwrap();
    let refused = command(&path)
        .arg("init")
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(error(&refused)["code"], "CONFIGURATION_ERROR");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "output_dir='original'"
    );
    let mut child = command(&path)
        .arg("init")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let input = child.stdin.as_mut().unwrap();
        input
            .write_all(b"yes\nfake-token\nalice\n\nteam\n*.snap\nreports\n")
            .unwrap();
    }
    let accepted = child.wait_with_output().unwrap();
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let config =
        dev_metrics::config::Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(config.github.unwrap().token.as_deref(), Some("fake-token"));
    assert_eq!(config.bitbucket.unwrap().username.as_deref(), Some("alice"));
}

#[test]
fn rewrite_restricts_existing_file_and_explicit_env_precedes_legacy() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "[bitbucket]\ntoken='file-token'").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    assert!(
        run(&path, &["set", "bitbucket.workspace", "team"])
            .status
            .success()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let output = command(&path)
        .args([
            "get",
            "bitbucket.token",
            "--show-secrets",
            "--format",
            "json",
        ])
        .env("DEV_METRICS_BITBUCKET_TOKEN", "new-token")
        .env("BITBUCKET_TOKEN", "legacy-token")
        .output()
        .unwrap();
    assert_eq!(json(&output)["value"], "new-token");
    assert_eq!(json(&output)["source"], "env");
}

#[cfg(unix)]
#[test]
fn interactive_token_input_is_not_echoed_to_the_terminal() {
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::new("script");
    #[cfg(target_os = "macos")]
    command.args([
        "-qF",
        "/dev/null",
        env!("CARGO_BIN_EXE_dev-metrics"),
        "--config",
        "config.toml",
        "config",
        "init",
        "--bitbucket-username",
        "alice",
        "--bitbucket-token",
        "fake-bitbucket-token",
        "--bitbucket-workspace",
        "team",
        "--metrics-excluded-files",
        "",
    ]);
    #[cfg(not(target_os = "macos"))]
    command.args(["-q", "-f", "-c", &format!("{} --config config.toml config init --bitbucket-username alice --bitbucket-token fake-bitbucket-token --bitbucket-workspace team --metrics-excluded-files ''", env!("CARGO_BIN_EXE_dev-metrics")), "/dev/null"]);
    let mut child = command
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut captured = Vec::new();
        let token_prompt = b"github.token (vide pour ignorer) : ";
        let output_prompt = b"output_dir (vide pour ignorer) : ";
        let mut announced = 0;
        let mut byte = [0u8; 1];
        while output.read(&mut byte).unwrap_or(0) == 1 {
            captured.push(byte[0]);
            if announced == 0 && captured.ends_with(token_prompt) {
                sender.send(1).unwrap();
                announced = 1;
            } else if announced == 1 && captured.ends_with(output_prompt) {
                sender.send(2).unwrap();
                announced = 2;
            }
        }
        captured
    });
    if receiver.recv_timeout(Duration::from_secs(5)) != Ok(1) {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("prompt absent");
    }
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"fake-github-secret\n").unwrap();
    if receiver.recv_timeout(Duration::from_secs(5)) != Ok(2) {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("prompt suivant absent");
    }
    input.write_all(b"reports\n").unwrap();
    let status = child.wait().unwrap();
    drop(input);
    let captured = reader.join().unwrap();
    assert!(
        status.success(),
        "status={status} output={}",
        String::from_utf8_lossy(&captured)
    );
    assert!(
        !String::from_utf8_lossy(&captured).contains("fake-github-secret"),
        "{}",
        String::from_utf8_lossy(&captured)
    );
    assert!(String::from_utf8_lossy(&captured).contains("reports"));
    let path = directory.path().join("config.toml");
    assert_eq!(
        dev_metrics::config::Config::load(&path)
            .unwrap()
            .github
            .unwrap()
            .token
            .as_deref(),
        Some("fake-github-secret")
    );
}

#[cfg(unix)]
#[test]
fn writes_through_a_symlink_without_replacing_it() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("managed/config.toml");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "[bitbucket]\nworkspace='first'\n").unwrap();
    let link = directory.path().join("config.toml");
    symlink("managed/config.toml", &link).unwrap();
    assert!(
        run(&link, &["set", "bitbucket.workspace", "second"])
            .status
            .success()
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        dev_metrics::config::Config::load(&target)
            .unwrap()
            .bitbucket
            .unwrap()
            .workspace
            .as_deref(),
        Some("second")
    );
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        run(&link, &["unset", "bitbucket.workspace"])
            .status
            .success()
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        dev_metrics::config::Config::load(&target)
            .unwrap()
            .bitbucket
            .unwrap()
            .workspace,
        None
    );
    assert!(
        run(
            &link,
            &[
                "init",
                "--non-interactive",
                "--force",
                "--bitbucket-workspace",
                "third"
            ]
        )
        .status
        .success()
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        dev_metrics::config::Config::load(&target)
            .unwrap()
            .bitbucket
            .unwrap()
            .workspace
            .as_deref(),
        Some("third")
    );

    let dangling = directory.path().join("dangling.toml");
    symlink("missing.toml", &dangling).unwrap();
    let failure = run(&dangling, &["init", "--non-interactive", "--force"]);
    assert_eq!(error(&failure)["code"], "CONFIGURATION_ERROR");
    assert!(
        std::fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!directory.path().join("missing.toml").exists());
}

#[test]
fn get_rejects_invalid_value_already_in_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "[bitbucket]\nworkspace='bad/path'").unwrap();
    assert_eq!(
        error(&run(
            &path,
            &["get", "bitbucket.workspace", "--format", "json"]
        ))["code"],
        "CONFIGURATION_ERROR"
    );
    assert_eq!(
        error(&run(&path, &["list", "--format", "json"]))["code"],
        "CONFIGURATION_ERROR"
    );
}

#[test]
fn unrelated_environment_does_not_block_github() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dev-metrics"))
        .args(["github", "--from", "2026-10-01", "--to", "2026-10-07"])
        .arg("--config")
        .arg(&path)
        .env("DEV_METRICS_BITBUCKET_USERNAME", "")
        .env_remove("DEV_METRICS_GITHUB_TOKEN")
        .env("PATH", directory.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
}
