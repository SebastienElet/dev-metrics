#![cfg(unix)]

use dev_metrics::github::resolve_token;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;

fn gh_script(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("gh");
    std::fs::write(&program, format!("#!/bin/sh\n{contents}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    (directory, program)
}

#[test]
fn gh_token_takes_priority_over_configuration_and_selects_github_com() {
    let (_directory, program) = gh_script(
        "test \"$*\" = 'auth token --hostname github.com' || exit 1\nprintf ' gh-token\\n'",
    );
    assert_eq!(
        resolve_token(program.as_os_str(), Some("configured-token")).unwrap(),
        "gh-token"
    );
}

#[test]
fn falls_back_when_gh_is_missing_fails_or_returns_empty_or_non_utf8_output() {
    assert_eq!(
        resolve_token(
            OsStr::new("/nonexistent/dev-metrics-gh"),
            Some("configured-token")
        )
        .unwrap(),
        "configured-token"
    );
    for script in [
        "printf 'secret-error' >&2; exit 1",
        "printf ' \\n'",
        "printf '\\377'",
    ] {
        let (_directory, program) = gh_script(script);
        assert_eq!(
            resolve_token(program.as_os_str(), Some("configured-token")).unwrap(),
            "configured-token"
        );
    }
}

#[test]
fn fails_without_credentials_and_never_echoes_token_material() {
    for configured in [None, Some(" "), Some("secret\nvalue")] {
        let result = resolve_token(OsStr::new("/nonexistent/dev-metrics-gh"), configured);
        assert!(result.is_err());
        assert!(!result.unwrap_err().to_string().contains("secret"));
    }
}

#[test]
fn falls_back_when_gh_hangs() {
    let (_directory, program) = gh_script("exec /bin/sleep 60");
    let start = std::time::Instant::now();
    assert_eq!(
        resolve_token(program.as_os_str(), Some("configured-token")).unwrap(),
        "configured-token"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn does_not_wait_for_a_descendant_inheriting_stdout() {
    let (_directory, program) = gh_script("/bin/sleep 8 &\nprintf gh-token");
    let start = std::time::Instant::now();
    assert_eq!(
        resolve_token(program.as_os_str(), Some("configured-token")).unwrap(),
        "gh-token"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(6));
}

#[test]
fn falls_back_when_gh_returns_an_excessively_large_token() {
    let (_directory, program) = gh_script(&format!("printf '{}'", "a".repeat(65537)));
    assert_eq!(
        resolve_token(program.as_os_str(), Some("configured-token")).unwrap(),
        "configured-token"
    );
}
