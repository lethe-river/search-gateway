use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::model::{
    CheckStatus, DoctorCheck, DoctorOverallStatus, DoctorResponse, ErrorCode, ProviderError,
};
use crate::providers::{FirecrawlClient, ProviderRequest, SearxngClient, TavilyClient};

pub struct DoctorEngine {
    cfg: Arc<Config>,
    config_path: Option<PathBuf>,
    warnings: Vec<String>,
    tavily: Option<Arc<TavilyClient>>,
    firecrawl: Option<Arc<FirecrawlClient>>,
    searxng: Option<Arc<SearxngClient>>,
    http: reqwest::Client,
}

impl DoctorEngine {
    pub fn new(
        cfg: Arc<Config>,
        config_path: Option<PathBuf>,
        warnings: Vec<String>,
        tavily: Option<Arc<TavilyClient>>,
        firecrawl: Option<Arc<FirecrawlClient>>,
        searxng: Option<Arc<SearxngClient>>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            cfg,
            config_path,
            warnings,
            tavily,
            firecrawl,
            searxng,
            http,
        }
    }

    pub async fn run(&self, live: bool) -> DoctorResponse {
        let mut checks = Vec::new();

        let mut config_detail = match &self.config_path {
            Some(path) => format!("loaded {}", path.display()),
            None => "using built-in defaults".to_string(),
        };
        if !self.warnings.is_empty() {
            config_detail.push_str("; ");
            config_detail.push_str(&self.warnings.join("; "));
        }
        let config_status = if self.cfg.has_usable_provider() {
            CheckStatus::Ok
        } else {
            CheckStatus::Misconfigured
        };
        checks.push(DoctorCheck {
            name: "config".into(),
            status: config_status,
            latency_ms: None,
            detail: config_detail,
        });

        if let Some(client) = &self.searxng {
            let health = client.health().await;
            let healthy = health.is_ok();
            checks.push(DoctorCheck {
                name: "searxng_health".into(),
                status: match &health {
                    Ok(_) => CheckStatus::Ok,
                    Err(error) => status_for_error(error.code),
                },
                latency_ms: health.as_ref().ok().map(|d| d.as_millis() as u64),
                detail: match &health {
                    Ok(_) => "GET /healthz returned 200".into(),
                    Err(error) => error.message.clone(),
                },
            });

            if healthy {
                match client.config().await {
                    Ok(config) => {
                        let mut problems = Vec::new();
                        for name in &self.cfg.searxng.required_engines {
                            match config.engines.iter().find(|engine| &engine.name == name) {
                                Some(engine) if engine.enabled.unwrap_or(false) => {}
                                Some(_) => problems.push(format!("{name} is disabled")),
                                None => problems.push(format!("{name} is missing")),
                            }
                        }
                        for name in &self.cfg.searxng.disabled_engines {
                            match config.engines.iter().find(|engine| &engine.name == name) {
                                Some(engine) if engine.enabled.unwrap_or(false) => {
                                    problems.push(format!("{name} is enabled"));
                                }
                                _ => {}
                            }
                        }
                        checks.push(DoctorCheck {
                            name: "searxng_engines".into(),
                            status: if problems.is_empty() {
                                CheckStatus::Ok
                            } else {
                                CheckStatus::Degraded
                            },
                            latency_ms: None,
                            detail: if problems.is_empty() {
                                if self.cfg.searxng.required_engines.is_empty()
                                    && self.cfg.searxng.disabled_engines.is_empty()
                                {
                                    "no engine requirements configured".into()
                                } else {
                                    "expected engines are enabled and noisy engines are disabled"
                                        .into()
                                }
                            } else {
                                problems.join("; ")
                            },
                        });
                    }
                    Err(error) => {
                        checks.push(DoctorCheck {
                            name: "searxng_engines".into(),
                            status: status_for_error(error.code),
                            latency_ms: None,
                            detail: error.message,
                        });
                    }
                }

                let started = std::time::Instant::now();
                match client.doctor_search().await {
                    Ok(output) => {
                        let count = output.sources.len();
                        checks.push(DoctorCheck {
                            name: "searxng_search".into(),
                            status: if count > 0 {
                                CheckStatus::Ok
                            } else {
                                CheckStatus::ZeroResults
                            },
                            latency_ms: Some(started.elapsed().as_millis() as u64),
                            detail: format!("{count} source(s) for \"searxng doctor\""),
                        });
                    }
                    Err(error) => {
                        checks.push(DoctorCheck {
                            name: "searxng_search".into(),
                            status: status_for_error(error.code),
                            latency_ms: Some(started.elapsed().as_millis() as u64),
                            detail: error.message,
                        });
                    }
                }
            } else {
                checks.push(DoctorCheck {
                    name: "searxng_engines".into(),
                    status: CheckStatus::Skipped,
                    latency_ms: None,
                    detail: "skipped because /healthz failed".into(),
                });
                checks.push(DoctorCheck {
                    name: "searxng_search".into(),
                    status: CheckStatus::Skipped,
                    latency_ms: None,
                    detail: "skipped because /healthz failed".into(),
                });
            }
        } else {
            checks.push(DoctorCheck {
                name: "searxng".into(),
                status: CheckStatus::Skipped,
                latency_ms: None,
                detail: "searxng is disabled in config".into(),
            });
        }

        checks.push(self.probe_tavily(live).await);
        checks.push(self.probe_firecrawl(live).await);
        checks.push(self.probe_fetch(live).await);

        // Snapshot after the probes so cooldowns and breaker transitions they
        // caused are visible in the report.
        let keyring = self.keyring_snapshot();
        let keyring_status = if keyring
            .values()
            .any(|keys| !keys.is_empty() && keys.iter().all(|key| !key.healthy))
        {
            CheckStatus::Degraded
        } else {
            CheckStatus::Ok
        };
        checks.insert(
            1,
            DoctorCheck {
                name: "keyring".into(),
                status: keyring_status,
                latency_ms: None,
                detail: format!(
                    "tavily {} key(s), firecrawl {} key(s)",
                    keyring.get("tavily").map_or(0, Vec::len),
                    keyring.get("firecrawl").map_or(0, Vec::len)
                ),
            },
        );

        let breaker = self.breaker_snapshot();
        let breaker_status = if breaker.values().any(|snapshot| snapshot.state == "open") {
            CheckStatus::Degraded
        } else {
            CheckStatus::Ok
        };
        checks.insert(
            2,
            DoctorCheck {
                name: "breaker".into(),
                status: breaker_status,
                latency_ms: None,
                detail: breaker
                    .iter()
                    .map(|(name, snapshot)| format!("{name}={}", snapshot.state))
                    .collect::<Vec<_>>()
                    .join(", "),
            },
        );

        let status = overall_status(&checks, live);
        DoctorResponse {
            status,
            live_verified: live,
            checks,
            breaker,
            keyring,
        }
    }

    /// A free plain-HTTP probe of the generic fetch path. It never calls
    /// Firecrawl, so live=true does not spend scrape credits here.
    async fn probe_fetch(&self, live: bool) -> DoctorCheck {
        if !live {
            return skipped_probe(
                "fetch",
                "live probe disabled; pass --live to run a free fetch",
            );
        }
        let url = &self.cfg.doctor.fetch_probe_url;
        let started = std::time::Instant::now();
        let response = match self
            .http
            .get(url)
            .timeout(Duration::from_secs(8))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return DoctorCheck {
                    name: "fetch_probe".into(),
                    status: CheckStatus::Unreachable,
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                    detail: format!("GET {url}: {error}"),
                };
            }
        };
        let status = response.status();
        let bytes = response.bytes().await.map(|body| body.len()).unwrap_or(0);
        DoctorCheck {
            name: "fetch_probe".into(),
            status: if status.is_success() && bytes > 0 {
                CheckStatus::Ok
            } else {
                CheckStatus::Degraded
            },
            latency_ms: Some(started.elapsed().as_millis() as u64),
            detail: format!("GET {url} -> HTTP {} ({bytes} bytes)", status.as_u16()),
        }
    }

    async fn probe_tavily(&self, live: bool) -> DoctorCheck {
        if !self.cfg.tavily_usable() {
            return skipped_probe("tavily", "provider is disabled or has no keys");
        }
        if !live {
            return skipped_probe("tavily", "live probe disabled; pass --live to spend one request");
        }
        let Some(client) = &self.tavily else {
            return skipped_probe("tavily", "provider client is not configured");
        };
        let request = ProviderRequest {
            query: "search-gateway doctor".into(),
            max_results: 1,
            recency_days: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            categories: vec!["general".into()],
        };
        let started = std::time::Instant::now();
        match client.search(&request, 1, Duration::from_secs(10)).await {
            Ok(output) => probe_result("tavily", output.sources.len(), started),
            Err(error) => probe_error("tavily", error, started),
        }
    }

    async fn probe_firecrawl(&self, live: bool) -> DoctorCheck {
        if !self.cfg.firecrawl_usable() {
            return skipped_probe("firecrawl", "provider is disabled or has no keys");
        }
        if !live {
            return skipped_probe(
                "firecrawl",
                "live probe disabled; pass --live to spend one request",
            );
        }
        let Some(client) = &self.firecrawl else {
            return skipped_probe("firecrawl", "provider client is not configured");
        };
        let request = ProviderRequest {
            query: "search-gateway doctor".into(),
            max_results: 1,
            recency_days: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            categories: vec!["general".into()],
        };
        let started = std::time::Instant::now();
        match client.search(&request, 1, Duration::from_secs(10)).await {
            Ok(output) => probe_result("firecrawl", output.sources.len(), started),
            Err(error) => probe_error("firecrawl", error, started),
        }
    }

    fn keyring_snapshot(&self) -> BTreeMap<String, Vec<crate::model::KeySnapshot>> {
        let mut snapshots = BTreeMap::new();
        if let Some(client) = &self.tavily {
            snapshots.insert("tavily".into(), client.keyring_snapshot());
        }
        if let Some(client) = &self.firecrawl {
            snapshots.insert("firecrawl".into(), client.keyring_snapshot());
        }
        snapshots
    }

    fn breaker_snapshot(&self) -> BTreeMap<String, crate::model::BreakerSnapshot> {
        let mut snapshots = BTreeMap::new();
        if let Some(client) = &self.tavily {
            snapshots.extend(client.breaker_snapshot());
        }
        if let Some(client) = &self.firecrawl {
            snapshots.extend(client.breaker_snapshot());
        }
        if let Some(client) = &self.searxng {
            snapshots.insert("searxng.search".into(), client.breaker_snapshot());
        }
        snapshots
    }
}

fn skipped_probe(name: &str, detail: &str) -> DoctorCheck {
    DoctorCheck {
        name: format!("{name}_probe"),
        status: CheckStatus::Skipped,
        latency_ms: None,
        detail: detail.into(),
    }
}

fn probe_result(name: &str, count: usize, started: std::time::Instant) -> DoctorCheck {
    DoctorCheck {
        name: format!("{name}_probe"),
        status: if count > 0 {
            CheckStatus::Ok
        } else {
            CheckStatus::ZeroResults
        },
        latency_ms: Some(started.elapsed().as_millis() as u64),
        detail: format!("{count} source(s)"),
    }
}

fn probe_error(name: &str, error: ProviderError, started: std::time::Instant) -> DoctorCheck {
    DoctorCheck {
        name: format!("{name}_probe"),
        status: status_for_error(error.code),
        latency_ms: Some(started.elapsed().as_millis() as u64),
        detail: error.message,
    }
}

fn status_for_error(code: ErrorCode) -> CheckStatus {
    match code {
        ErrorCode::Auth
        | ErrorCode::Misconfigured
        | ErrorCode::Blocked
        | ErrorCode::UnsupportedContentType => CheckStatus::Misconfigured,
        ErrorCode::QuotaExhausted
        | ErrorCode::RateLimited
        | ErrorCode::AllKeysCooling
        | ErrorCode::CircuitOpen
        | ErrorCode::BudgetExhausted => CheckStatus::Degraded,
        ErrorCode::ZeroResults => CheckStatus::ZeroResults,
        ErrorCode::Unreachable | ErrorCode::Timeout | ErrorCode::Http5xx => {
            CheckStatus::Unreachable
        }
        ErrorCode::InvalidResponse | ErrorCode::InvalidRequest | ErrorCode::Http4xx => {
            CheckStatus::Degraded
        }
        ErrorCode::UnknownSearchId | ErrorCode::UnknownFetchId => {
            CheckStatus::Misconfigured
        }
    }
}

fn overall_status(checks: &[DoctorCheck], live: bool) -> DoctorOverallStatus {
    if checks
        .iter()
        .any(|check| check.status == CheckStatus::Misconfigured)
    {
        return DoctorOverallStatus::Misconfigured;
    }
    if checks
        .iter()
        .any(|check| check.status == CheckStatus::Unreachable)
    {
        return DoctorOverallStatus::Unreachable;
    }
    // Without live probes, skipped checks mean the reported health is not
    // verified rather than known-good.
    if !live {
        return DoctorOverallStatus::Unknown;
    }
    if checks.iter().any(|check| {
        matches!(
            check.status,
            CheckStatus::Degraded | CheckStatus::ZeroResults
        )
    }) {
        return DoctorOverallStatus::Degraded;
    }
    DoctorOverallStatus::Ok
}
