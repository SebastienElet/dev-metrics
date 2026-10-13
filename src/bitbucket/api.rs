use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, LOCATION};
use reqwest::{Url, redirect, retry};
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::error::Error;
use std::time::Duration;

pub(super) struct Api {
    base: Url,
    client: Client,
}

impl Api {
    pub fn new(mut base: Url, token: &str) -> Result<Self, Box<dyn Error>> {
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("URL API Bitbucket invalide".into());
        }
        if token.is_empty()
            || token
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err("Token Bitbucket invalide".into());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "Token Bitbucket invalide")?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let client = Client::builder()
            .user_agent("dev-metrics")
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(redirect::Policy::none())
            .retry(retry::never())
            .build()?;
        Ok(Self { base, client })
    }

    fn checked_url(&self, url: Url) -> Result<Url, Box<dyn Error>> {
        if url.origin() != self.base.origin()
            || !url.path().starts_with(self.base.path())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err("Lien Bitbucket hors de l'API autorisée".into());
        }
        Ok(url)
    }

    pub fn get<T: DeserializeOwned>(&self, endpoint: &str) -> Result<T, Box<dyn Error>> {
        let (body, _) = self.request(self.base.join(endpoint)?)?;
        Ok(body)
    }

    fn request<T: DeserializeOwned>(&self, mut url: Url) -> Result<(T, Url), Box<dyn Error>> {
        let mut visited = BTreeSet::new();
        for _ in 0..=5 {
            url = self.checked_url(url)?;
            if !visited.insert(url.to_string()) {
                return Err("Redirection Bitbucket cyclique".into());
            }
            let response = self.client.get(url.clone()).send().map_err(|error| {
                if error.is_timeout() {
                    "Bitbucket : timeout"
                } else {
                    "Bitbucket : échec réseau"
                }
            })?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .ok_or("Redirection Bitbucket sans destination")?
                    .to_str()
                    .map_err(|_| "Redirection Bitbucket invalide")?;
                url = url
                    .join(location)
                    .map_err(|_| "Redirection Bitbucket invalide")?;
                continue;
            }
            if !response.status().is_success() {
                return Err(format!(
                    "Bitbucket : HTTP {} (vérifiez le token, les permissions et les limites API)",
                    response.status().as_u16()
                )
                .into());
            }
            return Ok((
                response.json().map_err(|_| "Réponse Bitbucket invalide")?,
                url,
            ));
        }
        Err("Trop de redirections Bitbucket".into())
    }

    pub fn list<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<T>, Box<dyn Error>> {
        let mut url = self.base.join(endpoint)?;
        url.query_pairs_mut()
            .append_pair("pagelen", "100")
            .extend_pairs(query.iter().copied());
        let mut visited = BTreeSet::new();
        let mut expected_size = None;
        let mut items = Vec::new();
        loop {
            if !visited.insert(url.to_string()) {
                return Err("Pagination Bitbucket cyclique".into());
            }
            let (page, effective): (Page<T>, _) = self.request(url)?;
            if let Some(size) = page.size {
                if expected_size.is_some_and(|previous| previous != size) {
                    return Err("Taille Bitbucket modifiée pendant la pagination".into());
                }
                expected_size = Some(size);
            }
            if page.values.is_empty() && page.next.is_some() {
                return Err("Pagination Bitbucket vide avec une page suivante".into());
            }
            items.extend(page.values);
            if expected_size.is_some_and(|size| items.len() > size) {
                return Err("Pagination Bitbucket incohérente".into());
            }
            match page.next {
                Some(next) => {
                    url = self.checked_url(
                        effective
                            .join(&next)
                            .map_err(|_| "Lien de pagination Bitbucket invalide")?,
                    )?
                }
                None => {
                    if expected_size.is_some_and(|size| size != items.len()) {
                        return Err("Pagination Bitbucket incomplète".into());
                    }
                    return Ok(items);
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct Page<T> {
    values: Vec<T>,
    next: Option<String>,
    size: Option<usize>,
}
