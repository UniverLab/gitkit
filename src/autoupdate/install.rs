//! Binary replacement mechanics: detects a cargo-managed install and,
//! otherwise, downloads, verifies, and swaps in the new binary.
//!
//! Never writes to a hardcoded install directory. Resolves
//! `std::env::current_exe()` and either replaces that file in place or,
//! when it is managed by `cargo install`, leaves it untouched and tells
//! the user to run `cargo install --force` instead.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::GITHUB_REPO;

enum Outcome {
    Updated,
    CargoManaged,
}

/// Entry point, called after the user confirms the "Install now?" prompt.
pub(super) fn run(latest_tag: &str) {
    match install(latest_tag) {
        Ok(Outcome::Updated) => {
            println!("  \x1b[32m✓\x1b[0m Updated! Restart your terminal to use the new version.");
        }
        Ok(Outcome::CargoManaged) => {
            println!("  ℹ  gitkit was installed with cargo — the auto-updater won't touch it.");
            println!("     Run this instead:");
            println!();
            println!("      cargo install --force gitkit");
            println!();
        }
        Err(e) => {
            eprintln!("  ⚠ Update failed: {e:#}");
        }
    }
}

fn install(latest_tag: &str) -> Result<Outcome> {
    let current_exe =
        std::env::current_exe().context("failed to resolve current executable path")?;

    if is_cargo_managed(&current_exe) {
        return Ok(Outcome::CargoManaged);
    }

    let dir = current_exe
        .parent()
        .context("executable path has no parent directory")?;
    let file_name = current_exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("gitkit");
    let tmp_path = dir.join(format!(".{file_name}.update"));

    let (arch, os) = detect_platform()?;
    update_binary_at(&tmp_path, &current_exe, |out| {
        download_and_extract(latest_tag, arch, os, out)
    })?;

    Ok(Outcome::Updated)
}

// ── Cargo-managed detection ─────────────────────────────────────

fn resolve_cargo_root(
    install_root_env: Option<String>,
    cargo_home_env: Option<String>,
    home_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(root) = install_root_env.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(root));
    }
    if let Some(home) = cargo_home_env.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(home));
    }
    home_dir.map(|h| h.join(".cargo"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

fn cargo_install_root() -> Option<PathBuf> {
    resolve_cargo_root(
        std::env::var("CARGO_INSTALL_ROOT").ok(),
        std::env::var("CARGO_HOME").ok(),
        home_dir(),
    )
}

fn is_cargo_managed_with_root(exe_path: &Path, root: Option<PathBuf>) -> bool {
    let Some(root) = root else {
        return false;
    };
    let bin_dir = root.join("bin");
    match (exe_path.canonicalize(), bin_dir.canonicalize()) {
        (Ok(exe), Ok(bin)) => exe.starts_with(bin),
        _ => false,
    }
}

fn is_cargo_managed(exe_path: &Path) -> bool {
    is_cargo_managed_with_root(exe_path, cargo_install_root())
}

// ── Download + replace ──────────────────────────────────────────

/// Fetches into `tmp_path` via `fetch`, makes it executable, and atomically
/// replaces `target` with it. On any failure, `tmp_path` is removed and
/// `target` is left untouched. `tmp_path` must sit beside `target` (same
/// directory) so the replace below never crosses a filesystem boundary.
fn update_binary_at(
    tmp_path: &Path,
    target: &Path,
    fetch: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let result = (|| -> Result<()> {
        fetch(tmp_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp_path, std::fs::Permissions::from_mode(0o755))
                .with_context(|| {
                    format!(
                        "failed to set executable permission on {}",
                        tmp_path.display()
                    )
                })?;
        }

        replace_binary(tmp_path, target)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(tmp_path);
    }

    result
}

fn replace_binary(tmp_path: &Path, target: &Path) -> Result<()> {
    // `tmp_path` sits beside `target`, so this is a same-filesystem rename
    // and should always succeed; the copy fallback only guards against the
    // rare case of a bind mount or similar splitting the directory across
    // devices.
    if std::fs::rename(tmp_path, target).is_err() {
        std::fs::copy(tmp_path, target)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    anyhow::anyhow!(
                        "permission denied replacing {} — check that the containing directory is writable by the current user",
                        target.display()
                    )
                } else {
                    anyhow::Error::new(e).context(format!("failed to replace {} (copy fallback)", target.display()))
                }
            })?;
        let _ = std::fs::remove_file(tmp_path);
    }
    Ok(())
}

fn download_and_extract(tag: &str, arch: &str, os: &str, output: &Path) -> Result<()> {
    // Asset naming has one source of truth: the explicit updater, which is
    // read against the release workflow (`{binary}-{tag}-{target}.tar.gz`).
    let archive_name = super::update::asset_name(tag, &format!("{arch}-{os}"));
    let url = format!("https://github.com/{GITHUB_REPO}/releases/download/{tag}/{archive_name}");

    let resp = ureq::get(&url)
        .set("User-Agent", "gitkit-autoupdate")
        .call()
        .with_context(|| format!("failed to download {url}"))?;

    extract_binary(resp.into_reader(), output)
}

fn extract_binary(reader: impl std::io::Read, output: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(decoder);

    let mut found = false;
    for entry in archive
        .entries()
        .context("corrupt archive: failed to read entries")?
    {
        let mut entry = entry.context("corrupt archive: failed to read entry")?;
        let path = entry
            .path()
            .context("corrupt archive: invalid entry path")?;
        if path.file_name().is_some_and(|n| n == "gitkit") {
            entry
                .unpack(output)
                .context("failed to extract binary from archive")?;
            found = true;
            break;
        }
    }

    if !found {
        anyhow::bail!("binary not found in archive");
    }

    let meta = std::fs::metadata(output)
        .with_context(|| format!("failed to stat extracted binary at {}", output.display()))?;
    if meta.len() == 0 {
        let _ = std::fs::remove_file(output);
        anyhow::bail!("extracted binary is empty");
    }

    Ok(())
}

fn detect_platform() -> Result<(&'static str, &'static str)> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => anyhow::bail!("unsupported architecture: {other}"),
    };
    let os = match std::env::consts::OS {
        "linux" => "unknown-linux-musl",
        "macos" => "apple-darwin",
        other => anyhow::bail!("unsupported OS: {other}"),
    };
    Ok((arch, os))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (name, content) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append_data(&mut header, name, *content).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn extract_binary_finds_named_entry() {
        let archive = make_tar_gz(&[("gitkit", b"fake-binary-contents")]);
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        extract_binary(std::io::Cursor::new(archive), &output).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"fake-binary-contents");
    }

    #[test]
    fn extract_binary_rejects_missing_entry() {
        let archive = make_tar_gz(&[("other-file", b"contents")]);
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let err = extract_binary(std::io::Cursor::new(archive), &output).unwrap_err();
        assert!(err.to_string().contains("not found"));
        assert!(!output.exists());
    }

    #[test]
    fn extract_binary_rejects_empty_binary() {
        let archive = make_tar_gz(&[("gitkit", b"")]);
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let err = extract_binary(std::io::Cursor::new(archive), &output).unwrap_err();
        assert!(err.to_string().contains("empty"));
        assert!(!output.exists());
    }

    #[test]
    fn extract_binary_rejects_corrupt_archive() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let result = extract_binary(std::io::Cursor::new(b"not a gzip stream".to_vec()), &output);
        assert!(result.is_err());
        assert!(!output.exists());
    }

    #[test]
    fn update_binary_at_replaces_target_and_sets_executable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gitkit");
        std::fs::write(&target, b"old-binary").unwrap();
        let tmp_path = dir.path().join(".gitkit.update");

        update_binary_at(&tmp_path, &target, |out| {
            std::fs::write(out, b"new-binary")?;
            Ok(())
        })
        .unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new-binary");
        assert!(!tmp_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    #[test]
    fn update_binary_at_leaves_target_untouched_when_fetch_fails() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gitkit");
        std::fs::write(&target, b"original-binary").unwrap();
        let tmp_path = dir.path().join(".gitkit.update");

        let result = update_binary_at(&tmp_path, &target, |_out| {
            anyhow::bail!("simulated download failure")
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"original-binary");
        assert!(!tmp_path.exists());
    }

    #[test]
    fn update_binary_at_cleans_up_tmp_file_when_extraction_writes_then_fails() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gitkit");
        std::fs::write(&target, b"original-binary").unwrap();
        let tmp_path = dir.path().join(".gitkit.update");

        let result = update_binary_at(&tmp_path, &target, |out| {
            std::fs::write(out, b"partial-garbage")?;
            anyhow::bail!("corrupt archive")
        });

        assert!(result.is_err());
        assert!(!tmp_path.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"original-binary");
    }

    #[test]
    fn resolve_cargo_root_prefers_install_root() {
        let root = resolve_cargo_root(
            Some("/opt/install-root".to_string()),
            Some("/opt/cargo-home".to_string()),
            Some(PathBuf::from("/home/user")),
        );
        assert_eq!(root, Some(PathBuf::from("/opt/install-root")));
    }

    #[test]
    fn resolve_cargo_root_falls_back_to_cargo_home() {
        let root = resolve_cargo_root(
            None,
            Some("/opt/cargo-home".to_string()),
            Some(PathBuf::from("/home/user")),
        );
        assert_eq!(root, Some(PathBuf::from("/opt/cargo-home")));
    }

    #[test]
    fn resolve_cargo_root_falls_back_to_home_dot_cargo() {
        let root = resolve_cargo_root(None, None, Some(PathBuf::from("/home/user")));
        assert_eq!(root, Some(PathBuf::from("/home/user/.cargo")));
    }

    #[test]
    fn resolve_cargo_root_ignores_empty_env_values() {
        let root = resolve_cargo_root(
            Some(String::new()),
            Some(String::new()),
            Some(PathBuf::from("/home/user")),
        );
        assert_eq!(root, Some(PathBuf::from("/home/user/.cargo")));
    }

    #[test]
    fn is_cargo_managed_detects_path_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_root = dir.path().join("cargo");
        let bin_dir = cargo_root.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe = bin_dir.join("gitkit");
        std::fs::write(&exe, b"binary").unwrap();

        assert!(is_cargo_managed_with_root(&exe, Some(cargo_root)));
    }

    #[test]
    fn is_cargo_managed_rejects_path_outside_root() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_root = dir.path().join("cargo");
        std::fs::create_dir_all(cargo_root.join("bin")).unwrap();
        let other_dir = dir.path().join("elsewhere");
        std::fs::create_dir_all(&other_dir).unwrap();
        let exe = other_dir.join("gitkit");
        std::fs::write(&exe, b"binary").unwrap();

        assert!(!is_cargo_managed_with_root(&exe, Some(cargo_root)));
    }

    #[test]
    fn is_cargo_managed_treats_uncanonicalizable_path_as_not_cargo() {
        let dir = tempfile::tempdir().unwrap();
        let cargo_root = dir.path().join("cargo");
        std::fs::create_dir_all(cargo_root.join("bin")).unwrap();
        let missing_exe = dir.path().join("does-not-exist");

        assert!(!is_cargo_managed_with_root(&missing_exe, Some(cargo_root)));
    }

    #[test]
    fn is_cargo_managed_with_no_root_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("gitkit");
        std::fs::write(&exe, b"binary").unwrap();

        assert!(!is_cargo_managed_with_root(&exe, None));
    }
}
