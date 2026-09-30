use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub(crate) const BASE_URL: &str =
    "https://raw.githubusercontent.com/UniverLab/canopy-registry/main";
pub(crate) const CACHE_NAME: &str = "agent-registry.toml";
pub(crate) const FETCH_BUDGET: Duration = Duration::from_secs(5);
pub(crate) const CACHE_TTL_SECS: i64 = 24 * 3600;

pub(crate) const SNAPSHOT: &str = include_str!("agent_registry_snapshot.toml");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Platform {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) project_paths: Vec<String>,
    #[serde(default)]
    pub(crate) instruction_file: Option<String>,
    #[serde(default)]
    pub(crate) instruction_precedence: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct MergedRegistry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fetched_at: Option<i64>,
    #[serde(default)]
    pub(crate) platforms: Vec<Platform>,
}

#[derive(Deserialize)]
struct Index {
    #[serde(default)]
    platforms: Vec<IndexPlatform>,
}

#[derive(Deserialize)]
struct IndexPlatform {
    name: String,
}

pub(crate) fn parse_merged(text: &str) -> Result<Vec<Platform>> {
    let registry: MergedRegistry =
        toml::from_str(text).context("Failed to parse agent registry")?;
    Ok(registry.platforms)
}

pub(crate) fn snapshot_platforms() -> Vec<Platform> {
    parse_merged(SNAPSHOT).expect("embedded agent registry snapshot is corrupt")
}

pub(crate) trait RegistryFetcher {
    fn get(&self, url: &str, timeout: Duration) -> Result<String>;
}

#[cfg(not(test))]
pub(crate) struct HttpFetcher;

#[cfg(not(test))]
impl RegistryFetcher for HttpFetcher {
    fn get(&self, url: &str, timeout: Duration) -> Result<String> {
        let body = crate::autoupdate::agent(url, timeout)?
            .get(url)
            .set("User-Agent", "gitkit")
            .call()
            .context("Failed to fetch agent registry")?
            .into_string()
            .context("Failed to read agent registry response")?;
        Ok(body)
    }
}

pub(crate) fn remaining_timeout(start: Instant, budget: Duration) -> Option<Duration> {
    budget.checked_sub(start.elapsed())
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn fetch_merged(fetcher: &dyn RegistryFetcher) -> Result<String> {
    let start = Instant::now();
    let first = remaining_timeout(start, FETCH_BUDGET).context("registry fetch budget exceeded")?;
    let index_url = format!("{BASE_URL}/index.toml");
    let index_text = fetcher.get(&index_url, first)?;
    let index: Index = toml::from_str(&index_text).context("Failed to parse registry index")?;
    let mut platforms = Vec::with_capacity(index.platforms.len());
    for entry in &index.platforms {
        let remaining =
            remaining_timeout(start, FETCH_BUDGET).context("registry fetch budget exceeded")?;
        let url = format!("{BASE_URL}/platforms/{}.toml", entry.name);
        let text = fetcher.get(&url, remaining)?;
        let platform = parse_platform(&entry.name, &text)?;
        platforms.push(platform);
    }
    if platforms.is_empty() {
        bail!("registry index declares no platforms");
    }
    let merged = MergedRegistry {
        fetched_at: Some(now_epoch()),
        platforms,
    };
    toml::to_string(&merged).context("Failed to serialize agent registry")
}

fn parse_platform(name: &str, text: &str) -> Result<Platform> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        project_paths: Vec<String>,
        #[serde(default)]
        instruction_file: Option<String>,
        #[serde(default)]
        instruction_precedence: Vec<String>,
    }
    let raw: Raw = toml::from_str(text)
        .with_context(|| format!("Failed to parse registry platform '{name}'"))?;
    Ok(Platform {
        name: name.to_string(),
        project_paths: raw.project_paths,
        instruction_file: raw.instruction_file,
        instruction_precedence: raw.instruction_precedence,
    })
}

#[cfg(not(test))]
pub(crate) fn cache_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("GITKIT_HOME") {
        return Some(PathBuf::from(home).join(CACHE_NAME));
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(home).join(".gitkit").join(CACHE_NAME))
}

pub(crate) fn read_fresh_cache(path: &Path, now: i64) -> Option<Vec<Platform>> {
    let text = std::fs::read_to_string(path).ok()?;
    let registry: MergedRegistry = toml::from_str(&text).ok()?;
    let fetched_at = registry.fetched_at?;
    let age = now.saturating_sub(fetched_at);
    if !(0..CACHE_TTL_SECS).contains(&age) || registry.platforms.is_empty() {
        return None;
    }
    Some(registry.platforms)
}

pub(crate) struct Resolution {
    pub platforms: Vec<Platform>,
    pub notice: Option<String>,
}

fn fallback(reason: &str) -> Resolution {
    Resolution {
        platforms: snapshot_platforms(),
        notice: Some(format!(
            "agentic: canopy registry fetch failed ({reason}); using the embedded registry snapshot"
        )),
    }
}

pub(crate) fn resolve(fetcher: &dyn RegistryFetcher, cache: Option<&Path>) -> Resolution {
    if let Some(path) = cache {
        if let Some(platforms) = read_fresh_cache(path, now_epoch()) {
            return Resolution {
                platforms,
                notice: None,
            };
        }
    }
    let text = match fetch_merged(fetcher) {
        Ok(text) => text,
        Err(error) => return fallback(&short_reason(&error)),
    };
    let platforms = match parse_merged(&text) {
        Ok(platforms) => platforms,
        Err(error) => return fallback(&short_reason(&error)),
    };
    if platforms.is_empty() {
        return fallback("empty registry");
    }
    if let Some(path) = cache {
        write_cache_best_effort(path, &text);
    }
    Resolution {
        platforms,
        notice: None,
    }
}

fn short_reason(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    message
        .lines()
        .next()
        .unwrap_or("unknown error")
        .to_string()
}

fn write_cache_best_effort(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = std::fs::write(path, text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeFetcher {
        texts: HashMap<String, String>,
        error: Option<String>,
    }

    impl FakeFetcher {
        fn ok(texts: HashMap<String, String>) -> Self {
            Self { texts, error: None }
        }

        fn failing(message: &str) -> Self {
            Self {
                texts: HashMap::new(),
                error: Some(message.to_string()),
            }
        }
    }

    impl RegistryFetcher for FakeFetcher {
        fn get(&self, url: &str, _timeout: Duration) -> Result<String> {
            if let Some(message) = &self.error {
                bail!("{message}");
            }
            self.texts
                .get(url)
                .cloned()
                .with_context(|| format!("no fixture for {url}"))
        }
    }

    fn index_text(names: &[&str]) -> String {
        let mut text = String::from("version = 8\n");
        for name in names {
            text.push_str(&format!("\n[[platforms]]\nname = \"{name}\"\n"));
        }
        text
    }

    fn two_platform_fetcher() -> FakeFetcher {
        let mut texts = HashMap::new();
        texts.insert(
            format!("{BASE_URL}/index.toml"),
            index_text(&["foo", "bar"]),
        );
        texts.insert(
            format!("{BASE_URL}/platforms/foo.toml"),
            "project_paths = [\".foo/\"]\ninstruction_file = \"AGENTS.md\"\n".to_string(),
        );
        texts.insert(
            format!("{BASE_URL}/platforms/bar.toml"),
            "project_paths = [\"bar.json\"]\n".to_string(),
        );
        FakeFetcher::ok(texts)
    }

    fn cached_registry(dir: &tempfile::TempDir, age_secs: i64) -> PathBuf {
        let path = dir.path().join(CACHE_NAME);
        let fetched_at = now_epoch() - age_secs;
        let text = format!(
            "fetched_at = {fetched_at}\n\n[[platforms]]\nname = \"cached\"\nproject_paths = [\".cached/\"]\n"
        );
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn parse_full_platform_file_ignores_unknown_fields() {
        let text = "provider = \"Anthropic\"\ntool_name = \"Claude Code\"\nconfig_path = \".claude.json\"\n\
            mcp_servers_key = [\"mcpServers\"]\ninstruction_file = \"AGENTS.md\"\n\
            project_paths = [\".claude/\", \".mcp.json\", \"CLAUDE.local.md\"]\n\
            instruction_precedence = [\"CLAUDE.md\", \".claude/CLAUDE.md\", \"CLAUDE.local.md\"]\n\
            \n[cli]\nbinary = \"claude\"\n\n[required_fields]\nfoo = \"bar\"\n";
        let platform = parse_platform("claude", text).unwrap();
        assert_eq!(platform.name, "claude");
        assert_eq!(
            platform.project_paths,
            vec![".claude/", ".mcp.json", "CLAUDE.local.md"]
        );
        assert_eq!(platform.instruction_file.as_deref(), Some("AGENTS.md"));
        assert_eq!(
            platform.instruction_precedence,
            vec!["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"]
        );
    }

    #[test]
    fn parse_merged_snapshot_has_all_platforms() {
        let platforms = parse_merged(SNAPSHOT).unwrap();
        assert_eq!(platforms.len(), 21);
        assert!(platforms.iter().all(|p| !p.project_paths.is_empty()));
    }

    #[test]
    fn snapshot_platforms_matches_parse() {
        assert_eq!(snapshot_platforms(), parse_merged(SNAPSHOT).unwrap());
    }

    #[test]
    fn fetch_merged_builds_cache_text() {
        let text = fetch_merged(&two_platform_fetcher()).unwrap();
        let platforms = parse_merged(&text).unwrap();
        assert_eq!(platforms.len(), 2);
        assert_eq!(platforms[0].name, "foo");
        assert_eq!(platforms[0].project_paths, vec![".foo/"]);
        assert_eq!(platforms[1].project_paths, vec!["bar.json"]);
    }

    #[test]
    fn remaining_timeout_shrinks_and_expires() {
        let start = Instant::now();
        let remaining = remaining_timeout(start, Duration::from_secs(5)).unwrap();
        assert!(remaining <= Duration::from_secs(5));
        let past = start - Duration::from_secs(6);
        assert!(remaining_timeout(past, Duration::from_secs(5)).is_none());
    }

    #[test]
    fn read_fresh_cache_accepts_fresh_rejects_stale() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = cached_registry(&dir, 3600);
        assert!(read_fresh_cache(&path, now_epoch()).is_some());
        let stale = cached_registry(&dir, 25 * 3600);
        assert!(read_fresh_cache(&stale, now_epoch()).is_none());
        std::fs::write(&path, "not valid toml {{{").unwrap();
        assert!(read_fresh_cache(&path, now_epoch()).is_none());
    }

    /// The TTL is a full day: an entry half a day old is still fresh. This
    /// pins the window itself (24 * 3600), not just its two ends — the fresh/
    /// stale pair above would accept a much shorter TTL too.
    #[test]
    fn read_fresh_cache_accepts_half_day_old_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = cached_registry(&dir, 12 * 3600);
        assert!(
            read_fresh_cache(&path, now_epoch()).is_some(),
            "12h is inside the {CACHE_TTL_SECS}s TTL"
        );
    }

    #[test]
    fn read_fresh_cache_rejects_future_timestamp() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(CACHE_NAME);
        let fetched_at = now_epoch() + 3600;
        std::fs::write(
            &path,
            format!(
                "fetched_at = {fetched_at}\n\n[[platforms]]\nname = \"x\"\nproject_paths = [\".x/\"]\n"
            ),
        )
        .unwrap();
        assert!(read_fresh_cache(&path, now_epoch()).is_none());
    }

    #[test]
    fn resolve_prefers_fresh_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = cached_registry(&dir, 60);
        let resolution = resolve(&FakeFetcher::failing("must not be called"), Some(&path));
        assert!(resolution.notice.is_none());
        assert_eq!(resolution.platforms.len(), 1);
        assert_eq!(resolution.platforms[0].name, "cached");
    }

    #[test]
    fn resolve_recovers_from_fetch_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(CACHE_NAME);
        let resolution = resolve(&FakeFetcher::failing("offline"), Some(&path));
        assert_eq!(resolution.platforms.len(), 21);
        assert!(resolution.notice.as_deref().unwrap().contains("embedded"));
    }

    #[test]
    fn resolve_fetch_success_writes_cache() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(CACHE_NAME);
        let resolution = resolve(&two_platform_fetcher(), Some(&path));
        assert!(resolution.notice.is_none());
        assert!(path.exists());
        let cached = read_fresh_cache(&path, now_epoch()).unwrap();
        assert_eq!(cached.len(), 2);
    }

    #[test]
    fn resolve_empty_registry_falls_back() {
        let mut texts = HashMap::new();
        texts.insert(
            format!("{BASE_URL}/index.toml"),
            "version = 8\n".to_string(),
        );
        let resolution = resolve(&FakeFetcher::ok(texts), None);
        assert_eq!(resolution.platforms.len(), 21);
        assert!(resolution.notice.is_some());
    }
}
