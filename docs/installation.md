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
gitkit update --check  # read-only: exit 1 when an update exists, 0 when current
gitkit update --yes    # skip the prompt
```

`gitkit update` fetches the latest **stable** release (drafts and
prereleases are ignored), downloads the asset for your platform — for example
`gitkit-v0.6.0-x86_64-unknown-linux-musl.tar.gz` — verifies its SHA256 against
the release's `SHA256SUMS.txt` when the release ships one, and replaces the
running binary atomically with a same-directory temporary file plus a rename.
A network or API failure exits non-zero with a message; the background check
below stays silent so it can never interrupt your work. `GITKIT_NO_UPDATE_CHECK`
disables only the background check, never the explicit command.

### Disable update checks

If you prefer to manage updates yourself, disable the check with:

```bash
export GITKIT_NO_UPDATE_CHECK=1
```

Add this to your shell profile to make it permanent.

### Cargo-installed versions

If gitkit was installed with `cargo install gitkit`, the auto-updater will
detect this and ask you to update using cargo instead:

```bash
cargo install --force gitkit
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
