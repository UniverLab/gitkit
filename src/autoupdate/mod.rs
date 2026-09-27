//! Update check: looks for a newer GitHub release and prints a one-line
//! notice pointing at `gitkit update`. It never prompts and never installs —
//! the explicit command in [`update`] is the only path that replaces the
//! binary, and it always asks first.
//!
//! Called once from `main`, before any subcommand runs — gitkit's own
//! binary is never invoked from inside a git hook (the hooks it installs
//! are plain POSIX `sh` scripts), so this is never on a hook path.
//! Every failure here returns silently: a version check must never
//! interrupt the user's actual work.

use std::io::IsTerminal;
use std::time::Duration;

use serde::Deserialize;

#[cfg(test)]
mod tests;
pub mod update;

pub use update::run_update;

const GITHUB_REPO: &str = "UniverLab/gitkit";
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
}

/// Entry point. Opt out with `GITKIT_NO_UPDATE_CHECK` (any value).
pub fn check_for_update() {
    if update_check_disabled(std::env::var("GITKIT_NO_UPDATE_CHECK").ok()) {
        return;
    }

    let Some(latest) = fetch_latest_tag() else {
        return;
    };

    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    if !is_newer(&current, &latest) {
        return;
    }

    // A confirm prompt in a script or CI would hang it — skip straight past.
    if !std::io::stdin().is_terminal() {
        return;
    }

    println!("  \x1b[33m⬆  Update available:\x1b[0m {current} → {latest}");
    println!("     Run \x1b[36mgitkit update\x1b[0m when you want it — gitkit never replaces itself on your behalf.");
}

fn fetch_latest_tag() -> Option<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let resp = ureq::get(&url)
        .timeout(HTTP_TIMEOUT)
        .set("User-Agent", "gitkit-autoupdate")
        .call()
        .ok()?;
    let body = resp.into_string().ok()?;
    let release: GithubRelease = serde_json::from_str(&body).ok()?;
    if release.tag_name.is_empty() {
        return None;
    }
    Some(release.tag_name)
}

fn update_check_disabled(opt_out: Option<String>) -> bool {
    opt_out.is_some()
}

fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |v: &str| -> (u64, u64, u64) {
        let v = v.trim_start_matches('v');
        let p: Vec<u64> = v.split('.').filter_map(|s| s.parse().ok()).collect();
        (
            *p.first().unwrap_or(&0),
            *p.get(1).unwrap_or(&0),
            *p.get(2).unwrap_or(&0),
        )
    };
    parse(latest) > parse(current)
}
