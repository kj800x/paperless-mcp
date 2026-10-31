use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const API_ACCEPT: &str = "application/json; version=9";
const LOOKUP_TTL: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub enum PaperlessError {
    Unauthorized,
    Forbidden,
    NotFound,
    Other(String),
}

impl std::fmt::Display for PaperlessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaperlessError::Unauthorized => {
                write!(f, "Paperless rejected the API token (401)")
            }
            PaperlessError::Forbidden => write!(
                f,
                "Paperless denied the request (403): this user lacks permission for that action"
            ),
            PaperlessError::NotFound => write!(f, "Not found"),
            PaperlessError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NamedObject {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub document_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CustomField {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Page<T> {
    pub count: i64,
    pub results: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct UiSettings {
    user: UiUser,
}

#[derive(Debug, Deserialize)]
struct UiUser {
    username: String,
}

/// Name/id lookup tables, scoped to what a single token can see.
pub struct Lookups {
    pub username: String,
    pub tags: Vec<NamedObject>,
    pub correspondents: Vec<NamedObject>,
    pub document_types: Vec<NamedObject>,
    pub custom_fields: Vec<CustomField>,
}

impl Lookups {
    pub fn tag_name(&self, id: i64) -> String {
        name_for(&self.tags, id)
    }

    pub fn correspondent_name(&self, id: i64) -> String {
        name_for(&self.correspondents, id)
    }

    pub fn document_type_name(&self, id: i64) -> String {
        name_for(&self.document_types, id)
    }

    pub fn custom_field_name(&self, id: i64) -> String {
        self.custom_fields
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| format!("field #{}", id))
    }
}

fn name_for(objects: &[NamedObject], id: i64) -> String {
    objects
        .iter()
        .find(|o| o.id == id)
        .map(|o| o.name.clone())
        .unwrap_or_else(|| format!("#{}", id))
}

/// Shared across requests. Holds no credentials of its own; every call is made
/// with the caller's token.
pub struct Paperless {
    http: reqwest::Client,
    base_url: String,
    pub public_url: String,
    // Keyed by the caller's Authorization header, since visibility differs per user.
    lookup_cache: Mutex<HashMap<String, (Instant, Arc<Lookups>)>>,
}

impl Paperless {
    pub fn new(base_url: String, public_url: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
            public_url: public_url.trim_end_matches('/').to_string(),
            lookup_cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        auth: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, PaperlessError> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.http.get(&url).query(query), auth).await
    }

    pub async fn patch<T: DeserializeOwned>(
        &self,
        auth: &str,
        path: &str,
        body: &Value,
    ) -> Result<T, PaperlessError> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.http.patch(&url).json(body), auth).await
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        auth: &str,
        path: &str,
        body: &Value,
    ) -> Result<T, PaperlessError> {
        let url = format!("{}{}", self.base_url, path);
        self.send(self.http.post(&url).json(body), auth).await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        auth: &str,
    ) -> Result<T, PaperlessError> {
        let response = request
            .header("Authorization", auth)
            .header("Accept", API_ACCEPT)
            .send()
            .await
            .map_err(|e| PaperlessError::Other(format!("Request to Paperless failed: {}", e)))?;

        match response.status().as_u16() {
            200..=299 => response.json::<T>().await.map_err(|e| {
                PaperlessError::Other(format!("Unexpected response from Paperless: {}", e))
            }),
            401 => Err(PaperlessError::Unauthorized),
            403 => Err(PaperlessError::Forbidden),
            404 => Err(PaperlessError::NotFound),
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(PaperlessError::Other(format!(
                    "Paperless returned HTTP {}: {}",
                    status,
                    truncate(&body, 500)
                )))
            }
        }
    }

    async fn get_all<T: DeserializeOwned>(
        &self,
        auth: &str,
        path: &str,
    ) -> Result<Vec<T>, PaperlessError> {
        let page: Page<T> = self
            .get(auth, path, &[("page_size", "100000".to_string())])
            .await?;
        Ok(page.results)
    }

    pub async fn document(&self, auth: &str, id: i64) -> Result<Value, PaperlessError> {
        self.get(auth, &format!("/api/documents/{}/", id), &[])
            .await
    }

    pub async fn lookups(&self, auth: &str) -> Result<Arc<Lookups>, PaperlessError> {
        {
            let cache = self.lookup_cache.lock().unwrap();
            if let Some((fetched, lookups)) = cache.get(auth)
                && fetched.elapsed() < LOOKUP_TTL
            {
                return Ok(lookups.clone());
            }
        }

        let (settings, tags, correspondents, document_types, custom_fields) = tokio::try_join!(
            self.get::<UiSettings>(auth, "/api/ui_settings/", &[]),
            self.get_all::<NamedObject>(auth, "/api/tags/"),
            self.get_all::<NamedObject>(auth, "/api/correspondents/"),
            self.get_all::<NamedObject>(auth, "/api/document_types/"),
            self.get_all::<CustomField>(auth, "/api/custom_fields/"),
        )?;

        let lookups = Arc::new(Lookups {
            username: settings.user.username,
            tags,
            correspondents,
            document_types,
            custom_fields,
        });

        let mut cache = self.lookup_cache.lock().unwrap();
        cache.retain(|_, (fetched, _)| fetched.elapsed() < LOOKUP_TTL);
        cache.insert(auth.to_string(), (Instant::now(), lookups.clone()));
        Ok(lookups)
    }

    pub fn document_url(&self, id: i64) -> String {
        format!("{}/documents/{}/details", self.public_url, id)
    }
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_string(),
    }
}
