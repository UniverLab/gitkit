---
title: Installation
description: Install gitkit with the quick installer, cargo, or from GitHub Releases.
order: 2
---

# Installation

## Quick install (recommended)

**Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/UniverLab/gitkit/main/scripts/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/UniverLab/gitkit/main/scripts/install.ps1 | iex
```

## Via cargo

```bash
cargo install gitkit
```

Available on [crates.io](https://crates.io/crates/gitkit).

## GitHub Releases

Precompiled binaries for Linux x86_64, macOS x86_64/ARM64 and Windows
x86_64 are published on the
[Releases](https://github.com/UniverLab/gitkit/releases) page.

## Self-update

gitkit automatically checks GitHub for newer releases each time it runs and
offers to update if a newer version is available. The update replaces the
running binary in place — no need to reinstall or restart your shell between
commands.

### Explicit update

Prefer to decide yourself? Run the check on demand:

```bash
gitkit update          # asks first (default: No), then updates in place
gitkit update --check  # read-only: exit 0 current, 1 update available, 2 check failed
gitkit update --yes    # skip the prompt
```

Both forms exit `0` when you are up to date (or nothing is installed), `1`
from `--check` when an update is available, and `2` when the check could not
complete — network, DNS, TLS, an HTTP error (≥ 400) or an unparsable
response — printing one stderr line that names the cause.

`gitkit update` fetches the latest **stable** release (drafts and
prereleases are ignored), downloads the asset for your platform — for example
`gitkit-v0.6.0-x86_64-unknown-linux-musl.tar.gz` — verifies its SHA256 against
the release's `SHA256SUMS.txt` when the release ships one, and replaces the
running binary atomically with a same-directory temporary file plus a rename.
The background check below prints at most a one-line notice so it can never
interrupt your work.
`GITKIT_NO_UPDATE_CHECK`
disables only the background check, never the explicit command.

**Behind a proxy:** set `HTTPS_PROXY` (or `HTTP_PROXY`) and, to exclude
hosts, `NO_PROXY` — all three are honoured in upper and lower case, exactly
like the other UniverLab tools.

### Disable update checks

If you prefer to manage updates yourself, disable the check with:

```bash
export GITKIT_NO_UPDATE_CHECK=1
```

Add this to your shell profile to make it permanent.

### Cargo-installed versions

If gitkit was installed with `cargo install gitkit`, the auto-updater will
detect this, refuse to replace the file cargo owns, and tell you to run:

```bash
installed with cargo — run: cargo install --force gitkit
```

This is because cargo manages the installation and needs to be involved in
the update to maintain consistency.

## Uninstall

First, remove gitkit's hooks from every repository it has touched:

```bash
gitkit uninstall
```

This lists every repository in the registry, shows what hooks are installed, and asks for
confirmation before removing anything. It restores any hand-written hook that gitkit had absorbed
when it first installed its dispatcher. Add `--data` to also remove local state under `~/.gitkit`
(builds, registry).

Then remove the binary itself:

**Linux / macOS:**

```bash
rm -f ~/.local/bin/gitkit
```

**Windows (PowerShell):**

```powershell
Remove-Item "$env:LOCALAPPDATA\gitkit\gitkit.exe" -Force
```

If gitkit was installed via `cargo install gitkit`, remove it with:

```bash
cargo uninstall gitkit
```
