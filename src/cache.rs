use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::model::{Agent, Session};

// Since v0.12.0 the cache also stores `agf_version` (the writing binary's
// version); any mismatch on read forces a full rescan (issue #37). That makes
// per-release CACHE_VERSION bumps unnecessary for scanner-behavior changes —
// CACHE_VERSION only needs bumping for schema/interpretation changes that can
// ship *within* one package version (e.g. successive dev builds).
//
// Bumped to 6 in v0.11.3:
// - Cursor scanner now (a) walks both depth-3 .txt and depth-4 .jsonl layouts,
//   (b) reads chat metadata from the `meta` table of store.db, and (c) drops
//   .jsonl transcripts that have no matching store.db (orphans `cursor-agent`
//   itself refuses to resume). Cache entries written by 0.11.x would surface
//   the old orphan-laden list until each transcript's mtime happened to
//   change. Bumping the version forces a one-time rescan on upgrade so the
//   PR description's "35 orphans -> 0" claim actually holds for upgraders.
//
// Bumped to 5 after v0.11.1:
// - Pi scanner now keeps all user-message summaries instead of only the first
//   one, matching the History preview behavior of other agents.
//
// Bumped to 4 after v0.11.1:
// - Pi scanner now extracts first-user-message summaries from JSONL session
//   logs. Older cache entries stored empty Pi summaries with fresh mtimes, so
//   they would otherwise keep rendering only the project name until a source
//   file changed.
//
// Bumped to 3 in v0.11.0:
// - Hermes Agent registered (#34) so the per-agent cache map gains a new key.
// - Hermes session payloads now carry first-user-message previews (only on
//   CLI/TUI session ids) instead of bare source/model fallbacks; older cache
//   entries written by 0.10.x would surface as stale "cli session (...)"
//   summaries until the source DB mtime happens to change.
//   Bumping the version forces a one-time rescan on first 0.11.0 launch.
// Bumped to 7 in v0.13.0:
// - Oh My Pi (#56) and Yolop (#55) each add a new per-agent cache key.
// - Yolop worktree sessions use repo_root for the project name instead of the
//   generated session directory, and recover the original repository name via
//   `.git` indirection / parent-session metadata; its persisted titles,
//   canonical roots, timestamps, and short worktree slugs replace inferred
//   summaries, generated paths, and log-only times. Local dev builds can share a
//   package version, so agf_version alone cannot invalidate their old entries.
//
// Bumped to 8 in v0.13.1:
// - Claude's `data_sources` now includes `~/.claude/projects`, so entries
//   written by 0.13.0 carry an mtime taken from history.jsonl alone. That
//   mtime is older than the tree's, which would make every 0.13.0 entry look
//   stale-but-present in a confusing half state; a clean rescan is simpler.
// - Yolop timestamps are now clamped to a plausible window, so persisted
//   far-future values must not survive the upgrade and keep pinning sessions
//   to the top of the time sort.
const CACHE_VERSION: u32 = 9;

/// The binary version stamped into every cache write; any mismatch on read
/// invalidates the whole cache (see `parse_cache`).
const AGF_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    /// `CARGO_PKG_VERSION` of the binary that wrote the file. Defaults to ""
    /// for caches written before this field existed, which fails the version
    /// match in `parse_cache` and forces the rescan we want.
    #[serde(default)]
    agf_version: String,
    agents: HashMap<Agent, AgentCache>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentCache {
    fingerprint: SourceFingerprint,
    sessions: Vec<CachedSession>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceFingerprint {
    /// Lexical source identity catches configurable-root changes even when two
    /// directories happen to have identical metadata.
    sources: Vec<String>,
    newest_mtime_ns: u64,
    total_size: u64,
    entries: u64,
    /// Stable digest of every entry's path, type, mtime, and size. Aggregate
    /// maxima/totals alone miss a same-size edit to an older file whenever a
    /// different file still owns the newest mtime.
    digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedSession {
    agent: Agent,
    session_id: String,
    project_name: String,
    project_path: String,
    summaries: Vec<String>,
    timestamp: i64,
    git_branch: Option<String>,
    worktree: Option<String>,
    recap: Option<String>,
    #[serde(default = "default_true")]
    interactive: bool,
}

const fn default_true() -> bool {
    true
}

fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
        .join("agf")
        .join("sessions.json")
}

struct CacheWriteLock(PathBuf);

impl CacheWriteLock {
    fn acquire(path: PathBuf) -> std::io::Result<Self> {
        for _ in 0..200 {
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
                        .is_ok_and(|age| age > std::time::Duration::from_secs(300));
                    if stale {
                        let _ = fs::remove_dir(&path);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "timed out waiting for cache write lock",
        ))
    }
}

impl Drop for CacheWriteLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

/// True when the `AGF_DEBUG` env var is set (any value), probed once per process.
fn debug_enabled() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| std::env::var("AGF_DEBUG").is_ok())
}

fn to_cached(s: &Session) -> CachedSession {
    CachedSession {
        agent: s.agent,
        session_id: s.session_id.clone(),
        project_name: s.project_name.clone(),
        project_path: s.project_path.clone(),
        summaries: s.summaries.iter().take(10).cloned().collect(),
        timestamp: s.timestamp,
        git_branch: s.git_branch.clone(),
        worktree: s.worktree.clone(),
        recap: s.recap.clone(),
        interactive: s.interactive,
    }
}

fn from_cached(c: &CachedSession) -> Session {
    Session {
        agent: c.agent,
        session_id: c.session_id.clone(),
        project_name: c.project_name.clone(),
        project_path: c.project_path.clone(),
        summaries: c.summaries.clone(),
        timestamp: crate::model::normalize_timestamp(c.timestamp, 0),
        git_branch: c.git_branch.clone(),
        worktree: c.worktree.clone(),
        recap: c.recap.clone(),
        interactive: c.interactive,
    }
}

fn source_fingerprint(paths: &[PathBuf]) -> SourceFingerprint {
    use sha2::{Digest, Sha256};
    use walkdir::WalkDir;
    let mut fingerprint = SourceFingerprint {
        sources: paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        ..SourceFingerprint::default()
    };
    fingerprint.sources.sort();
    let mut entry_fingerprints = Vec::new();
    for p in paths {
        if !p.exists() {
            continue;
        }
        for entry in WalkDir::new(p)
            .max_depth(8)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if let Ok(m) = entry.metadata() {
                let mtime_ns = m
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
                    .unwrap_or(0);
                fingerprint.newest_mtime_ns = fingerprint.newest_mtime_ns.max(mtime_ns);
                fingerprint.total_size = fingerprint.total_size.saturating_add(m.len());
                fingerprint.entries = fingerprint.entries.saturating_add(1);
                let file_type = if m.is_dir() {
                    1
                } else if m.is_file() {
                    2
                } else if m.file_type().is_symlink() {
                    3
                } else {
                    4
                };
                entry_fingerprints.push((
                    entry.path().to_string_lossy().into_owned(),
                    file_type,
                    mtime_ns,
                    m.len(),
                ));
            }
        }
    }
    entry_fingerprints.sort_unstable();
    let mut hasher = Sha256::new();
    for (path, file_type, mtime_ns, size) in entry_fingerprints {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update([file_type]);
        hasher.update(mtime_ns.to_le_bytes());
        hasher.update(size.to_le_bytes());
    }
    fingerprint.digest = format!("{:x}", hasher.finalize());
    fingerprint
}

pub(crate) fn agent_fingerprint(agent: Agent) -> SourceFingerprint {
    source_fingerprint(&config::data_sources(agent))
}

/// Validate raw cache-file content. The cache is usable only when (a) it
/// parses, (b) the schema `version` matches, and (c) it was written by this
/// exact binary version. (c) exists because scanner behavior can change
/// between releases without a schema change — issue #37: mtime checks can't
/// distinguish "the data didn't change" from "the data didn't change but its
/// interpretation just did", so every upgrade pays one full rescan instead of
/// silently serving per-session payloads shaped by an older binary.
fn parse_cache(content: &str, binary_version: &str) -> Result<CacheFile, String> {
    let cache: CacheFile =
        serde_json::from_str(content).map_err(|e| format!("cache parse failed: {e}"))?;
    if cache.version != CACHE_VERSION {
        return Err(format!(
            "cache schema version {} != {}",
            cache.version, CACHE_VERSION
        ));
    }
    if cache.agf_version != binary_version {
        let by = if cache.agf_version.is_empty() {
            "an older agf (pre-agf_version)"
        } else {
            cache.agf_version.as_str()
        };
        return Err(format!("cache built by {by}, current {binary_version}"));
    }
    Ok(cache)
}

/// Load cached sessions. Returns (sessions, stale_agents).
/// stale_agents are agents whose data sources have changed since cache was written.
pub fn load_cache() -> (Vec<Session>, Vec<Agent>) {
    let path = cache_path();
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), config::installed_agents()),
    };

    let cache = match parse_cache(&content, AGF_VERSION) {
        Ok(c) => c,
        Err(why) => {
            if debug_enabled() {
                eprintln!("[agf] {why} → rescanning");
            }
            return (Vec::new(), config::installed_agents());
        }
    };

    let mut sessions = Vec::new();
    let mut stale = Vec::new();

    for agent in config::installed_agents() {
        let current_fingerprint = source_fingerprint(&config::data_sources(agent));

        match cache.agents.get(&agent) {
            Some(ac) => {
                // Stale-while-revalidate: cached payload remains visible until
                // a successful worker result replaces it.
                sessions.extend(ac.sessions.iter().map(from_cached));
                if ac.fingerprint != current_fingerprint {
                    stale.push(agent);
                }
            }
            None => stale.push(agent),
        }
    }

    sessions.sort_by(|a, b| crate::model::compare_sessions(a, b, crate::model::SortMode::Time));
    (sessions, stale)
}

/// Write all sessions to cache, grouped by agent.
///
/// `skip_agents` lists agents whose background scan did not complete this
/// run (e.g. the user exited the TUI before the worker finished); for those
/// we preserve the prior cache entry verbatim so we don't accidentally
/// persist an empty session list with a fresh `mtime`, which would mark the
/// agent "fresh" on the next launch and hide its sessions.
pub fn write_cache(
    sessions: &[Session],
    skip_agents: &std::collections::HashSet<Agent>,
    invalidate_agents: &std::collections::HashSet<Agent>,
    observed_fingerprints: &HashMap<Agent, SourceFingerprint>,
) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let lock_path = path.with_file_name(".sessions-write.lock");
    let Ok(_write_lock) = CacheWriteLock::acquire(lock_path) else {
        return;
    };

    let mut agents: HashMap<Agent, AgentCache> = HashMap::new();

    // Start from prior entries. Agents not scanned by this process must remain
    // byte-for-byte associated with the fingerprint they were loaded under;
    // recomputing a newer fingerprint beside old payload would make stale data
    // appear fresh after an append-vs-exit race.
    // gates this on schema AND binary version: entries written by another
    // binary are exactly the stale data issue #37 is about, so on upgrade we
    // drop them and let the next launch rescan those agents instead.
    if let Ok(content) = fs::read_to_string(&path)
        && let Ok(mut prior) = parse_cache(&content, AGF_VERSION)
    {
        agents.extend(prior.agents.drain());
    }
    for agent in invalidate_agents {
        agents.remove(agent);
    }

    for agent in config::installed_agents() {
        if skip_agents.contains(&agent) || invalidate_agents.contains(&agent) {
            continue;
        }
        let Some(observed) = observed_fingerprints.get(&agent) else {
            continue;
        };
        // A source changed after the worker's final fingerprint and before
        // cache persistence. Preserve the prior stale entry; the next launch
        // will rescan instead of blessing pre-change payload as current.
        if &agent_fingerprint(agent) != observed {
            continue;
        }
        let agent_sessions: Vec<CachedSession> = sessions
            .iter()
            .filter(|s| s.agent == agent)
            .map(to_cached)
            .collect();
        agents.insert(
            agent,
            AgentCache {
                fingerprint: observed.clone(),
                sessions: agent_sessions,
            },
        );
    }

    let cache = CacheFile {
        version: CACHE_VERSION,
        agf_version: AGF_VERSION.to_string(),
        agents,
    };

    if let Ok(json) = serde_json::to_string(&cache) {
        // Best-effort: a cache we failed to persist just costs the next launch
        // a rescan, so there is nothing actionable to report here.
        if crate::fsx::write_atomic(&path, json.as_bytes()).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
            }
        }
    }
}

/// One agent's scan result, streamed back from a worker thread.
pub struct ScanResult {
    pub agent: Agent,
    pub sessions: Result<crate::scanner::CompletedScan, String>,
}

/// Spawn one worker thread per stale agent. Each worker sends its result on
/// `tx` as soon as it finishes — the TUI can ingest results progressively
/// without blocking on the slowest scanner.
///
/// The returned `JoinHandle` resolves once every worker has finished; the
/// caller can use it to know when the channel will close.
pub fn start_stale_scan(stale: &[Agent]) -> std::sync::mpsc::Receiver<ScanResult> {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    let debug = debug_enabled();
    let installed: std::collections::HashSet<Agent> =
        crate::config::installed_agents().into_iter().collect();
    let stale: Vec<Agent> = stale
        .iter()
        .copied()
        .filter(|a| installed.contains(a))
        .collect();

    let (tx, rx) = mpsc::channel();
    for agent in stale {
        let tx = tx.clone();
        thread::spawn(move || {
            let start = Instant::now();
            let sessions =
                std::panic::catch_unwind(|| crate::scanner::scan_agent_consistent(agent))
                    .map_err(|_| "scanner panicked".to_string())
                    .and_then(|result| result);
            if debug {
                match &sessions {
                    Ok(scan) => eprintln!(
                        "[agf] {:?} scan: {} sessions in {:?}",
                        agent,
                        scan.sessions.len(),
                        start.elapsed()
                    ),
                    Err(error) => eprintln!(
                        "[agf] {:?} scan failed in {:?}: {error}",
                        agent,
                        start.elapsed()
                    ),
                }
            }
            // Receiver dropped (TUI exited): silently ignore.
            let _ = tx.send(ScanResult { agent, sessions });
        });
    }
    // Drop the original sender so the receiver closes once all workers finish.
    drop(tx);
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn temp_source(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("agf-cache-{label}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn cache_json(version: u32, agf_version: Option<&str>) -> String {
        let mut v = serde_json::json!({
            "version": version,
            "agents": {}
        });
        if let Some(av) = agf_version {
            v["agf_version"] = serde_json::Value::String(av.to_string());
        }
        v.to_string()
    }

    #[test]
    fn parse_cache_accepts_matching_schema_and_binary_version() {
        let json = cache_json(CACHE_VERSION, Some(AGF_VERSION));
        assert!(parse_cache(&json, AGF_VERSION).is_ok());
    }

    #[test]
    fn parse_cache_rejects_schema_version_mismatch() {
        let json = cache_json(CACHE_VERSION + 1, Some(AGF_VERSION));
        let err = parse_cache(&json, AGF_VERSION).unwrap_err();
        assert!(err.contains("schema version"), "unexpected: {err}");
    }

    #[test]
    fn parse_cache_rejects_binary_version_mismatch() {
        let json = cache_json(CACHE_VERSION, Some("0.0.1"));
        let err = parse_cache(&json, AGF_VERSION).unwrap_err();
        assert!(err.contains("cache built by 0.0.1"), "unexpected: {err}");
    }

    #[test]
    fn parse_cache_rejects_pre_agf_version_caches() {
        // Caches written before the field existed carry no `agf_version` key;
        // the serde default "" must fail the match and force a rescan.
        let json = cache_json(CACHE_VERSION, None);
        let err = parse_cache(&json, AGF_VERSION).unwrap_err();
        assert!(err.contains("pre-agf_version"), "unexpected: {err}");
    }

    #[test]
    fn parse_cache_rejects_garbage() {
        let err = parse_cache("not json", AGF_VERSION).unwrap_err();
        assert!(err.contains("cache parse failed"), "unexpected: {err}");
    }

    #[test]
    fn cache_file_round_trips_and_agent_serializes_as_variant_name() {
        // Protects the on-disk format: `Agent` unit variants must serialize
        // as their exact variant-name strings — the same strings the old
        // hand-rolled agent_to_str/agent_from_str mapping produced.
        assert_eq!(
            serde_json::to_string(&Agent::ClaudeCode).unwrap(),
            "\"ClaudeCode\""
        );

        let mut agents = HashMap::new();
        agents.insert(
            Agent::ClaudeCode,
            AgentCache {
                fingerprint: SourceFingerprint {
                    sources: vec!["/source".to_string()],
                    newest_mtime_ns: 42,
                    total_size: 7,
                    entries: 1,
                    digest: "fixture".to_string(),
                },
                sessions: vec![CachedSession {
                    agent: Agent::ClaudeCode,
                    session_id: "sid".to_string(),
                    project_name: "proj".to_string(),
                    project_path: "/p".to_string(),
                    summaries: vec!["hello".to_string()],
                    timestamp: 7,
                    git_branch: Some("main".to_string()),
                    worktree: None,
                    recap: None,
                    interactive: true,
                }],
            },
        );
        let cache = CacheFile {
            version: CACHE_VERSION,
            agf_version: AGF_VERSION.to_string(),
            agents,
        };

        let json = serde_json::to_string(&cache).unwrap();
        assert!(json.contains("\"ClaudeCode\""), "unexpected json: {json}");

        let parsed = parse_cache(&json, AGF_VERSION).expect("round-trip parse");
        let entry = &parsed.agents[&Agent::ClaudeCode];
        assert_eq!(entry.fingerprint.newest_mtime_ns, 42);
        assert_eq!(entry.fingerprint.digest, "fixture");
        assert_eq!(entry.sessions.len(), 1);
        assert_eq!(entry.sessions[0].agent, Agent::ClaudeCode);
        assert_eq!(entry.sessions[0].session_id, "sid");
        assert_eq!(entry.sessions[0].git_branch.as_deref(), Some("main"));
    }

    #[test]
    fn fingerprint_distinguishes_changes_within_one_second() {
        let path = temp_source("nanoseconds");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"a").unwrap();
        file.set_modified(
            SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_700_000_000)
                + Duration::from_millis(100),
        )
        .unwrap();
        let first = source_fingerprint(std::slice::from_ref(&path));
        file.set_modified(
            SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_700_000_000)
                + Duration::from_millis(200),
        )
        .unwrap();
        let second = source_fingerprint(std::slice::from_ref(&path));
        assert_ne!(first, second);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn fingerprint_includes_source_identity_and_wal_size() {
        let db = temp_source("db");
        let wal = temp_source("db-wal");
        std::fs::write(&db, b"db").unwrap();
        let before = source_fingerprint(&[db.clone(), wal.clone()]);
        std::fs::write(&wal, b"new wal contents").unwrap();
        let after = source_fingerprint(&[db.clone(), wal.clone()]);
        assert_ne!(before, after);
        let other = source_fingerprint(std::slice::from_ref(&db));
        assert_ne!(after.sources, other.sources);
        let _ = std::fs::remove_file(db);
        let _ = std::fs::remove_file(wal);
    }

    #[test]
    fn fingerprint_detects_same_size_edit_to_non_newest_entry() {
        let dir = temp_source("non-newest");
        std::fs::create_dir_all(&dir).unwrap();
        let older = dir.join("older");
        let newest = dir.join("newest");
        std::fs::write(&older, b"old").unwrap();
        std::fs::write(&newest, b"top").unwrap();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(base + Duration::from_secs(1))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&newest)
            .unwrap()
            .set_modified(base + Duration::from_secs(3))
            .unwrap();
        let first = source_fingerprint(std::slice::from_ref(&dir));

        // Same total size and still older than `newest`: aggregate
        // newest-mtime/size/count fingerprints cannot see this change.
        std::fs::write(&older, b"new").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(base + Duration::from_secs(2))
            .unwrap();
        let second = source_fingerprint(std::slice::from_ref(&dir));

        assert_eq!(first.newest_mtime_ns, second.newest_mtime_ns);
        assert_eq!(first.total_size, second.total_size);
        assert_eq!(first.entries, second.entries);
        assert_ne!(first.digest, second.digest);
        let _ = std::fs::remove_dir_all(dir);
    }
}
