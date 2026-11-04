use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::GatewayError;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub tavily: TavilyConfig,
    pub firecrawl: FirecrawlConfig,
    pub searxng: SearxngConfig,
    pub search: SearchConfig,
    pub fetch: FetchConfig,
    pub map: MapConfig,
    pub keyring: KeyringConfig,
    pub circuit: CircuitConfig,
    pub cache: CacheConfig,
    pub rate: RateConfig,
    pub doctor: DoctorConfig,
    pub log: LogConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TavilyConfig {
    pub enabled: bool,
    pub base_url: String,
    pub keys: Vec<String>,
    pub search_depth: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FirecrawlConfig {
    pub enabled: bool,
    pub base_url: String,
    pub keys: Vec<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SearxngConfig {
    pub enabled: bool,
    pub base_url: String,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub server_timeout_s: u64,
    pub categories: Vec<String>,
    pub language: String,
    pub max_results: usize,
    /// Engines `doctor` expects to be enabled. Empty disables the check.
    pub required_engines: Vec<String>,
    /// Engines `doctor` expects to be disabled. Empty disables the check.
    pub disabled_engines: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub budget_ms: u64,
    pub default_max_results: usize,
    /// Characters kept from each source description in search responses.
    pub snippet_max_chars: usize,
    /// Whole-response character budget for `web_search` and each
    /// `get_sources` page. Sources removed by the budget stay in the cache.
    pub response_max_chars: usize,
    /// Time held back from paid stages so the SearXNG last resort can run.
    pub reserve_last_resort_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FetchConfig {
    pub budget_ms: u64,
    pub max_chars_default: usize,
    pub max_chars_limit: usize,
    pub specialist_timeout_ms: u64,
    pub generic_timeout_ms: u64,
    pub firecrawl_timeout_ms: u64,
    /// Allow `web_fetch` to reach loopback/private/link-local hosts.
    pub allow_private_hosts: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MapConfig {
    pub budget_ms: u64,
    pub default_max_results: usize,
    pub max_results_limit: usize,
    /// Whole-response character budget for `web_map`. URLs removed by the
    /// budget are counted in `omitted_urls`.
    pub response_max_chars: usize,
    pub tavily_timeout_ms: u64,
    pub firecrawl_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeyringConfig {
    pub auth_cooldown_secs: u64,
    pub quota_cooldown_secs: u64,
    pub rate_limit_cooldown_secs: u64,
    pub transport_retry: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CircuitConfig {
    pub failure_threshold: u32,
    pub cooldown_secs: u64,
    pub misconfig_cooldown_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub max_sessions: usize,
    pub ttl_secs: u64,
    pub max_sources_per_session: usize,
    /// Maximum number of paginated `web_fetch` pages kept in memory.
    pub max_pages: usize,
    /// TTL for paginated `web_fetch` pages, independent of search TTL.
    pub page_ttl_secs: u64,
    /// Persist search sessions under the configuration directory.
    pub persist: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RateConfig {
    /// Outbound tool calls allowed per rolling minute. Zero disables the limit.
    pub max_calls_per_minute: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DoctorConfig {
    /// Free URL fetched by `doctor` with live=true. It must not require an API
    /// key; paid provider probes are controlled separately.
    pub fetch_probe_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    pub level: String,
    pub file: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tavily: TavilyConfig::default(),
            firecrawl: FirecrawlConfig::default(),
            searxng: SearxngConfig::default(),
            search: SearchConfig::default(),
            fetch: FetchConfig::default(),
            map: MapConfig::default(),
            keyring: KeyringConfig::default(),
            circuit: CircuitConfig::default(),
            cache: CacheConfig::default(),
            rate: RateConfig::default(),
            doctor: DoctorConfig::default(),
            log: LogConfig::default(),
        }
    }
}

impl Default for TavilyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://api.tavily.com".into(),
            keys: Vec::new(),
            search_depth: "basic".into(),
            timeout_ms: 10_000,
        }
    }
}

impl Default for FirecrawlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://api.firecrawl.dev".into(),
            keys: Vec::new(),
            timeout_ms: 10_000,
        }
    }
}

impl Default for SearxngConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "http://127.0.0.1:12310".into(),
            connect_timeout_ms: 2_000,
            request_timeout_ms: 8_000,
            server_timeout_s: 6,
            categories: vec!["general".into()],
            language: "auto".into(),
            max_results: 10,
            required_engines: vec![
                "google cse".into(),
                "bing".into(),
                "360search".into(),
            ],
            disabled_engines: vec![
                "brave".into(),
                "duckduckgo".into(),
                "startpage".into(),
            ],
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            budget_ms: 30_000,
            default_max_results: 5,
            snippet_max_chars: 800,
            response_max_chars: 30_000,
            reserve_last_resort_ms: 8_000,
        }
    }
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            budget_ms: 30_000,
            max_chars_default: 12_000,
            max_chars_limit: 200_000,
            specialist_timeout_ms: 10_000,
            generic_timeout_ms: 8_000,
            firecrawl_timeout_ms: 12_000,
            allow_private_hosts: false,
        }
    }
}

impl Default for MapConfig {
    fn default() -> Self {
        Self {
            budget_ms: 20_000,
            default_max_results: 50,
            max_results_limit: 200,
            response_max_chars: 30_000,
            tavily_timeout_ms: 10_000,
            firecrawl_timeout_ms: 10_000,
        }
    }
}

impl Default for KeyringConfig {
    fn default() -> Self {
        Self {
            auth_cooldown_secs: 600,
            quota_cooldown_secs: 3_600,
            rate_limit_cooldown_secs: 60,
            transport_retry: 1,
        }
    }
}

impl Default for CircuitConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            cooldown_secs: 300,
            misconfig_cooldown_secs: 600,
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_sessions: 32,
            ttl_secs: 600,
            max_sources_per_session: 200,
            max_pages: 64,
            page_ttl_secs: 600,
            persist: true,
        }
    }
}

impl Default for RateConfig {
    fn default() -> Self {
        Self {
            max_calls_per_minute: 60,
        }
    }
}

impl Default for DoctorConfig {
    fn default() -> Self {
        Self {
            fetch_probe_url: "https://example.com/".into(),
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: false,
        }
    }
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

impl Config {
    /// Strict load for the MCP server: a missing default config is an error and
    /// at least one provider must be usable.
    pub fn load(config_path: Option<&Path>) -> Result<LoadedConfig, GatewayError> {
        Self::load_internal(config_path, false, true)
    }

    /// Lenient load for `--doctor` and `--selftest`: a missing default config
    /// yields defaults, and an unusable provider set is reported by doctor.
    pub fn load_lenient(config_path: Option<&Path>) -> Result<LoadedConfig, GatewayError> {
        Self::load_internal(config_path, true, false)
    }

    fn load_internal(
        config_path: Option<&Path>,
        lenient_missing: bool,
        require_provider: bool,
    ) -> Result<LoadedConfig, GatewayError> {
        let env_path = env::var("SEARCH_GATEWAY_CONFIG")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let explicit = config_path.is_some() || env_path.is_some();
        let path = config_path
            .map(PathBuf::from)
            .or_else(|| env_path.map(PathBuf::from))
            .unwrap_or_else(default_config_path);

        let mut warnings = Vec::new();
        let mut config = if path.exists() {
            let text = fs::read_to_string(&path).map_err(|error| {
                GatewayError::config(format!("failed to read {}: {error}", path.display()))
            })?;
            toml::from_str::<Config>(&text).map_err(|error| {
                GatewayError::config(format!("failed to parse {}: {error}", path.display()))
            })?
        } else {
            if explicit || !lenient_missing {
                return Err(GatewayError::config(format!(
                    "config file not found: {}\ncreate it with:\n{}",
                    path.display(),
                    sample_config()
                )));
            }
            warnings.push(format!(
                "config file not found at {}; using built-in defaults",
                path.display()
            ));
            Config::default()
        };

        config.tavily.keys = resolve_keys(
            &config.tavily.keys,
            "TAVILY_API_KEY",
            "tavily",
            &mut warnings,
        );
        config.firecrawl.keys = resolve_keys(
            &config.firecrawl.keys,
            "FIRECRAWL_API_KEY",
            "firecrawl",
            &mut warnings,
        );
        config.validate(require_provider)?;

        Ok(LoadedConfig {
            config,
            path: if path.exists() { Some(path) } else { None },
            warnings,
        })
    }

    fn validate(&self, require_provider: bool) -> Result<(), GatewayError> {
        validate_base_url("tavily.base_url", &self.tavily.base_url)?;
        validate_base_url("firecrawl.base_url", &self.firecrawl.base_url)?;
        validate_base_url("searxng.base_url", &self.searxng.base_url)?;
        validate_base_url("doctor.fetch_probe_url", &self.doctor.fetch_probe_url)?;

        if !matches!(self.tavily.search_depth.as_str(), "basic" | "advanced") {
            return Err(GatewayError::config(
                "tavily.search_depth must be either \"basic\" or \"advanced\"",
            ));
        }
        if !(1..=50).contains(&self.search.default_max_results) {
            return Err(GatewayError::config(
                "search.default_max_results must be between 1 and 50",
            ));
        }
        if !(100..=2_000).contains(&self.search.snippet_max_chars) {
            return Err(GatewayError::config(
                "search.snippet_max_chars must be between 100 and 2000",
            ));
        }
        if self.search.response_max_chars < 1_000 {
            return Err(GatewayError::config(
                "search.response_max_chars must be at least 1000",
            ));
        }
        if self.search.snippet_max_chars > self.search.response_max_chars {
            return Err(GatewayError::config(
                "search.snippet_max_chars must not exceed search.response_max_chars",
            ));
        }
        if self.search.budget_ms == 0 || self.fetch.budget_ms == 0 || self.map.budget_ms == 0 {
            return Err(GatewayError::config("tool budgets must be greater than zero"));
        }
        if self.search.reserve_last_resort_ms == 0 {
            return Err(GatewayError::config(
                "search.reserve_last_resort_ms must be greater than zero",
            ));
        }
        if self.tavily.timeout_ms == 0 || self.firecrawl.timeout_ms == 0 {
            return Err(GatewayError::config(
                "provider timeouts must be greater than zero",
            ));
        }
        if self.searxng.connect_timeout_ms == 0
            || self.searxng.request_timeout_ms == 0
            || self.searxng.server_timeout_s == 0
        {
            return Err(GatewayError::config(
                "searxng timeouts must be greater than zero",
            ));
        }
        if self.fetch.max_chars_default == 0 || self.fetch.max_chars_limit == 0 {
            return Err(GatewayError::config(
                "fetch.max_chars_default and fetch.max_chars_limit must be greater than zero",
            ));
        }
        if self.fetch.max_chars_default > self.fetch.max_chars_limit {
            return Err(GatewayError::config(
                "fetch.max_chars_default must not exceed fetch.max_chars_limit",
            ));
        }
        if self.fetch.specialist_timeout_ms == 0
            || self.fetch.generic_timeout_ms == 0
            || self.fetch.firecrawl_timeout_ms == 0
        {
            return Err(GatewayError::config(
                "fetch stage timeouts must be greater than zero",
            ));
        }
        if self.map.default_max_results == 0 || self.map.max_results_limit == 0 {
            return Err(GatewayError::config(
                "map result limits must be greater than zero",
            ));
        }
        if self.map.default_max_results > self.map.max_results_limit {
            return Err(GatewayError::config(
                "map.default_max_results must not exceed map.max_results_limit",
            ));
        }
        if self.map.response_max_chars < 1_000 {
            return Err(GatewayError::config(
                "map.response_max_chars must be at least 1000",
            ));
        }
        if self.map.tavily_timeout_ms == 0 || self.map.firecrawl_timeout_ms == 0 {
            return Err(GatewayError::config(
                "map stage timeouts must be greater than zero",
            ));
        }
        if self.keyring.transport_retry > 1 {
            return Err(GatewayError::config(
                "keyring.transport_retry must be 0 or 1",
            ));
        }
        if self.keyring.auth_cooldown_secs == 0
            || self.keyring.quota_cooldown_secs == 0
            || self.keyring.rate_limit_cooldown_secs == 0
        {
            return Err(GatewayError::config(
                "keyring cooldowns must be greater than zero",
            ));
        }
        if self.circuit.failure_threshold == 0 {
            return Err(GatewayError::config(
                "circuit.failure_threshold must be at least 1",
            ));
        }
        if self.circuit.cooldown_secs == 0 || self.circuit.misconfig_cooldown_secs == 0 {
            return Err(GatewayError::config(
                "circuit cooldowns must be greater than zero",
            ));
        }
        if self.searxng.max_results == 0 {
            return Err(GatewayError::config(
                "searxng.max_results must be greater than zero",
            ));
        }
        if self.cache.ttl_secs == 0
            || self.cache.max_sessions == 0
            || self.cache.max_sources_per_session == 0
            || self.cache.max_pages == 0
            || self.cache.page_ttl_secs == 0
        {
            return Err(GatewayError::config(
                "cache sizes and TTLs must be greater than zero",
            ));
        }

        if require_provider && !self.has_usable_provider() {
            return Err(GatewayError::config(
                "no usable provider: set tavily.keys or firecrawl.keys, or enable searxng",
            ));
        }
        Ok(())
    }

    pub fn has_usable_provider(&self) -> bool {
        (self.tavily.enabled && !self.tavily.keys.is_empty())
            || (self.firecrawl.enabled && !self.firecrawl.keys.is_empty())
            || self.searxng.enabled
    }

    pub fn tavily_usable(&self) -> bool {
        self.tavily.enabled && !self.tavily.keys.is_empty()
    }

    pub fn firecrawl_usable(&self) -> bool {
        self.firecrawl.enabled && !self.firecrawl.keys.is_empty()
    }
}

fn resolve_keys(
    raw: &[String],
    legacy_env: &str,
    provider: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    for entry in raw {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let value = if let Some(name) = entry.strip_prefix("env:") {
            match env::var(name) {
                Ok(value) if !value.trim().is_empty() => value,
                _ => {
                    warnings.push(format!(
                        "{provider} key source env:{name} is missing or empty"
                    ));
                    continue;
                }
            }
        } else {
            entry.to_string()
        };
        push_key_values(&value, &mut keys, &mut seen);
    }

    if keys.is_empty() {
        if let Ok(value) = env::var(legacy_env) {
            push_key_values(&value, &mut keys, &mut seen);
        }
    }
    keys
}

/// Accepts one key per entry as well as comma/semicolon/whitespace separated
/// lists, matching the existing key-rotator environment format.
fn push_key_values(raw: &str, keys: &mut Vec<String>, seen: &mut HashSet<String>) {
    for value in raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if seen.insert(value.to_string()) {
            keys.push(value.to_string());
        }
    }
}

fn validate_base_url(field: &str, value: &str) -> Result<(), GatewayError> {
    let url = url::Url::parse(value)
        .map_err(|error| GatewayError::config(format!("{field} is not a valid URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(GatewayError::config(format!(
            "{field} must use http or https"
        )));
    }
    Ok(())
}

pub fn default_config_path() -> PathBuf {
    if let Ok(appdata) = env::var("APPDATA") {
        return PathBuf::from(appdata)
            .join("search-gateway")
            .join("config.toml");
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("search-gateway")
            .join("config.toml");
    }
    PathBuf::from("config.toml")
}

pub fn sample_config() -> &'static str {
    r#"# %APPDATA%\search-gateway\config.toml
[tavily]
enabled = true
base_url = "https://api.tavily.com"
keys = ["env:TAVILY_API_KEY_1", "env:TAVILY_API_KEY_2"]
search_depth = "basic"
timeout_ms = 10000

[firecrawl]
enabled = true
base_url = "https://api.firecrawl.dev"
keys = ["env:FIRECRAWL_API_KEY_1"]
timeout_ms = 10000

[searxng]
enabled = true
base_url = "http://127.0.0.1:12310"
connect_timeout_ms = 2000
request_timeout_ms = 8000
server_timeout_s = 6
categories = ["general"]
language = "auto"
max_results = 10
required_engines = ["google cse", "bing", "360search"]
disabled_engines = ["brave", "duckduckgo", "startpage"]

[search]
budget_ms = 30000
default_max_results = 5
snippet_max_chars = 800
response_max_chars = 30000
reserve_last_resort_ms = 8000

[fetch]
budget_ms = 30000
max_chars_default = 12000
max_chars_limit = 200000
specialist_timeout_ms = 10000
generic_timeout_ms = 8000
firecrawl_timeout_ms = 12000
allow_private_hosts = false

[map]
budget_ms = 20000
default_max_results = 50
max_results_limit = 200
response_max_chars = 30000

[keyring]
auth_cooldown_secs = 600
quota_cooldown_secs = 3600
rate_limit_cooldown_secs = 60
transport_retry = 1

[circuit]
failure_threshold = 3
cooldown_secs = 300
misconfig_cooldown_secs = 600

[cache]
max_sessions = 32
ttl_secs = 600
max_sources_per_session = 200
max_pages = 64
page_ttl_secs = 600
persist = true

[rate]
max_calls_per_minute = 60

[doctor]
fetch_probe_url = "https://example.com/"

[log]
level = "info"
file = false
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_separated_key_lists_and_deduplicates() {
        let mut keys = Vec::new();
        let mut seen = HashSet::new();
        push_key_values("tvly-a, tvly-b;tvly-c\ntvly-a", &mut keys, &mut seen);
        assert_eq!(keys, vec!["tvly-a", "tvly-b", "tvly-c"]);
    }

    #[test]
    fn rejects_zero_valued_resource_limits() {
        let mut config = Config::default();
        config.cache.max_pages = 0;
        assert!(config.validate(false).is_err());

        let mut config = Config::default();
        config.search.reserve_last_resort_ms = 0;
        assert!(config.validate(false).is_err());

        let mut config = Config::default();
        config.map.response_max_chars = 0;
        assert!(config.validate(false).is_err());

        let mut config = Config::default();
        config.fetch.max_chars_limit = 0;
        assert!(config.validate(false).is_err());

        let mut config = Config::default();
        config.circuit.cooldown_secs = 0;
        assert!(config.validate(false).is_err());
    }

    #[test]
    fn rejects_an_invalid_doctor_probe_url() {
        let mut config = Config::default();
        config.doctor.fetch_probe_url = "file:///etc/passwd".into();
        assert!(config.validate(false).is_err());
    }
}
