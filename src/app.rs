use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::cache::SearchCache;
use crate::config::{Config, LoadedConfig};
use crate::doctor::DoctorEngine;
use crate::error::GatewayError;
use crate::fetch::FetchEngine;
use crate::keyring::Keyring;
use crate::map::MapEngine;
use crate::providers::{
    self, FirecrawlClient, SearxngClient, TavilyClient,
};
use crate::rate::RateLimiter;
use crate::search::SearchEngine;

pub struct App {
    pub config: Arc<Config>,
    pub search: SearchEngine,
    pub fetch: FetchEngine,
    pub map: MapEngine,
    pub doctor: DoctorEngine,
    pub cache: Arc<SearchCache>,
}

fn session_path(config_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = config_path {
        return path.parent().map(|parent| parent.join("sessions.json"));
    }
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join("search-gateway").join("sessions.json"))
}

impl App {
    pub fn new(loaded: LoadedConfig) -> Result<Self, GatewayError> {
        Self::build(loaded, true)
    }

    /// Builds an app even when no provider is usable, so `doctor` can report
    /// the misconfiguration instead of failing before it runs.
    pub fn new_lenient(loaded: LoadedConfig) -> Result<Self, GatewayError> {
        Self::build(loaded, false)
    }

    fn build(loaded: LoadedConfig, require_provider: bool) -> Result<Self, GatewayError> {
        let cfg = Arc::new(loaded.config);
        if require_provider && !cfg.has_usable_provider() {
            return Err(GatewayError::config(
                "no usable provider: set tavily.keys or firecrawl.keys, or enable searxng",
            ));
        }

        let http = providers::new_http_client(2_000)?;
        let tavily = cfg.tavily_usable().then(|| {
            let keyring = Arc::new(Mutex::new(Keyring::new(
                cfg.tavily.keys.clone(),
                cfg.keyring.clone(),
            )));
            Arc::new(TavilyClient::new(
                &cfg.tavily,
                keyring,
                cfg.circuit.clone(),
                http.clone(),
            ))
        });
        let firecrawl = cfg.firecrawl_usable().then(|| {
            let keyring = Arc::new(Mutex::new(Keyring::new(
                cfg.firecrawl.keys.clone(),
                cfg.keyring.clone(),
            )));
            Arc::new(FirecrawlClient::new(
                &cfg.firecrawl,
                keyring,
                cfg.circuit.clone(),
                http.clone(),
            ))
        });
        let searxng = cfg.searxng.enabled.then(|| {
            Arc::new(SearxngClient::new(
                &cfg.searxng,
                cfg.circuit.clone(),
                http.clone(),
            ))
        });

        let persist_path = cfg
            .cache
            .persist
            .then(|| session_path(loaded.path.as_deref()))
            .flatten();
        let cache = Arc::new(SearchCache::new(cfg.cache.clone(), persist_path));
        let limiter = Arc::new(RateLimiter::new(cfg.rate.max_calls_per_minute));
        let search = SearchEngine::new(
            cfg.clone(),
            tavily.clone(),
            firecrawl.clone(),
            searxng.clone(),
            cache.clone(),
            limiter.clone(),
        );
        let fetch_http = providers::new_fetch_client(2_000)?;
        let fetch = FetchEngine::new(
            cfg.clone(),
            fetch_http,
            firecrawl.clone(),
            limiter.clone(),
        );
        let map = MapEngine::new(cfg.clone(), tavily.clone(), firecrawl.clone(), limiter);
        let doctor = DoctorEngine::new(
            cfg.clone(),
            loaded.path,
            loaded.warnings,
            tavily,
            firecrawl,
            searxng,
            http,
        );

        Ok(Self {
            config: cfg,
            search,
            fetch,
            map,
            doctor,
            cache,
        })
    }
}
