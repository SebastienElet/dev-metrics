use std::error::Error;
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn resolve_token(
    gh_program: &OsStr,
    configured: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    if let Some(token) = gh_token(gh_program) {
        return Ok(token);
    }
    configured.and_then(valid_token).map(str::to_owned)
        .ok_or_else(|| "Authentification GitHub absente : gh auth token indisponible et aucun token valide dans [github]".into())
}

fn valid_token(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.bytes().all(|c| c.is_ascii_graphic())).then_some(value)
}

fn gh_token(program: &OsStr) -> Option<String> {
    let mut capture = tempfile::tempfile().ok()?;
    let mut child = Command::new(program)
        .args(["auth", "token", "--hostname", "github.com"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(capture.try_clone().ok()?))
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                if capture.metadata().ok()?.len() > 65536 {
                    return None;
                }
                capture.seek(SeekFrom::Start(0)).ok()?;
                let mut value = String::new();
                capture.take(65536).read_to_string(&mut value).ok()?;
                return valid_token(&value).map(str::to_owned);
            }
            Ok(None) if start.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
