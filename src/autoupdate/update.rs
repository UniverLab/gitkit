//! Explicit `gitkit update` — the only path that downloads and replaces the
//! running binary.
//!
//! Port of canopy 3.0.1's `autoupdate` core (`run_update` / `run_update_with`
//! / `UpdateDeps`): the command always asks first — default **NO** — never
//! updates silently, and refuses to touch a cargo-managed install. Every
//! external fact (release list, archive bytes, prompt answer, executable
//! path, target triple) is injected through [`UpdateDeps`], so the unit tests
//! in `tests.rs` run with zero network access.
//!
//! The background check in [`super::check_for_update`] is a separate, silent
//! path: it has its own prompt and its own installer, it never receives the
//! seams defined here, and it swallows every error — only this module's
//! command is allowed to fail loudly on a network problem.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::GITHUB_REPO;

/// Total request timeout for the release-list lookup. An explicit command
/// must fail loudly, but it must never hang either.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(15);

/// Total request timeout for the archive download. A release binary is a few
/// MiB, so a slow link must not be mistaken for a dead one.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// The exact remediation printed when the running binary lives below
/// `~/.cargo/bin`: cargo owns that file, so gitkit refuses to replace it.
pub const CARGO_INSTALL_HINT: &str = "cargo install --force gitkit";

/// The release fields needed to select a stable, published binary.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct GitHubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
}

/// Injectable release-JSON lookup used by the update core.
pub trait ReleaseFetcher {
    fn get(&self, url: &str) -> Result<String>;
}

/// Production release lookup. All network and HTTP-status handling lives
/// behind [`ReleaseFetcher`] so unit tests can use a deterministic fake.
pub struct RealFetcher;

impl ReleaseFetcher for RealFetcher {
    fn get(&self, url: &str) -> Result<String> {
        let response = ureq::get(url)
            .timeout(RELEASE_TIMEOUT)
            .set("User-Agent", "gitkit-update")
            .call()
            .map_err(|error| anyhow!("failed to fetch GitHub releases: {error}"))?;
        if response.status() != 200 {
            bail!("GitHub releases request failed: HTTP {}", response.status());
        }
        response
            .into_string()
            .context("failed to read GitHub releases response")
    }
}

/// Injectable binary downloader. The archive is decoded only after this seam
/// returns, keeping the updater tests entirely offline.
pub trait BinaryDownloader {
    fn download(&self, url: &str) -> Result<Vec<u8>>;
}

/// Production binary downloader.
pub struct RealDownloader;

impl BinaryDownloader for RealDownloader {
    fn download(&self, url: &str) -> Result<Vec<u8>> {
        let response = ureq::get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .set("User-Agent", "gitkit-update")
            .call()
            .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
        if response.status() != 200 {
            bail!("Download failed: HTTP {}", response.status());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .context("failed to read update archive")?;
        Ok(bytes)
    }
}

/// Dependencies for the hermetic update command path. The real command uses
/// the same flow with production I/O; tests provide every fallible external
/// fact here and never contact GitHub.
pub struct UpdateDeps<'a> {
    pub current: &'a str,
    pub releases: std::result::Result<Vec<GitHubRelease>, String>,
    pub exe: &'a Path,
    pub cargo_bin: &'a Path,
    pub target: std::result::Result<&'a str, String>,
    pub downloader: &'a dyn BinaryDownloader,
    pub confirm: &'a dyn Fn() -> bool,
}

// ── Public update entry points ───────────────────────────────────

/// Check for and, after consent, install the latest stable release.
///
/// The returned integer is the process exit code: `0` means no update was
/// installed (already current, a declined prompt, or a cargo-managed
/// install) and `1` means an update was available in `--check` mode or the
/// downloaded archive carried no `gitkit` binary. Network and API errors
/// surface as `Err` — the explicit command fails loudly, unlike the silent
/// background check.
pub fn run_update(check: bool, yes: bool) -> Result<i32> {
    let current = current_version();
    let releases = fetch_releases_with(&RealFetcher)?;

    // The first pass is limited to the network result. The hermetic core
    // below owns all output and consent, so `--check` cannot touch a local
    // path, the target, or the prompt before it returns.
    let latest = select_latest_stable(&releases, current);
    if latest.is_none() || check {
        let deps = UpdateDeps {
            current,
            releases: Ok(releases),
            exe: Path::new("/tmp/gitkit-update-test/gitkit"),
            cargo_bin: Path::new("/tmp/gitkit-update-test/not-cargo"),
            target: Ok("x86_64-unknown-linux-musl"),
            downloader: &RealDownloader,
            confirm: &|| false,
        };
        return run_update_with(check, yes, &deps);
    }

    // An actual install needs the executable and target facts. Resolve them
    // only after the read-only pass established that a newer release exists,
    // so `--check` keeps working on platforms without release assets.
    let latest = latest.expect("newer release was established above");
    let exe = std::env::current_exe().context("failed to locate gitkit executable")?;
    let cargo_bin = cargo_bin_dir();
    // Resolved eagerly but *carried* as a result: the hermetic core decides
    // when it surfaces, so the cargo guard still wins on a cargo-managed
    // install and an unsupported target fails loudly before any download.
    let target = resolve_target().map_err(|error| error.to_string());
    let deps = UpdateDeps {
        current,
        releases: Ok(releases),
        exe: &exe,
        cargo_bin: &cargo_bin,
        target,
        downloader: &RealDownloader,
        confirm: &|| {
            inquire::Confirm::new(&format!("Update to {latest}? [y/N]"))
                .with_default(false)
                .prompt()
                .unwrap_or(false)
        },
    };
    run_update_with(false, yes, &deps)
}

/// Hermetic update flow used by unit tests and embedders. It has no
/// network, process-manager, or filesystem setup step; callers provide
/// those facts through [`UpdateDeps`].
pub fn run_update_with(check: bool, yes: bool, deps: &UpdateDeps<'_>) -> Result<i32> {
    run_update_core(check, yes, deps)
}

fn run_update_core(check: bool, yes: bool, deps: &UpdateDeps<'_>) -> Result<i32> {
    let releases = deps
        .releases
        .as_ref()
        .map_err(|error| anyhow!("release lookup failed: {error}"))?;
    let Some(latest) = select_latest_stable(releases, deps.current) else {
        println!("gitkit {} is up to date", deps.current);
        return Ok(0);
    };
    println!("gitkit {} → {latest}", deps.current);

    // `--check` ends here: exit 1 = update available, 0 = already current.
    // Nothing below this line — cargo guard, target, prompt, download — may
    // run in read-only mode.
    if check {
        return Ok(1);
    }

    if is_cargo_installed(deps.exe, deps.cargo_bin) {
        println!("{CARGO_INSTALL_HINT}");
        return Ok(0);
    }

    let target = deps
        .target
        .as_ref()
        .map_err(|error| anyhow!("target resolution failed: {error}"))?;
    if !yes && !(deps.confirm)() {
        println!("Aborted.");
        return Ok(0);
    }

    let staging = tempfile::tempdir().context("failed to create update staging directory")?;
    let staged = staging.path().join("gitkit-new");
    if !download_and_extract_with(deps.downloader, &latest, target, &staged)? {
        eprintln!("  ✗ Binary not found in archive");
        return Ok(1);
    }

    replace_binary(&staged, deps.exe)?;
    println!("✓ updated to {latest}");
    Ok(0)
}

// ── Version helpers ─────────────────────────────────────────────

pub(super) fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A tag is "stable" when it carries nothing but digits and dots after an
/// optional `v` — anything prerelease-shaped is skipped.
pub(super) fn is_stable_version(tag: &str) -> bool {
    let value = tag.trim_start_matches('v');
    !value.is_empty() && value.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Numeric, `v`-insensitive component-wise comparison; a missing component
/// counts as `0`, so `1.0` equals `1.0.0`.
pub(super) fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u32> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect()
    };
    let (pa, pb) = (parse(a), parse(b));
    let len = pa.len().max(pb.len());
    for index in 0..len {
        let comparison = pa
            .get(index)
            .copied()
            .unwrap_or(0)
            .cmp(&pb.get(index).copied().unwrap_or(0));
        if comparison != std::cmp::Ordering::Equal {
            return comparison;
        }
    }
    std::cmp::Ordering::Equal
}

/// Select the newest published stable release strictly newer than `current`.
/// Drafts, prereleases, and non-semver tags never win.
pub fn select_latest_stable(releases: &[GitHubRelease], current: &str) -> Option<String> {
    releases
        .iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter(|release| is_stable_version(&release.tag_name))
        .filter(|release| compare_versions(&release.tag_name, current).is_gt())
        .max_by(|a, b| compare_versions(&a.tag_name, &b.tag_name))
        .map(|release| release.tag_name.clone())
}

// ── Release lookup ──────────────────────────────────────────────

/// Fetch the full release list (not `/releases/latest`) so draft, prerelease,
/// and non-semver tags can be filtered in code, as the spec requires.
pub(super) fn fetch_releases_with(fetcher: &dyn ReleaseFetcher) -> Result<Vec<GitHubRelease>> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases");
    let body = fetcher.get(&url)?;
    serde_json::from_str(&body).context("failed to parse releases JSON")
}

// ── Target and installation-path helpers ────────────────────────

/// Resolve a target triple from explicit OS/architecture inputs. Windows
/// ships as a `.zip`, which this updater does not handle: refuse loudly
/// before anything is downloaded.
pub fn target_for(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        _ => bail!("unsupported target: {arch}-{os}"),
    }
}

/// Resolve the Rust target triple used by the published release assets.
pub fn resolve_target() -> Result<&'static str> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The exact release asset name, matching `rust-release.yml`'s Package step
/// and `scripts/install.sh`: `gitkit-{tag}-{target}.tar.gz`, tag keeping its
/// leading `v` (e.g. `gitkit-v0.6.0-x86_64-unknown-linux-musl.tar.gz`).
pub fn asset_name(tag: &str, target: &str) -> String {
    format!("gitkit-{tag}-{target}.tar.gz")
}

/// Return the cargo bin directory selected by the environment, falling back
/// to the conventional `$HOME/.cargo/bin` location.
pub fn cargo_bin_dir() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("bin"))
        .unwrap_or_else(|| {
            home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cargo")
                .join("bin")
        })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Test whether an executable lives below the cargo bin directory. Both
/// paths are canonicalized when possible, with the raw paths as a safe
/// fallback for non-existent test paths and macOS temporary symlinks: an
/// uncanonicalizable path is treated as *not* cargo-managed rather than
/// guessing.
pub fn is_cargo_installed(exe: &Path, cargo_bin: &Path) -> bool {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let cargo_bin = cargo_bin
        .canonicalize()
        .unwrap_or_else(|_| cargo_bin.to_path_buf());
    exe.starts_with(cargo_bin)
}

// ── Download, verification, extraction, and atomic replacement ──

/// Download the release asset for `tag`/`target`, verify it when the release
/// ships checksums, and unpack the `gitkit` entry into `output`.
/// Returns `Ok(false)` when the archive holds no such entry.
pub fn download_and_extract_with(
    downloader: &dyn BinaryDownloader,
    tag: &str,
    target: &str,
    output: &Path,
) -> Result<bool> {
    let asset = asset_name(tag, target);
    let url = format!("https://github.com/{GITHUB_REPO}/releases/download/{tag}/{asset}");
    let bytes = downloader.download(&url)?;
    verify_checksum_if_present(downloader, tag, &asset, &bytes)?;

    let decoder = flate2::read::GzDecoder::new(bytes.as_slice());
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .context("corrupt archive: failed to read entries")?
    {
        let mut entry = entry.context("corrupt archive: failed to read entry")?;
        let path = entry
            .path()
            .context("corrupt archive: invalid entry path")?;
        if path.file_name().is_some_and(|name| name == "gitkit") {
            entry
                .unpack(output)
                .context("failed to extract binary from archive")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755))
                    .context("failed to make the new binary executable")?;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// Verify the archive against the release's `SHA256SUMS.txt` when the
/// release ships one. A missing checksum file (older releases) or a missing
/// line for this asset skips verification; a present-but-mismatched digest
/// is fatal — the same rule `scripts/install.sh` applies.
fn verify_checksum_if_present(
    downloader: &dyn BinaryDownloader,
    tag: &str,
    asset: &str,
    bytes: &[u8],
) -> Result<()> {
    let url = format!("https://github.com/{GITHUB_REPO}/releases/download/{tag}/SHA256SUMS.txt");
    let Ok(sums) = downloader.download(&url) else {
        return Ok(());
    };

    let text = String::from_utf8_lossy(&sums);
    let Some(line) = text
        .lines()
        .find(|line| line.split_whitespace().nth(1) == Some(asset))
    else {
        return Ok(());
    };
    let expected = line.split_whitespace().next().unwrap_or("");
    if expected.is_empty() {
        return Ok(());
    }

    use sha2::{Digest, Sha256};
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("SHA256 mismatch for {asset}: expected {expected}, got {actual}");
    }
    Ok(())
}

/// Replace `current_exe` from a staged file using a same-directory temp file
/// plus a rename, so the running binary is swapped atomically and never
/// written in place. Falls back to a copy where a rename over the target is
/// not possible (Windows, exotic mounts).
pub fn replace_binary(staged: &Path, current_exe: &Path) -> Result<()> {
    let parent = current_exe
        .parent()
        .context("cannot determine the gitkit executable directory")?;
    let temporary = tempfile::NamedTempFile::new_in(parent)
        .context("failed to create an adjacent update file")?;
    std::fs::copy(staged, temporary.path()).context("failed to stage gitkit binary")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o755))
            .context("failed to set the executable permission on the staged binary")?;
    }
    temporary
        .as_file()
        .sync_all()
        .context("failed to flush the staged gitkit binary")?;

    if std::fs::rename(temporary.path(), current_exe).is_ok() {
        return Ok(());
    }

    // The NamedTempFile stays alive until this function returns, so the
    // staged bytes are still available after a failed rename.
    std::fs::copy(temporary.path(), current_exe)
        .context("failed to replace binary (copy fallback)")?;
    Ok(())
}
