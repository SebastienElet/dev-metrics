use super::{CheckFormat, config_error};
use crate::cli::error::{CommandError, ErrorCode};
use dev_metrics::config::{Config, environment_value, validate_setting};
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::{redirect, retry};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ok,
    Failed,
    Skipped,
}

#[derive(Serialize)]
struct SourceCheck {
    status: Status,
    probe: Option<&'static str>,
    required_permission: Option<&'static str>,
    message: &'static str,
}

impl SourceCheck {
    fn ok() -> Self {
        Self {
            status: Status::Ok,
            probe: None,
            required_permission: None,
            message: "Accès vérifié sur un dépôt",
        }
    }

    fn skipped() -> Self {
        Self {
            status: Status::Skipped,
            probe: None,
            required_permission: None,
            message: "Token absent",
        }
    }

    fn failed(
        probe: &'static str,
        required_permission: Option<&'static str>,
        message: &'static str,
    ) -> Self {
        Self {
            status: Status::Failed,
            probe: Some(probe),
            required_permission,
            message,
        }
    }
}

#[derive(Serialize)]
pub struct CheckReport {
    github: SourceCheck,
    bitbucket: SourceCheck,
}

impl CheckReport {
    pub fn success(&self) -> bool {
        let checks = [&self.github.status, &self.bitbucket.status];
        checks.iter().any(|status| matches!(status, Status::Ok))
            && checks
                .iter()
                .all(|status| !matches!(status, Status::Failed))
    }

    pub fn render(&self, format: CheckFormat) -> Result<String, CommandError> {
        match format {
            CheckFormat::Json => serde_json::to_string(self).map_err(|_| {
                CommandError::new(
                    ErrorCode::OutputError,
                    "Impossible de sérialiser le résultat",
                )
            }),
            CheckFormat::Table => Ok(format!(
                "Source\tÉtat\tDétail\nGitHub\t{}\t{}\nBitbucket\t{}\t{}",
                label(&self.github.status),
                detail(&self.github),
                label(&self.bitbucket.status),
                detail(&self.bitbucket)
            )),
        }
    }
}

fn label(status: &Status) -> &'static str {
    match status {
        Status::Ok => "OK",
        Status::Failed => "ÉCHEC",
        Status::Skipped => "IGNORÉ",
    }
}

fn detail(check: &SourceCheck) -> String {
    match check.required_permission {
        Some(permission) => format!(
            "{} : {} (droit requis : {permission})",
            check.probe.unwrap_or("accès"),
            check.message
        ),
        None => check.message.into(),
    }
}

pub fn run(path: &Path) -> Result<CheckReport, CommandError> {
    run_with_bases(
        path,
        &"https://api.github.com/".parse().unwrap(),
        &"https://api.bitbucket.org/2.0/".parse().unwrap(),
    )
}

fn run_with_bases(
    path: &Path,
    github_base: &Url,
    bitbucket_base: &Url,
) -> Result<CheckReport, CommandError> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            return Err(config_error(format!(
                "Impossible de lire {}",
                path.display()
            )));
        }
    };
    let config = Config::parse(&source)
        .map_err(|_| config_error(format!("Configuration invalide dans {}", path.display())))?;
    let github_token = environment_value("github.token")
        .map_err(config_error)?
        .or_else(|| config.github.and_then(|github| github.token));
    let bitbucket_token = environment_value("bitbucket.token")
        .map_err(config_error)?
        .or_else(|| {
            config
                .bitbucket
                .as_ref()
                .and_then(|bitbucket| bitbucket.token.clone())
        });
    let bitbucket_email = environment_value("bitbucket.email")
        .map_err(config_error)?
        .or_else(|| {
            config
                .bitbucket
                .as_ref()
                .and_then(|bitbucket| bitbucket.email.clone())
        });
    let bitbucket_workspace = environment_value("bitbucket.workspace")
        .map_err(config_error)?
        .or_else(|| config.bitbucket.and_then(|bitbucket| bitbucket.workspace));
    let client = Client::builder()
        .user_agent("dev-metrics")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(redirect::Policy::none())
        .retry(retry::never())
        .build()
        .map_err(|_| config_error("Impossible de préparer les vérifications HTTP"))?;
    let github = match github_token {
        None => SourceCheck::skipped(),
        Some(token) if validate_setting("github.token", &token).is_err() => {
            SourceCheck::failed("token", None, "Token GitHub invalide")
        }
        Some(token) => probe_github(&client, github_base, &token),
    };
    let bitbucket = match bitbucket_token {
        None => SourceCheck::skipped(),
        Some(token) if validate_setting("bitbucket.token", &token).is_err() => {
            SourceCheck::failed("token", None, "Token Bitbucket invalide")
        }
        Some(_)
            if bitbucket_email
                .as_deref()
                .is_some_and(|email| validate_setting("bitbucket.email", email).is_err()) =>
        {
            SourceCheck::failed("email", None, "Email Bitbucket invalide")
        }
        Some(_)
            if bitbucket_workspace
                .as_deref()
                .is_none_or(|value| validate_setting("bitbucket.workspace", value).is_err()) =>
        {
            SourceCheck::failed("workspace", None, "Workspace Bitbucket absent ou invalide")
        }
        Some(token) => probe_bitbucket(
            &client,
            bitbucket_base,
            &token,
            bitbucket_email.as_deref(),
            bitbucket_workspace.as_deref().unwrap(),
        ),
    };
    Ok(CheckReport { github, bitbucket })
}

#[derive(Clone, Copy)]
enum Provider<'a> {
    Github,
    Bitbucket { email: Option<&'a str> },
}

fn request(
    client: &Client,
    base: &Url,
    provider: Provider<'_>,
    token: &str,
    endpoint: &str,
    probe: &'static str,
    required_permission: Option<&'static str>,
) -> Result<Value, SourceCheck> {
    let url = base
        .join(endpoint)
        .map_err(|_| SourceCheck::failed(probe, None, "URL API invalide"))?;
    if url.origin() != base.origin() || !url.path().starts_with(base.path()) {
        return Err(SourceCheck::failed(probe, None, "URL API invalide"));
    }
    let accept = match provider {
        Provider::Github => "application/vnd.github+json",
        Provider::Bitbucket { .. } => "application/json",
    };
    let request = client.get(url).header("Accept", accept);
    let mut request = match provider {
        Provider::Bitbucket { email: Some(email) } => request.basic_auth(email, Some(token)),
        _ => request.bearer_auth(token),
    };
    if matches!(provider, Provider::Github) {
        request = request.header("X-GitHub-Api-Version", "2026-03-10");
    }
    let response = request.send().map_err(|error| {
        SourceCheck::failed(
            probe,
            None,
            if error.is_timeout() {
                "Timeout API"
            } else {
                "Échec réseau"
            },
        )
    })?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(SourceCheck::failed(probe, None, "Token refusé"));
    }
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
        || (response.status() == reqwest::StatusCode::FORBIDDEN
            && (response
                .headers()
                .get("X-RateLimit-Remaining")
                .is_some_and(|value| value == "0")
                || response.headers().contains_key("Retry-After")))
    {
        return Err(SourceCheck::failed(probe, None, "Limite API atteinte"));
    }
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(SourceCheck::failed(
            probe,
            required_permission,
            "Accès refusé",
        ));
    }
    if !response.status().is_success() {
        return Err(SourceCheck::failed(probe, None, "Réponse HTTP inattendue"));
    }
    response
        .json()
        .map_err(|_| SourceCheck::failed(probe, None, "Réponse API invalide"))
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn repository_name<'a>(value: &'a Value, workspace: Option<&str>) -> Option<&'a str> {
    let full_name = value.get("full_name")?.as_str()?;
    let (owner, name) = full_name.split_once('/')?;
    if !valid_segment(owner)
        || !valid_segment(name)
        || workspace.is_some_and(|workspace| owner != workspace)
    {
        return None;
    }
    Some(full_name)
}

fn probe_github(client: &Client, base: &Url, token: &str) -> SourceCheck {
    let check = || -> Result<(), SourceCheck> {
        let user = request(client, base, Provider::Github, token, "user", "user", None)?;
        if user
            .get("login")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(SourceCheck::failed("user", None, "Réponse API invalide"));
        }
        let repos = request(
            client,
            base,
            Provider::Github,
            token,
            "user/repos?per_page=1",
            "repositories",
            None,
        )?;
        let repos = repos
            .as_array()
            .ok_or_else(|| SourceCheck::failed("repositories", None, "Réponse API invalide"))?;
        let repo = repos
            .first()
            .ok_or_else(|| SourceCheck::failed("repositories", None, "Aucun dépôt accessible"))?;
        let repo = repository_name(repo, None)
            .ok_or_else(|| SourceCheck::failed("repositories", None, "Réponse API invalide"))?;
        for (endpoint, probe, permission) in [
            (
                format!("repos/{repo}/commits?per_page=1"),
                "commits",
                Some("contents:read"),
            ),
            (
                format!("repos/{repo}/pulls?per_page=1"),
                "pull_requests",
                Some("pull_requests:read"),
            ),
        ] {
            let value = request(
                client,
                base,
                Provider::Github,
                token,
                &endpoint,
                probe,
                permission,
            )?;
            if !value.is_array() {
                return Err(SourceCheck::failed(probe, None, "Réponse API invalide"));
            }
        }
        Ok(())
    };
    match check() {
        Ok(()) => SourceCheck::ok(),
        Err(error) => error,
    }
}

fn probe_bitbucket(
    client: &Client,
    base: &Url,
    token: &str,
    email: Option<&str>,
    workspace: &str,
) -> SourceCheck {
    let check = || -> Result<(), SourceCheck> {
        let user = request(
            client,
            base,
            Provider::Bitbucket { email },
            token,
            "user",
            "user",
            Some("read:user:bitbucket"),
        )?;
        if user
            .get("uuid")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(SourceCheck::failed("user", None, "Réponse API invalide"));
        }
        let workspace_value = request(
            client,
            base,
            Provider::Bitbucket { email },
            token,
            &format!("workspaces/{workspace}"),
            "workspace",
            Some("read:workspace:bitbucket"),
        )?;
        if workspace_value.get("slug").and_then(Value::as_str) != Some(workspace) {
            return Err(SourceCheck::failed(
                "workspace",
                None,
                "Réponse API invalide",
            ));
        }
        let repos = request(
            client,
            base,
            Provider::Bitbucket { email },
            token,
            &format!("repositories/{workspace}?pagelen=1"),
            "repositories",
            Some("read:repository:bitbucket"),
        )?;
        let repos = repos
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| SourceCheck::failed("repositories", None, "Réponse API invalide"))?;
        let repo = repos
            .first()
            .ok_or_else(|| SourceCheck::failed("repositories", None, "Aucun dépôt accessible"))?;
        let repo = repository_name(repo, Some(workspace))
            .ok_or_else(|| SourceCheck::failed("repositories", None, "Réponse API invalide"))?;
        let pulls = request(
            client,
            base,
            Provider::Bitbucket { email },
            token,
            &format!("repositories/{repo}/pullrequests?pagelen=1"),
            "pull_requests",
            Some("read:pullrequest:bitbucket"),
        )?;
        if pulls.get("values").and_then(Value::as_array).is_none() {
            return Err(SourceCheck::failed(
                "pull_requests",
                None,
                "Réponse API invalide",
            ));
        }
        Ok(())
    };
    match check() {
        Ok(()) => SourceCheck::ok(),
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};
    use serde_json::json;

    fn endpoint(
        server: &mut Server,
        path: &str,
        token: &str,
        body: serde_json::Value,
    ) -> mockito::Mock {
        server
            .mock("GET", path)
            .match_query(Matcher::Any)
            .match_header("authorization", format!("Bearer {token}").as_str())
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create()
    }

    fn config(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, source).unwrap();
        (directory, path)
    }

    #[test]
    fn checks_user_repository_contents_and_pull_requests_for_both_sources() {
        let mut github = Server::new();
        let mut bitbucket = Server::new();
        let github_user = endpoint(
            &mut github,
            "/user",
            "fake-github-token",
            json!({"login":"octocat"}),
        );
        let github_repos = endpoint(
            &mut github,
            "/user/repos",
            "fake-github-token",
            json!([{"full_name":"octocat/project"}]),
        );
        let github_commits = endpoint(
            &mut github,
            "/repos/octocat/project/commits",
            "fake-github-token",
            json!([]),
        );
        let github_pulls = endpoint(
            &mut github,
            "/repos/octocat/project/pulls",
            "fake-github-token",
            json!([]),
        );
        let bitbucket_user = endpoint(
            &mut bitbucket,
            "/2.0/user",
            "fake-bitbucket-token",
            json!({"uuid":"{alice}"}),
        );
        let bitbucket_workspace = endpoint(
            &mut bitbucket,
            "/2.0/workspaces/team",
            "fake-bitbucket-token",
            json!({"slug":"team"}),
        );
        let bitbucket_repos = endpoint(
            &mut bitbucket,
            "/2.0/repositories/team",
            "fake-bitbucket-token",
            json!({"values":[{"full_name":"team/project"}]}),
        );
        let bitbucket_pulls = endpoint(
            &mut bitbucket,
            "/2.0/repositories/team/project/pullrequests",
            "fake-bitbucket-token",
            json!({"values":[]}),
        );
        let (_directory, path) = config(
            "[github]\ntoken='fake-github-token'\n[bitbucket]\ntoken='fake-bitbucket-token'\nworkspace='team'\n",
        );
        let report = run_with_bases(
            &path,
            &github.url().parse().unwrap(),
            &format!("{}/2.0/", bitbucket.url()).parse().unwrap(),
        )
        .unwrap();
        assert!(report.success());
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["github"]["status"], "ok");
        assert_eq!(value["bitbucket"]["status"], "ok");
        assert!(!value.to_string().contains("fake-"));
        for probe in [
            github_user,
            github_repos,
            github_commits,
            github_pulls,
            bitbucket_user,
            bitbucket_workspace,
            bitbucket_repos,
            bitbucket_pulls,
        ] {
            probe.assert();
        }
    }

    #[test]
    fn reports_the_permission_for_a_denied_github_contents_probe() {
        let mut github = Server::new();
        endpoint(
            &mut github,
            "/user",
            "fake-github-token",
            json!({"login":"octocat"}),
        );
        endpoint(
            &mut github,
            "/user/repos",
            "fake-github-token",
            json!([{"full_name":"octocat/project"}]),
        );
        let denied = github
            .mock("GET", "/repos/octocat/project/commits")
            .match_query(Matcher::Any)
            .match_header("authorization", "Bearer fake-github-token")
            .with_status(403)
            .with_body("fake-github-token")
            .create();
        let (_directory, path) = config("[github]\ntoken='fake-github-token'\n");
        let report = run_with_bases(
            &path,
            &github.url().parse().unwrap(),
            &"http://127.0.0.1:1/2.0/".parse().unwrap(),
        )
        .unwrap();
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["github"]["status"], "failed");
        assert_eq!(value["github"]["required_permission"], "contents:read");
        assert_eq!(value["github"]["probe"], "commits");
        assert!(!value.to_string().contains("fake-github-token"));
        denied.assert();
    }

    #[test]
    fn does_not_claim_missing_metadata_for_a_denied_repository_listing() {
        let mut github = Server::new();
        endpoint(
            &mut github,
            "/user",
            "fake-github-token",
            json!({"login":"octocat"}),
        );
        let denied = github
            .mock("GET", "/user/repos")
            .match_query(Matcher::Any)
            .match_header("authorization", "Bearer fake-github-token")
            .with_status(403)
            .create();
        let (_directory, path) = config("[github]\ntoken='fake-github-token'\n");
        let report = run_with_bases(
            &path,
            &github.url().parse().unwrap(),
            &"http://127.0.0.1:1/2.0/".parse().unwrap(),
        )
        .unwrap();
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["github"]["probe"], "repositories");
        assert_eq!(value["github"]["required_permission"], Value::Null);
        denied.assert();
    }

    #[test]
    fn reports_the_permission_for_a_denied_bitbucket_pull_request_probe() {
        let mut bitbucket = Server::new();
        endpoint(
            &mut bitbucket,
            "/2.0/user",
            "fake-bitbucket-token",
            json!({"uuid":"{alice}"}),
        );
        endpoint(
            &mut bitbucket,
            "/2.0/workspaces/team",
            "fake-bitbucket-token",
            json!({"slug":"team"}),
        );
        endpoint(
            &mut bitbucket,
            "/2.0/repositories/team",
            "fake-bitbucket-token",
            json!({"values":[{"full_name":"team/project"}]}),
        );
        let denied = bitbucket
            .mock("GET", "/2.0/repositories/team/project/pullrequests")
            .match_query(Matcher::Any)
            .match_header("authorization", "Bearer fake-bitbucket-token")
            .with_status(403)
            .with_body("fake-bitbucket-token")
            .create();
        let (_directory, path) =
            config("[bitbucket]\ntoken='fake-bitbucket-token'\nworkspace='team'\n");
        let report = run_with_bases(
            &path,
            &"http://127.0.0.1:1/".parse().unwrap(),
            &format!("{}/2.0/", bitbucket.url()).parse().unwrap(),
        )
        .unwrap();
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            value["bitbucket"]["required_permission"],
            "read:pullrequest:bitbucket"
        );
        assert_eq!(value["bitbucket"]["probe"], "pull_requests");
        assert!(!report.success());
        denied.assert();
    }

    #[test]
    fn identifies_each_bitbucket_scope_at_the_denied_probe() {
        for (denied_path, probe, permission) in [
            ("/2.0/user", "user", "read:user:bitbucket"),
            (
                "/2.0/workspaces/team",
                "workspace",
                "read:workspace:bitbucket",
            ),
            (
                "/2.0/repositories/team",
                "repositories",
                "read:repository:bitbucket",
            ),
        ] {
            let mut bitbucket = Server::new();
            if denied_path != "/2.0/user" {
                endpoint(
                    &mut bitbucket,
                    "/2.0/user",
                    "fake-bitbucket-token",
                    json!({"uuid":"{alice}"}),
                );
            }
            if denied_path == "/2.0/repositories/team" {
                endpoint(
                    &mut bitbucket,
                    "/2.0/workspaces/team",
                    "fake-bitbucket-token",
                    json!({"slug":"team"}),
                );
            }
            let denied = bitbucket
                .mock("GET", denied_path)
                .match_query(Matcher::Any)
                .match_header("authorization", "Bearer fake-bitbucket-token")
                .with_status(403)
                .create();
            let (_directory, path) =
                config("[bitbucket]\ntoken='fake-bitbucket-token'\nworkspace='team'\n");
            let report = run_with_bases(
                &path,
                &"http://127.0.0.1:1/".parse().unwrap(),
                &format!("{}/2.0/", bitbucket.url()).parse().unwrap(),
            )
            .unwrap();
            let value = serde_json::to_value(&report).unwrap();
            assert_eq!(value["bitbucket"]["probe"], probe);
            assert_eq!(value["bitbucket"]["required_permission"], permission);
            assert!(
                report
                    .render(CheckFormat::Table)
                    .unwrap()
                    .contains(permission)
            );
            denied.assert();
        }
    }

    #[test]
    fn distinguishes_rate_limits_from_missing_permissions() {
        let mut github = Server::new();
        let limited = github
            .mock("GET", "/user")
            .match_header("authorization", "Bearer fake-github-token")
            .with_status(403)
            .with_header("X-RateLimit-Remaining", "0")
            .create();
        let (_directory, path) = config("[github]\ntoken='fake-github-token'\n");
        let report = run_with_bases(
            &path,
            &github.url().parse().unwrap(),
            &"http://127.0.0.1:1/2.0/".parse().unwrap(),
        )
        .unwrap();
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["github"]["required_permission"], Value::Null);
        assert_eq!(value["github"]["message"], "Limite API atteinte");
        limited.assert();
    }

    #[test]
    fn rejects_unsafe_repository_names_and_empty_access_without_guessing_a_scope() {
        for repos in [json!([]), json!([{"full_name":"octocat/../other"}])] {
            let mut github = Server::new();
            endpoint(
                &mut github,
                "/user",
                "fake-github-token",
                json!({"login":"octocat"}),
            );
            endpoint(&mut github, "/user/repos", "fake-github-token", repos);
            let (_directory, path) = config("[github]\ntoken='fake-github-token'\n");
            let report = run_with_bases(
                &path,
                &github.url().parse().unwrap(),
                &"http://127.0.0.1:1/2.0/".parse().unwrap(),
            )
            .unwrap();
            let value = serde_json::to_value(&report).unwrap();
            assert_eq!(value["github"]["status"], "failed");
            assert_eq!(value["github"]["required_permission"], Value::Null);
            assert_eq!(value["github"]["probe"], "repositories");
        }
    }

    #[test]
    fn bitbucket_email_selects_basic_authentication_for_check() {
        let mut bitbucket = Server::new();
        let user = bitbucket
            .mock("GET", "/2.0/user")
            .match_header(
                "authorization",
                "Basic YWxpY2VAZXhhbXBsZS50ZXN0OmZha2UtYml0YnVja2V0LXRva2Vu",
            )
            .with_header("content-type", "application/json")
            .with_body(json!({"uuid":"{alice}"}).to_string())
            .create();
        let (_directory, path) = config(
            "[bitbucket]\nemail='alice@example.test'\ntoken='fake-bitbucket-token'\nworkspace='team'\n",
        );
        let report = run_with_bases(
            &path,
            &"http://127.0.0.1:1/".parse().unwrap(),
            &format!("{}/2.0/", bitbucket.url()).parse().unwrap(),
        )
        .unwrap();
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["bitbucket"]["probe"], "workspace");
        user.assert();
    }
}
