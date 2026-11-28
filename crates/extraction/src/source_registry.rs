//! Source registry defining allowed origins, paths, acquisition modes, and retention.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use url::Url;

/// Permitted acquisition channel for a registered source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionMode {
    /// RSS 2.0 or Atom feed.
    Feed,
    /// Documented JSON/REST API.
    Api,
    /// Direct HTTP GET of an HTML or sitemap resource.
    HttpGet,
}

/// Retention rule for snapshots retrieved from a registered source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetentionMode {
    /// Retain normalized text and evidence spans.
    #[default]
    Full,
    /// Retain attempt metadata only; withhold normalized text and quotes.
    MetadataOnly,
}

/// Policy entry for a single permitted web source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SourcePolicy {
    /// Stable identifier for this source.
    pub source_id: String,
    /// Exact HTTPS origin (`https://host[:port]`).
    pub origin: String,
    /// Allowed URL path prefixes (e.g. `["/rss", "/item"]`).
    pub allowed_paths: Vec<String>,
    /// Permitted acquisition modes.
    pub permitted_modes: Vec<AcquisitionMode>,
    /// Maximum requests allowed in one batch.
    pub max_requests: usize,
    /// Minimum delay between requests in seconds.
    pub crawl_delay_seconds: u64,
    /// Content retention policy.
    #[serde(default)]
    pub retention: RetentionMode,
}

/// Registry of permitted web sources (`schema_version: 1`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SourceRegistry {
    /// Must be `1`.
    pub schema_version: u32,
    /// Registered source policies.
    pub sources: Vec<SourcePolicy>,
}

/// Rejection reason from checking a URL against a [`SourceRegistry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// Unsupported `schema_version`.
    UnsupportedSchema(u32),
    /// Non-HTTPS scheme or embedded credentials.
    UnsafeUrl,
    /// No registered source matched the URL origin and path prefix.
    UnregisteredTarget,
    /// The matched source does not permit the requested acquisition mode.
    ModeNotPermitted,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(v) => write!(f, "unsupported source registry schema {v}"),
            Self::UnsafeUrl => f.write_str("source URL must use HTTPS without credentials"),
            Self::UnregisteredTarget => {
                f.write_str("target URL is not permitted by the source registry")
            }
            Self::ModeNotPermitted => {
                f.write_str("acquisition mode is not permitted for this source")
            }
        }
    }
}

impl Error for RegistryError {}

impl SourceRegistry {
    /// Validates and parses a JSON source registry.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON parsing fails or `schema_version != 1`.
    pub fn from_json(raw: &str) -> Result<Self, Box<dyn Error>> {
        let registry: Self = serde_json::from_str(raw)?;
        if registry.schema_version != 1 {
            return Err(Box::new(RegistryError::UnsupportedSchema(
                registry.schema_version,
            )));
        }
        Ok(registry)
    }

    /// Checks whether `url` is allowed for `mode` and returns its [`SourcePolicy`].
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] if the URL is unsafe, unregistered, or disallows `mode`.
    pub fn check(&self, url: &Url, mode: AcquisitionMode) -> Result<&SourcePolicy, RegistryError> {
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(RegistryError::UnsafeUrl);
        }
        let origin = url.origin().ascii_serialization();
        let path = url.path();
        let policy = self
            .sources
            .iter()
            .find(|entry| {
                entry.origin == origin
                    && entry
                        .allowed_paths
                        .iter()
                        .any(|prefix| path.starts_with(prefix))
            })
            .ok_or(RegistryError::UnregisteredTarget)?;
        if !policy.permitted_modes.contains(&mode) {
            return Err(RegistryError::ModeNotPermitted);
        }
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_origin_path_mode_and_https() {
        let registry = SourceRegistry::from_json(
            r#"{
                "schema_version": 1,
                "sources": [{
                    "source_id": "hn",
                    "origin": "https://news.ycombinator.com",
                    "allowed_paths": ["/rss", "/news"],
                    "permitted_modes": ["feed"],
                    "max_requests": 10,
                    "crawl_delay_seconds": 30,
                    "retention": "full"
                }]
            }"#,
        )
        .unwrap();

        let ok_url = Url::parse("https://news.ycombinator.com/rss").unwrap();
        assert_eq!(
            registry
                .check(&ok_url, AcquisitionMode::Feed)
                .unwrap()
                .source_id,
            "hn"
        );
        assert_eq!(
            registry.check(&ok_url, AcquisitionMode::HttpGet),
            Err(RegistryError::ModeNotPermitted)
        );
        let bad_path = Url::parse("https://news.ycombinator.com/item?id=1").unwrap();
        assert_eq!(
            registry.check(&bad_path, AcquisitionMode::Feed),
            Err(RegistryError::UnregisteredTarget)
        );
        let http_url = Url::parse("http://news.ycombinator.com/rss").unwrap();
        assert_eq!(
            registry.check(&http_url, AcquisitionMode::Feed),
            Err(RegistryError::UnsafeUrl)
        );
    }
}
