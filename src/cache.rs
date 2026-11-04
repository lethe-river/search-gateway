use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::CacheConfig;
use crate::model::Source;

#[derive(Debug)]
struct CacheEntry {
    id: String,
    created: u64,
    query: String,
    snippet_max_chars: usize,
    sources: Vec<Source>,
}

#[derive(Debug)]
struct QueryEntry {
    key: String,
    created: u64,
    search_id: String,
    sources: Vec<Source>,
    chain_used: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedCache {
    version: u32,
    entries: Vec<PersistedEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedEntry {
    id: String,
    created: u64,
    #[serde(default)]
    query: String,
    #[serde(default = "default_snippet_cap")]
    snippet_max_chars: usize,
    sources: Vec<Source>,
}

fn default_snippet_cap() -> usize {
    800
}

/// In-process cache for the full normalized source set of a search. Session
/// entries are optionally persisted so `get_sources` survives a restart.
#[derive(Debug)]
pub struct SearchCache {
    entries: Mutex<VecDeque<CacheEntry>>,
    queries: Mutex<VecDeque<QueryEntry>>,
    cfg: CacheConfig,
    persist_path: Option<PathBuf>,
    /// Serializes in-process persistence so concurrent searches cannot
    /// interleave their read-merge-write cycles.
    persist_lock: Arc<Mutex<()>>,
}

#[derive(Debug)]
pub struct CachePage {
    pub total_sources: usize,
    pub query: String,
    pub snippet_max_chars: usize,
    pub sources: Vec<Source>,
    pub next_offset: Option<usize>,
}

#[derive(Debug)]
pub struct CachedQuery {
    pub sources: Vec<Source>,
    pub chain_used: Vec<String>,
    pub search_id: String,
    pub age_secs: u64,
}

impl SearchCache {
    pub fn new(cfg: CacheConfig, persist_path: Option<PathBuf>) -> Self {
        let cache = Self {
            entries: Mutex::new(VecDeque::new()),
            queries: Mutex::new(VecDeque::new()),
            cfg,
            persist_path,
            persist_lock: Arc::new(Mutex::new(())),
        };
        cache.load();
        cache
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<CacheEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn put(&self, sources: Vec<Source>) -> String {
        self.put_with_meta(sources, default_snippet_cap(), String::new())
    }

    pub fn put_with_meta(
        &self,
        mut sources: Vec<Source>,
        snippet_max_chars: usize,
        query: String,
    ) -> String {
        sources.truncate(self.cfg.max_sources_per_session);
        let id = Uuid::new_v4().to_string();
        let now = now_unix();
        {
            let mut entries = self.lock();
            entries.retain(|entry| now.saturating_sub(entry.created) < self.ttl());
            entries.push_front(CacheEntry {
                id: id.clone(),
                created: now,
                query,
                snippet_max_chars,
                sources,
            });
            while entries.len() > self.cfg.max_sessions {
                entries.pop_back();
            }
        }
        self.schedule_persist();
        id
    }

    /// True when a live (TTL-valid) source-set entry exists for `id`.
    pub fn contains(&self, id: &str) -> bool {
        let mut entries = self.lock();
        let now = now_unix();
        entries.retain(|entry| now.saturating_sub(entry.created) < self.ttl());
        entries.iter().any(|entry| entry.id == id)
    }

    /// Updates the description cap stored with a source set. The cached
    /// descriptions themselves keep their original (up to 2000) length.
    pub fn set_snippet_cap(&self, id: &str, snippet_max_chars: usize) {
        let mut entries = self.lock();
        let now = now_unix();
        entries.retain(|entry| now.saturating_sub(entry.created) < self.ttl());
        if let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) {
            entry.snippet_max_chars = snippet_max_chars;
        }
    }

    pub fn get(&self, id: &str, offset: usize, limit: usize) -> Option<CachePage> {
        let mut entries = self.lock();
        let now = now_unix();
        entries.retain(|entry| now.saturating_sub(entry.created) < self.ttl());
        let entry = entries.iter().find(|entry| entry.id == id)?;
        let total = entry.sources.len();
        if offset >= total {
            return Some(CachePage {
                total_sources: total,
                query: entry.query.clone(),
                snippet_max_chars: entry.snippet_max_chars,
                sources: Vec::new(),
                next_offset: None,
            });
        }
        let end = offset.saturating_add(limit).min(total);
        let sources = entry.sources[offset..end].to_vec();
        Some(CachePage {
            total_sources: total,
            query: entry.query.clone(),
            snippet_max_chars: entry.snippet_max_chars,
            sources,
            next_offset: (end < total).then_some(end),
        })
    }

    /// Returns a cached, non-empty source set for an identical search. Access
    /// refreshes LRU order but not the creation-based TTL.
    pub fn get_query(&self, key: &str) -> Option<CachedQuery> {
        let mut queries = self
            .queries
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = now_unix();
        queries.retain(|entry| now.saturating_sub(entry.created) < self.ttl());
        let index = queries.iter().position(|entry| entry.key == key)?;
        let entry = queries.remove(index)?;
        let result = CachedQuery {
            sources: entry.sources.clone(),
            chain_used: entry.chain_used.clone(),
            search_id: entry.search_id.clone(),
            age_secs: now.saturating_sub(entry.created),
        };
        queries.push_front(entry);
        Some(result)
    }

    /// Stores a successful search for identical repeat queries. Empty or
    /// degraded results are never cached.
    pub fn put_query(
        &self,
        key: String,
        sources: Vec<Source>,
        chain_used: Vec<String>,
        search_id: String,
    ) {
        if sources.is_empty() {
            return;
        }
        let mut queries = self
            .queries
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = now_unix();
        queries.retain(|entry| now.saturating_sub(entry.created) < self.ttl());
        queries.retain(|entry| entry.key != key);
        queries.push_front(QueryEntry {
            key,
            created: now,
            search_id,
            sources,
            chain_used,
        });
        while queries.len() > self.cfg.max_sessions {
            queries.pop_back();
        }
    }

    /// Points a cached query at a repaired source-set entry without changing
    /// its age or TTL.
    pub fn replace_query_search_id(&self, key: &str, search_id: String) {
        let mut queries = self
            .queries
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = now_unix();
        queries.retain(|entry| now.saturating_sub(entry.created) < self.ttl());
        if let Some(entry) = queries.iter_mut().find(|entry| entry.key == key) {
            entry.search_id = search_id;
        }
    }

    fn ttl(&self) -> u64 {
        self.cfg.ttl_secs
    }

    fn load(&self) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let Ok(bytes) = fs::read(path) else {
            return;
        };
        let Ok(persisted) = serde_json::from_slice::<PersistedCache>(&bytes) else {
            return;
        };
        if persisted.version != 2 {
            // v1 entries have no query/snippet metadata. They expire within
            // the normal TTL, so dropping them on upgrade is safe.
            return;
        }
        let now = now_unix();
        let ttl = self.ttl();
        let mut entries = self.lock();
        entries.clear();
        for entry in persisted.entries {
            if now.saturating_sub(entry.created) < ttl {
                entries.push_back(CacheEntry {
                    id: entry.id,
                    created: entry.created,
                    query: entry.query,
                    snippet_max_chars: entry.snippet_max_chars,
                    sources: entry.sources,
                });
            }
        }
        while entries.len() > self.cfg.max_sessions {
            entries.pop_back();
        }
    }

    /// Snapshot the live entries and write them off the async worker thread.
    /// Outside a Tokio runtime (unit tests, CLI startup) the write is direct.
    fn schedule_persist(&self) {
        let Some(path) = self.persist_path.clone() else {
            return;
        };
        let snapshot: Vec<PersistedEntry> = {
            let entries = self.lock();
            entries
                .iter()
                .map(|entry| PersistedEntry {
                    id: entry.id.clone(),
                    created: entry.created,
                    query: entry.query.clone(),
                    snippet_max_chars: entry.snippet_max_chars,
                    sources: entry.sources.clone(),
                })
                .collect()
        };
        let max_sessions = self.cfg.max_sessions;
        let ttl = self.ttl();
        let lock = self.persist_lock.clone();
        let write = move || {
            let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
            persist_to_path(&path, snapshot, max_sessions, ttl);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // The handle is intentionally dropped: the cache is
                // best-effort, so a write still in flight at shutdown may be
                // lost rather than blocking the server.
                let _ = handle.spawn_blocking(write);
            }
            Err(_) => write(),
        }
    }
}

fn persist_to_path(
    path: &Path,
    entries: Vec<PersistedEntry>,
    max_sessions: usize,
    ttl: u64,
) {
    let now = now_unix();
    // Several MCP processes may share one config directory. Merge what is
    // already on disk so one process does not silently drop another's
    // search_ids.
    let mut merged: BTreeMap<String, PersistedEntry> = BTreeMap::new();
    if let Ok(bytes) = fs::read(path) {
        if let Ok(persisted) = serde_json::from_slice::<PersistedCache>(&bytes) {
            if persisted.version == 2 {
                for entry in persisted.entries {
                    if now.saturating_sub(entry.created) < ttl {
                        merged.insert(entry.id.clone(), entry);
                    }
                }
            }
        }
    }
    for entry in entries {
        merged.insert(entry.id.clone(), entry);
    }
    let mut entries: Vec<PersistedEntry> = merged.into_values().collect();
    entries.sort_by(|left, right| right.created.cmp(&left.created));
    entries.truncate(max_sessions);

    let Ok(bytes) = serde_json::to_vec(&PersistedCache {
        version: 2,
        entries,
    }) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    if fs::write(&temporary, bytes).is_err() {
        return;
    }
    if fs::rename(&temporary, path).is_err() {
        // Windows cannot rename over an existing file.
        let _ = fs::remove_file(path);
        if fs::rename(&temporary, path).is_err() {
            let _ = fs::remove_file(&temporary);
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: &str) -> Source {
        Source {
            url: url.into(),
            provider: "tavily".into(),
            title: "Title".into(),
            description: Some("description".into()),
            published_date: None,
            content: None,
        }
    }

    #[test]
    fn session_cache_survives_reopen() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("sessions.json");
        let cfg = CacheConfig {
            persist: true,
            ..CacheConfig::default()
        };

        let cache = SearchCache::new(cfg.clone(), Some(path.clone()));
        let id = cache.put(vec![source("https://example.com/a")]);
        drop(cache);

        let reopened = SearchCache::new(cfg, Some(path));
        let page = reopened.get(&id, 0, 10).expect("persisted session");
        assert_eq!(page.total_sources, 1);
        assert_eq!(page.sources[0].url, "https://example.com/a");
    }

    #[test]
    fn concurrent_writers_keep_each_others_sessions() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("sessions.json");
        let cfg = CacheConfig {
            persist: true,
            ..CacheConfig::default()
        };

        let first = SearchCache::new(cfg.clone(), Some(path.clone()));
        let second = SearchCache::new(cfg.clone(), Some(path.clone()));
        let first_id = first.put(vec![source("https://example.com/first")]);
        let second_id = second.put(vec![source("https://example.com/second")]);

        let reopened = SearchCache::new(cfg, Some(path));
        assert!(reopened.get(&first_id, 0, 10).is_some());
        assert!(reopened.get(&second_id, 0, 10).is_some());
    }
}
