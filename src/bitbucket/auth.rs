use std::error::Error;
use std::ffi::OsString;

pub fn resolve_workspace<'a>(
    explicit: Option<&'a str>,
    configured: Option<&'a str>,
) -> Result<&'a str, Box<dyn Error>> {
    let workspace = explicit
        .or(configured)
        .ok_or("Workspace Bitbucket absent ; indiquez --workspace ou [bitbucket].workspace")?;
    super::validate_workspace(workspace)?;
    Ok(workspace)
}

pub fn resolve_token(
    environment: Option<OsString>,
    configured: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    let token = match environment {
        Some(value) => value.into_string().map_err(|_| "Token Bitbucket invalide")?,
        None => configured.ok_or("Authentification Bitbucket absente ; configurez BITBUCKET_TOKEN ou [bitbucket].token")?.into(),
    };
    if token.is_empty() || !token.bytes().all(|c| c.is_ascii_graphic()) {
        return Err("Token Bitbucket invalide".into());
    }
    Ok(token)
}
