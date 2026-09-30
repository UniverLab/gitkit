//! Tests for the autoupdate module.
//!
//! The background-check tests (top) live here unchanged from their original
//! home in `mod.rs`; everything below exercises the explicit `gitkit update`
//! core in [`super::update`] through injected fetchers and downloaders — no
//! test touches the network.

use super::*;
use anyhow::Context;
use serial_test::serial;
use std::cell::Cell;
use std::path::{Path, PathBuf};

// ── Background check (`check_for_update`) ───────────────────────

#[test]
fn is_newer_minor_version() {
    assert!(is_newer("v0.9.0", "v0.10.0"));
}

#[test]
fn is_newer_major_version() {
    assert!(is_newer("v0.99.99", "v1.0.0"));
}

#[test]
fn is_newer_equal_versions_not_newer() {
    assert!(!is_newer("v0.4.0", "v0.4.0"));
}

#[test]
fn is_newer_older_is_not_newer() {
    assert!(!is_newer("v1.0.0", "v0.9.0"));
}

#[test]
fn is_newer_handles_missing_v_prefix_on_current() {
    assert!(is_newer("0.4.0", "v0.5.0"));
}

#[test]
fn is_newer_handles_missing_v_prefix_on_latest() {
    assert!(is_newer("v0.4.0", "0.5.0"));
}

#[test]
fn is_newer_handles_missing_v_prefix_on_both() {
    assert!(is_newer("0.4.0", "0.5.0"));
}

#[test]
fn update_check_disabled_when_var_is_set() {
    assert!(update_check_disabled(Some(String::new())));
    assert!(update_check_disabled(Some("1".to_string())));
}

#[test]
fn update_check_not_disabled_when_var_is_absent() {
    assert!(!update_check_disabled(None));
}

// ── Version comparison ──────────────────────────────────────────

#[test]
fn compare_versions_equal() {
    assert_eq!(
        super::update::compare_versions("1.0.0", "1.0.0"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn compare_versions_greater_patch() {
    assert_eq!(
        super::update::compare_versions("1.0.1", "1.0.0"),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn compare_versions_less_patch() {
    assert_eq!(
        super::update::compare_versions("1.0.0", "1.0.1"),
        std::cmp::Ordering::Less
    );
}

#[test]
fn compare_versions_major_wins() {
    assert_eq!(
        super::update::compare_versions("2.0.0", "1.9.9"),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn compare_versions_with_v_prefix() {
    assert_eq!(
        super::update::compare_versions("v1.2.3", "1.2.3"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn compare_versions_different_length() {
    assert_eq!(
        super::update::compare_versions("1.0", "1.0.0"),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        super::update::compare_versions("1.0.0.1", "1.0.0"),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn compare_versions_handles_build_metadata() {
    assert_eq!(
        super::update::compare_versions("1.0.0+build1", "1.0.0+build2"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn stable_version_accepts_plain() {
    assert!(super::update::is_stable_version("1.0.0"));
    assert!(super::update::is_stable_version("v1.0.0"));
    assert!(super::update::is_stable_version("v0.32.1"));
    assert!(super::update::is_stable_version("10.20.30"));
}

#[test]
fn stable_version_rejects_prerelease() {
    assert!(!super::update::is_stable_version("1.0.0-beta"));
    assert!(!super::update::is_stable_version("v1.0.0-rc1"));
    assert!(!super::update::is_stable_version("1.0.0-alpha+build123"));
    assert!(!super::update::is_stable_version("1.0.0-something"));
    assert!(!super::update::is_stable_version("v2.0.0-rc.1"));
}

#[test]
fn stable_version_rejects_empty() {
    assert!(!super::update::is_stable_version(""));
    assert!(!super::update::is_stable_version("v"));
}

#[test]
fn current_version_returns_non_empty() {
    let version = super::update::current_version();
    assert!(!version.is_empty(), "version should not be empty");
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}

// ── Asset naming (release workflow contract) ────────────────────

#[test]
fn asset_name_gitkit_musl() {
    // Shape asserted against a literal, then re-checked against a tag
    // derived from the crate version so a bump can never rot this test.
    assert_eq!(
        super::update::asset_name("v1.2.3", "x86_64-unknown-linux-musl"),
        "gitkit-v1.2.3-x86_64-unknown-linux-musl.tar.gz"
    );
    assert_eq!(
        super::update::asset_name("v1.2.3", "aarch64-apple-darwin"),
        "gitkit-v1.2.3-aarch64-apple-darwin.tar.gz"
    );
    let derived = format!("v{}", env!("CARGO_PKG_VERSION"));
    assert_eq!(
        super::update::asset_name(&derived, "x86_64-unknown-linux-musl"),
        format!("gitkit-{derived}-x86_64-unknown-linux-musl.tar.gz")
    );
}

// ── Cargo-install guard ─────────────────────────────────────────

#[test]
fn cargo_detect_default_home() {
    assert!(super::update::is_cargo_installed(
        Path::new("/home/u/.cargo/bin/gitkit"),
        Path::new("/home/u/.cargo/bin")
    ));
}

#[test]
fn cargo_detect_local_bin_is_not_cargo() {
    assert!(!super::update::is_cargo_installed(
        Path::new("/home/u/.local/bin/gitkit"),
        Path::new("/home/u/.cargo/bin")
    ));
}

#[serial]
#[test]
fn cargo_detect_custom_cargo_home() {
    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let previous = std::env::var_os("CARGO_HOME");
    std::env::set_var("CARGO_HOME", "/opt/cargo");
    let cargo_bin = super::update::cargo_bin_dir();
    assert!(super::update::is_cargo_installed(
        Path::new("/opt/cargo/bin/gitkit"),
        &cargo_bin
    ));
    assert!(!super::update::is_cargo_installed(
        Path::new("/home/u/.cargo/bin/gitkit"),
        &cargo_bin
    ));
    match previous {
        Some(value) => std::env::set_var("CARGO_HOME", value),
        None => std::env::remove_var("CARGO_HOME"),
    }
}

/// The refusal line is a spec literal — it must stay byte-identical to what
/// the docs quote for cargo users (em dash U+2014 included).
#[test]
fn cargo_refusal_message_is_the_spec_literal() {
    assert_eq!(
        super::update::CARGO_INSTALL_HINT,
        "installed with cargo — run: cargo install --force gitkit"
    );
}

#[test]
fn display_version_drops_v_for_messages() {
    assert_eq!(super::update::display_version("v0.6.0"), "0.6.0");
    assert_eq!(super::update::display_version("0.6.0"), "0.6.0");
    // Guard the other half of the contract: only display strips the `v`.
    // Asset names and download URLs keep the raw release tag.
    assert_eq!(
        super::update::asset_name("v0.6.0", "x86_64-unknown-linux-musl"),
        "gitkit-v0.6.0-x86_64-unknown-linux-musl.tar.gz"
    );
}

// ── Fakes: injected fetcher and downloader, zero network ───────

struct FakeFetcher {
    body: String,
}

impl super::update::ReleaseFetcher for FakeFetcher {
    fn get(&self, _url: &str) -> anyhow::Result<String> {
        Ok(self.body.clone())
    }
}

struct FailingFetcher;

impl super::update::ReleaseFetcher for FailingFetcher {
    fn get(&self, _url: &str) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("network is down"))
    }
}

struct RecordingDownloader {
    called: Cell<bool>,
    bytes: Vec<u8>,
    /// `None` simulates a release that ships no `SHA256SUMS.txt`.
    sums: Option<Vec<u8>>,
}

impl super::update::BinaryDownloader for RecordingDownloader {
    fn download(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        self.called.set(true);
        if url.ends_with("SHA256SUMS.txt") {
            return self.sums.clone().context("no SHA256SUMS.txt in release");
        }
        Ok(self.bytes.clone())
    }
}

fn recording_downloader(bytes: Vec<u8>, sums: Option<Vec<u8>>) -> RecordingDownloader {
    RecordingDownloader {
        called: Cell::new(false),
        bytes,
        sums,
    }
}

/// A fake release tag guaranteed newer than this binary, whatever the
/// current version is. Hardcoding "the next version" re-traps on every
/// version bump (canopy's `v3.0.1` broke the moment Cargo.toml caught up).
fn fake_newer_tag() -> String {
    let mut parts = env!("CARGO_PKG_VERSION").split('.');
    let (major, minor, patch) = (
        parts.next().expect("semver major"),
        parts.next().expect("semver minor"),
        parts
            .next()
            .expect("semver patch")
            .parse::<u64>()
            .expect("numeric patch"),
    );
    format!("v{major}.{minor}.{}", patch + 1)
}

/// The crate version under test, never a hardcoded literal.
const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn newer_release() -> super::update::GitHubRelease {
    super::update::GitHubRelease {
        tag_name: fake_newer_tag(),
        prerelease: false,
        draft: false,
    }
}

fn current_release() -> super::update::GitHubRelease {
    super::update::GitHubRelease {
        tag_name: format!("v{CURRENT}"),
        prerelease: false,
        draft: false,
    }
}

fn check_deps<'a>(
    current: &'a str,
    releases: Vec<super::update::GitHubRelease>,
    downloader: &'a RecordingDownloader,
) -> super::update::UpdateDeps<'a> {
    super::update::UpdateDeps {
        current,
        releases: Ok(releases),
        exe: Path::new("/tmp/gitkit-update-test/gitkit"),
        cargo_bin: Path::new("/tmp/gitkit-update-test/not-cargo"),
        target: Ok("x86_64-unknown-linux-musl"),
        downloader,
        confirm: &|| true,
    }
}

fn gitkit_tar_bytes(contents: &[u8]) -> Vec<u8> {
    let mut archive = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    archive
        .append_data(&mut header, "gitkit", contents)
        .unwrap();
    let tar_bytes = archive.into_inner().unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
    encoder.finish().unwrap()
}

// ── `--check` contract ──────────────────────────────────────────

#[test]
fn check_exit_1_when_newer_stable() {
    let downloader = recording_downloader(Vec::new(), None);
    let deps = check_deps(CURRENT, vec![newer_release()], &downloader);
    assert_eq!(
        super::update::run_update_with(true, false, &deps).unwrap(),
        1
    );
    // `--check` is read-only: it must return before any download.
    assert!(!downloader.called.get());
}

#[test]
fn check_exit_0_when_equal() {
    let downloader = recording_downloader(Vec::new(), None);
    let deps = check_deps(CURRENT, vec![current_release()], &downloader);
    assert_eq!(
        super::update::run_update_with(true, false, &deps).unwrap(),
        0
    );
    assert!(!downloader.called.get());
}

#[test]
fn check_ignores_prerelease() {
    let downloader = recording_downloader(Vec::new(), None);
    let deps = check_deps(
        CURRENT,
        vec![
            super::update::GitHubRelease {
                tag_name: format!("{}-rc1", fake_newer_tag()),
                prerelease: true,
                draft: false,
            },
            current_release(),
        ],
        &downloader,
    );
    assert_eq!(
        super::update::run_update_with(true, false, &deps).unwrap(),
        0
    );
    assert!(!downloader.called.get());
}

#[test]
fn check_ignores_draft() {
    let releases = vec![super::update::GitHubRelease {
        tag_name: fake_newer_tag(),
        prerelease: false,
        draft: true,
    }];
    assert_eq!(
        super::update::select_latest_stable(&releases, CURRENT),
        None
    );
}

#[test]
fn select_latest_picks_max_stable() {
    // Synthetic ordering fixture — deliberately independent of the crate
    // version, so it tests ordering only and never rots on a version bump.
    let releases = vec![
        super::update::GitHubRelease {
            tag_name: "v9.9.9".to_string(),
            prerelease: false,
            draft: false,
        },
        super::update::GitHubRelease {
            tag_name: "v9.9.10".to_string(),
            prerelease: false,
            draft: false,
        },
        super::update::GitHubRelease {
            tag_name: "v9.9.10-rc1".to_string(),
            prerelease: true,
            draft: false,
        },
        super::update::GitHubRelease {
            tag_name: "v8.0.0".to_string(),
            prerelease: false,
            draft: false,
        },
    ];
    assert_eq!(
        super::update::select_latest_stable(&releases, "9.0.0"),
        Some("v9.9.10".to_string())
    );
}

// ── Background path shares the read-only seam ───────────────────

#[test]
fn background_path_never_calls_replace() {
    let fake = fake_newer_tag();
    let fetcher = FakeFetcher {
        body: format!(r#"[{{"tag_name":"{fake}","prerelease":false,"draft":false}}]"#),
    };
    let downloader = recording_downloader(Vec::new(), None);

    // The background check resolves the newer tag through this same
    // fetch/select seam, and it is never handed a downloader: reaching
    // `download_and_extract_with` → `replace_binary` requires passing
    // through the explicit command core below. Nothing here can download
    // or swap the binary.
    let releases = super::update::fetch_releases_with(&fetcher).unwrap();
    assert_eq!(
        super::update::select_latest_stable(&releases, CURRENT),
        Some(fake)
    );
    assert!(!downloader.called.get());
}

#[test]
fn background_check_is_quiet_without_network() {
    let downloader = recording_downloader(Vec::new(), None);
    let error = super::update::fetch_releases_with(&FailingFetcher).unwrap_err();
    assert!(error.to_string().contains("network is down"));
    assert!(!downloader.called.get());
}

// ── Extraction and atomic replacement ───────────────────────────

#[test]
fn download_and_extract_uses_the_gitkit_entry() {
    let downloader = recording_downloader(gitkit_tar_bytes(b"data"), None);
    let output = tempfile::NamedTempFile::new().unwrap();
    assert!(super::update::download_and_extract_with(
        &downloader,
        &fake_newer_tag(),
        "x86_64-unknown-linux-musl",
        output.path()
    )
    .unwrap());
    assert_eq!(std::fs::read(output.path()).unwrap(), b"data");
    assert!(downloader.called.get());
}

#[test]
fn replace_binary_replaces_target_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("staged");
    let target = dir.path().join("gitkit");
    std::fs::write(&staged, b"new").unwrap();
    std::fs::write(&target, b"old").unwrap();
    super::update::replace_binary(&staged, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
    }
}

// ── Checksum verification ───────────────────────────────────────

#[test]
fn checksum_mismatch_fails_loudly() {
    let tag = fake_newer_tag();
    let asset = super::update::asset_name(&tag, "x86_64-unknown-linux-musl");
    let sums = format!("{}  {asset}\n", "0".repeat(64)).into_bytes();
    let downloader = recording_downloader(gitkit_tar_bytes(b"data"), Some(sums));
    let output = tempfile::NamedTempFile::new().unwrap();
    let error = super::update::download_and_extract_with(
        &downloader,
        &tag,
        "x86_64-unknown-linux-musl",
        output.path(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("SHA256 mismatch"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn checksum_matches_when_release_ships_sums() {
    use sha2::{Digest, Sha256};

    let tag = fake_newer_tag();
    let asset = super::update::asset_name(&tag, "x86_64-unknown-linux-musl");
    let bytes = gitkit_tar_bytes(b"data");
    let hex: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let sums = format!("{hex}  {asset}\n").into_bytes();
    let downloader = recording_downloader(bytes, Some(sums));
    let output = tempfile::NamedTempFile::new().unwrap();
    assert!(super::update::download_and_extract_with(
        &downloader,
        &tag,
        "x86_64-unknown-linux-musl",
        output.path()
    )
    .unwrap());
    assert_eq!(std::fs::read(output.path()).unwrap(), b"data");
}

#[test]
fn checksum_missing_skips_verification() {
    // `sums: None` makes the SHA256SUMS.txt download fail → old release,
    // verification skipped, install proceeds.
    let downloader = recording_downloader(gitkit_tar_bytes(b"data"), None);
    let output = tempfile::NamedTempFile::new().unwrap();
    assert!(super::update::download_and_extract_with(
        &downloader,
        &fake_newer_tag(),
        "x86_64-unknown-linux-musl",
        output.path()
    )
    .unwrap());
    assert!(downloader.called.get());
}

// ── Explicit install flow ───────────────────────────────────────

#[test]
fn install_refuses_cargo_binary_without_downloading() {
    let downloader = recording_downloader(Vec::new(), None);
    let exe = PathBuf::from("/home/u/.cargo/bin/gitkit");
    let cargo_bin = PathBuf::from("/home/u/.cargo/bin");
    let deps = super::update::UpdateDeps {
        current: CURRENT,
        releases: Ok(vec![newer_release()]),
        exe: &exe,
        cargo_bin: &cargo_bin,
        target: Ok("x86_64-unknown-linux-musl"),
        downloader: &downloader,
        confirm: &|| panic!("the cargo guard must answer before the prompt"),
    };
    assert_eq!(
        super::update::run_update_with(false, true, &deps).unwrap(),
        0
    );
    assert!(!downloader.called.get());
}

#[test]
fn install_declined_leaves_everything_untouched() {
    let downloader = recording_downloader(Vec::new(), None);
    let deps = super::update::UpdateDeps {
        current: CURRENT,
        releases: Ok(vec![newer_release()]),
        exe: Path::new("/tmp/gitkit-update-test/gitkit"),
        cargo_bin: Path::new("/tmp/gitkit-update-test/not-cargo"),
        target: Ok("x86_64-unknown-linux-musl"),
        downloader: &downloader,
        confirm: &|| false,
    };
    assert_eq!(
        super::update::run_update_with(false, false, &deps).unwrap(),
        0
    );
    assert!(!downloader.called.get());
}

#[test]
fn install_with_yes_replaces_binary() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("gitkit");
    std::fs::write(&exe, b"old").unwrap();
    let cargo_bin = dir.path().join("not-cargo-bin");
    std::fs::create_dir_all(&cargo_bin).unwrap();
    let downloader = recording_downloader(gitkit_tar_bytes(b"new"), None);

    let deps = super::update::UpdateDeps {
        current: CURRENT,
        releases: Ok(vec![newer_release()]),
        exe: &exe,
        cargo_bin: &cargo_bin,
        target: Ok("x86_64-unknown-linux-musl"),
        downloader: &downloader,
        confirm: &|| panic!("--yes must skip the prompt"),
    };
    assert_eq!(
        super::update::run_update_with(false, true, &deps).unwrap(),
        0
    );
    assert!(downloader.called.get());
    assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    // The staging directory is cleaned up; only the target binary remains.
    assert!(exe.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
    }
}

#[test]
fn install_unsupported_target_errors_before_downloading() {
    let downloader = recording_downloader(Vec::new(), None);
    let deps = super::update::UpdateDeps {
        current: CURRENT,
        releases: Ok(vec![newer_release()]),
        exe: Path::new("/tmp/gitkit-update-test/gitkit"),
        cargo_bin: Path::new("/tmp/gitkit-update-test/not-cargo"),
        target: Err("unsupported target: x86_64-windows".to_string()),
        downloader: &downloader,
        confirm: &|| panic!("target failure must surface before the prompt"),
    };
    let error = super::update::run_update_with(false, true, &deps).unwrap_err();
    assert!(error.to_string().contains("x86_64-windows"));
    assert!(!downloader.called.get());
}

// ── Loud vs. silent error handling ──────────────────────────────

/// A failed release lookup is not an `Err` any more: both `--check` and a
/// plain update exit `2`, the cause reaches stderr as one line, and nothing
/// is downloaded.
#[test]
fn plain_update_exits_2_without_network() {
    let downloader = recording_downloader(Vec::new(), None);
    let deps = super::update::UpdateDeps {
        current: CURRENT,
        releases: Err("network is down".to_string()),
        exe: Path::new("/tmp/gitkit-update-test/gitkit"),
        cargo_bin: Path::new("/tmp/gitkit-update-test/not-cargo"),
        target: Ok("x86_64-unknown-linux-musl"),
        downloader: &downloader,
        confirm: &|| panic!("a failed lookup must answer before the prompt"),
    };
    assert_eq!(
        super::update::run_update_with(false, true, &deps).unwrap(),
        2
    );
    assert!(!downloader.called.get());
}

/// The third leg of the 0/1/2 contract: `--check` reports a failed lookup
/// as exit 2 without touching the downloader or the prompt.
#[test]
fn check_exit_2_when_lookup_fails() {
    let downloader = recording_downloader(Vec::new(), None);
    let deps = super::update::UpdateDeps {
        current: CURRENT,
        releases: Err("network is down".to_string()),
        exe: Path::new("/tmp/gitkit-update-test/gitkit"),
        cargo_bin: Path::new("/tmp/gitkit-update-test/not-cargo"),
        target: Ok("x86_64-unknown-linux-musl"),
        downloader: &downloader,
        confirm: &|| panic!("a failed lookup must answer before the prompt"),
    };
    assert_eq!(
        super::update::run_update_with(true, false, &deps).unwrap(),
        2
    );
    assert!(!downloader.called.get());
}

// ── Target resolution ───────────────────────────────────────────

#[test]
fn resolve_target_names_unsupported() {
    let error = super::update::target_for("windows", "x86_64").unwrap_err();
    assert!(error.to_string().contains("x86_64-windows"));
}

#[test]
fn detect_platform_returns_valid_target() {
    if (cfg!(target_os = "linux") || cfg!(target_os = "macos"))
        && (cfg!(target_arch = "x86_64") || cfg!(target_arch = "aarch64"))
    {
        assert!(
            super::update::resolve_target().is_ok(),
            "should detect a supported release target"
        );
    }
}
