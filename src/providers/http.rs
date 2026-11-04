use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::circuit::CircuitBreaker;
use crate::keyring::{KeyLease, Keyring};
use crate::model::{ErrorCode, ProviderError};

/// HTTP executor shared by keyed providers.
///
/// It owns the key ring and the provider circuit breaker. Key-level HTTP
/// failures rotate keys; transport failures retry the same key once.
pub struct KeyedHttp {
    pub http: reqwest::Client,
    /// One key ring per provider, shared by every operation so a key that is
    /// cooling down is skipped by search, fetch and map alike.
    pub keyring: Arc<Mutex<Keyring>>,
    pub breaker: CircuitBreaker,
}

impl KeyedHttp {
    pub fn new(
        http: reqwest::Client,
        keyring: Arc<Mutex<Keyring>>,
        breaker: CircuitBreaker,
    ) -> Self {
        Self {
            http,
            keyring,
            breaker,
        }
    }

    pub async fn send_json<F>(
        &self,
        build: F,
        timeout: Duration,
    ) -> Result<Value, ProviderError>
    where
        F: Fn(&str) -> reqwest::RequestBuilder,
    {
        if !self.breaker.allow() {
            return Err(ProviderError::new(
                ErrorCode::CircuitOpen,
                "provider circuit breaker is open",
                0,
            ));
        }

        let transport_retry = self
            .keyring
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .transport_retry();
        let mut current: Option<KeyLease> = None;
        let mut transport_retried = false;
        let mut attempts = 0u32;
        let mut last_retry_after: Option<u64> = None;

        loop {
            let lease = match current.take() {
                Some(lease) => lease,
                None => {
                    let mut keyring = self
                        .keyring
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if keyring.is_empty() {
                        drop(keyring);
                        self.breaker.release_probe();
                        return Err(ProviderError::new(
                            ErrorCode::Misconfigured,
                            "no API keys configured",
                            attempts,
                        ));
                    }
                    match keyring.next() {
                        Some(lease) => lease,
                        None => {
                            drop(keyring);
                            self.breaker.release_probe();
                            return Err(ProviderError::new(
                                ErrorCode::AllKeysCooling,
                                "all API keys are cooling down",
                                attempts,
                            )
                            .with_retry_after(last_retry_after));
                        }
                    }
                }
            };

            attempts = attempts.saturating_add(1);
            let response = match build(&lease.value).timeout(timeout).send().await {
                Ok(response) => response,
                Err(error) => {
                    let code = if error.is_timeout() {
                        ErrorCode::Timeout
                    } else {
                        ErrorCode::Unreachable
                    };
                    if transport_retry > 0 && !transport_retried {
                        transport_retried = true;
                        current = Some(lease);
                        continue;
                    }
                    self.breaker.record_transient_failure();
                    return Err(ProviderError::new(code, error.to_string(), attempts));
                }
            };

            let status = response.status();
            if status.is_success() {
                let bytes = response.bytes().await.map_err(|error| {
                    self.breaker.record_transient_failure();
                    ProviderError::new(ErrorCode::Unreachable, error.to_string(), attempts)
                })?;
                return serde_json::from_slice(&bytes).map_err(|error| {
                    self.breaker.record_transient_failure();
                    ProviderError::new(
                        ErrorCode::InvalidResponse,
                        format!("provider returned invalid JSON: {error}"),
                        attempts,
                    )
                });
            }

            let retry_after = parse_retry_after(response.headers());
            last_retry_after = retry_after.map(|duration| duration.as_secs());
            let code = classify_status(status.as_u16());
            match code {
                ErrorCode::Auth | ErrorCode::QuotaExhausted | ErrorCode::RateLimited => {
                    self.keyring
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .mark_failure(lease.index, code, retry_after);
                    current = None;
                    continue;
                }
                ErrorCode::Misconfigured => {
                    self.breaker.record_misconfig();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ProviderError::new(
                        code,
                        format!("HTTP {}: {}", status.as_u16(), truncate(&body, 300)),
                        attempts,
                    )
                    .with_retry_after(last_retry_after));
                }
                ErrorCode::InvalidRequest => {
                    // A malformed query or filter is a request-level failure.
                    // It must not open the provider-wide breaker.
                    self.breaker.release_probe();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ProviderError::new(
                        code,
                        format!("HTTP {}: {}", status.as_u16(), truncate(&body, 300)),
                        attempts,
                    )
                    .with_retry_after(last_retry_after));
                }
                ErrorCode::Http5xx => {
                    if transport_retry > 0 && !transport_retried {
                        transport_retried = true;
                        current = Some(lease);
                        continue;
                    }
                    self.breaker.record_transient_failure();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ProviderError::new(
                        code,
                        format!("HTTP {}: {}", status.as_u16(), truncate(&body, 300)),
                        attempts,
                    )
                    .with_retry_after(last_retry_after));
                }
                _ => {
                    self.breaker.record_transient_failure();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ProviderError::new(
                        code,
                        format!("HTTP {}: {}", status.as_u16(), truncate(&body, 300)),
                        attempts,
                    )
                    .with_retry_after(last_retry_after));
                }
            }
        }
    }

    pub fn keyring_snapshot(&self) -> Vec<crate::model::KeySnapshot> {
        self.keyring
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }

    pub fn breaker_snapshot(&self) -> crate::model::BreakerSnapshot {
        self.breaker.snapshot()
    }
}

pub fn classify_status(status: u16) -> ErrorCode {
    match status {
        401 | 403 => ErrorCode::Auth,
        402 | 432 | 433 => ErrorCode::QuotaExhausted,
        429 => ErrorCode::RateLimited,
        400 | 405 | 409 | 413 | 422 => ErrorCode::InvalidRequest,
        500..=599 => ErrorCode::Http5xx,
        _ => ErrorCode::Misconfigured,
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    retry_after_secs(headers).map(Duration::from_secs)
}

/// Parses an integer `Retry-After` header. HTTP-date values are ignored.
pub fn retry_after_secs(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value.trim().parse::<u64>().ok()
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    let mut output: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_errors_are_not_provider_misconfiguration() {
        assert_eq!(classify_status(400), ErrorCode::InvalidRequest);
        assert_eq!(classify_status(422), ErrorCode::InvalidRequest);
        assert_eq!(classify_status(404), ErrorCode::Misconfigured);
        assert_eq!(classify_status(503), ErrorCode::Http5xx);
    }
}
