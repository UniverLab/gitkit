---
title: CLI Reference
description: Every gitkit command and flag.
order: 8
---

# CLI Reference

```
gitkit [command] [options]
```

Running `gitkit` with no command starts the interactive wizard.

## Core

| Command | Description |
|---|---|
| `gitkit` / `gitkit init` | Interactive wizard to configure the repo |
| `gitkit status` | Show current configuration status, including per-hook health |
| `gitkit status --repair` | Set the executable bit on every dormant hook |
| `gitkit status --strict` | Exit non-zero if any hook is dormant (for CI) |
| `gitkit status --global` | Machine-wide: every repo gitkit has touched, health read from disk |
| `gitkit status --global --prune` | Also remove registry entries whose repo no longer exists |
| `gitkit status --scan <DIR>` | Discover repos with gitkit hooks under DIR and register them |
| `gitkit clone <repo> [dir]` | Clone a repository and run the wizard |
| `gitkit clone -b <branch> <repo>` | Clone a specific branch |

## Hooks

| Command | Description |
|---|---|
| `gitkit hooks add <builtin>` | Install a built-in hook (hook type inferred) |
| `gitkit hooks add <hook> <command>` | Install a custom shell command as a hook |
| `gitkit hooks list` | List installed hooks |
| `gitkit hooks list --available` | Show all built-in hooks with descriptions |
| `gitkit hooks remove <hook>` | Remove an installed hook |
| `gitkit hooks show <hook>` | Print hook content |

## Lock

| Command | Description |
|---|---|
| `gitkit lock` | Block commits until `gitkit unlock` |
| `gitkit lock --reason <msg>` | Set the message shown on a blocked commit |
| `gitkit lock --timeout <duration>` | Auto-expire the lock, e.g. `30m`, `2h` |
| `gitkit lock --push` | Also block pushes (in addition to commits) |
| `gitkit lock --all` | Block both commits and pushes |
| `gitkit lock status` | Show whether a lock is active, its reason and expiry |
| `gitkit lock status --json` | Show lock status as machine-readable JSON with exit code signal |
| `gitkit unlock` | Remove the lock and restore any backed-up hook |

`git commit --no-verify` and `git push --no-verify` bypass the lock — see [Lock](lock.md) for why
that is accepted rather than defended against.

## Uninstall

| Command | Description |
|---|---|
| `gitkit uninstall` | Remove gitkit hooks from every repository it has touched |
| `gitkit uninstall --data` | Also remove local state under `~/.gitkit` (builds, registry) |
| `gitkit uninstall --yes` | Skip the confirmation prompt |
| `gitkit uninstall --dry-run` | Print what would be done without changing anything |

By default, `gitkit uninstall` lists every repository in the registry, shows what hooks are
installed, and asks for confirmation before removing anything. It restores any hand-written hook
that gitkit had absorbed when it first installed its dispatcher. The gitkit binary itself is never
removed — see [Installation](installation.md#uninstall) for how to remove it.

## Ignore

| Command | Description |
|---|---|
| `gitkit ignore add <templates>` | Generate/merge `.gitignore` via gitignore.io |
| `gitkit ignore list [filter]` | List available templates |

## Attributes

| Command | Description |
|---|---|
| `gitkit attributes init` | Apply line-endings preset to `.gitattributes` |

## Config

| Command | Description |
|---|---|
| `gitkit config apply defaults` | `push.autoSetupRemote`, `help.autocorrect`, `diff.algorithm` |
| `gitkit config apply advanced` | `merge.conflictstyle zdiff3`, `rerere.enabled` |
| `gitkit config apply delta` | `core.pager delta` |
| `gitkit config show` | Show current git config values |

Scope: `--global` (all repos) or `--local` (current repo). Default is
`--local` inside a repo, `--global` otherwise.

## Builds

| Command | Description |
|---|---|
| `gitkit build list` | List saved builds |
| `gitkit build save <name>` | Save current repo config as a build |
| `gitkit build apply <name>` | Apply a saved build |
| `gitkit build delete <name>` | Delete a saved build |

## Update

| Command | Description |
|---|---|
| `gitkit update` | Fetch the latest stable release and ask before replacing the running binary |
| `gitkit update --check` | Report only: exit `1` when an update exists, `0` when current; changes nothing |
| `gitkit update --yes` | Skip the confirmation prompt |

`gitkit update` is explicit: it always asks first and the answer defaults to
**No**, so gitkit never rewrites itself behind your back. Drafts, prereleases
and non-semver tags are skipped, the downloaded asset is SHA256-verified
against the release's `SHA256SUMS.txt` when one ships, and the binary is
swapped atomically (same-directory temporary file plus rename). A cargo-managed
install is left alone — the command prints `cargo install --force gitkit`.
Network errors exit non-zero with a message; the background check
(`GITKIT_NO_UPDATE_CHECK`) remains silent.

## Global flags

| Flag | Description |
|---|---|
| `--yes`, `-y` | Skip confirmation prompts |
| `--force`, `-f` | Overwrite existing files |
| `--dry-run` | Preview changes without applying |
| `--help` | Show help for any command |
| `--version` | Show gitkit version |
