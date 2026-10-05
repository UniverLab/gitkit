---
title: Gitkit
description: Set up a git repo the way you actually work — hooks, .gitignore, .gitattributes and git config in one guided flow.
order: 1
---

# Gitkit

Gitkit sets up a git repository the way you actually work: one guided flow
for hooks, `.gitignore`, `.gitattributes` and git config. It is a single
Rust binary — no Node.js, no Python, no runtime dependencies.

## Why gitkit

Every new repository needs the same ritual: pick a `.gitignore`, normalize
line endings, install a commit-message hook, set the git config options
you always set. Doing it by hand is error-prone; doing it with four
different tools (husky, gitignore.io, dotfiles, …) drags in runtimes and
copy-paste. Gitkit folds the whole ritual into one interactive wizard —
and lets you **save the result as a build** you can re-apply to any
project with one command.

- **Guided repo setup** — `gitkit` (no arguments) walks you through
  everything, showing what is already configured.
- **Status overview** — `gitkit status` shows hooks, ignore patterns,
  attributes and config at a glance; `gitkit status --global` shows every
  repository gitkit has touched, machine-wide.
- **Clone and bootstrap** — `gitkit clone <url>` clones and drops straight
  into the wizard.
- **Hook management** — built-in hooks (conventional commits, secret
  detection, branch naming) or your own shell command.
- **Agent lock** — block commits and/or pushes locally and reversibly for
  the duration of an agent session with `gitkit lock`.
- **Ignore & attribute presets** — all gitignore.io templates plus
  built-ins, line-ending and binary presets.
- **Curated git config** — practical presets with `--global`/`--local`
  scope and idempotency detection.
- **Builds** — save a configuration once, apply it everywhere.
- **Self-update** — gitkit checks GitHub for newer releases in the background,
  and `gitkit update` installs a newer stable release on demand — always after
  asking you first.

## How the documentation is organized

- [Installation](installation.md) — install, update (background check or the explicit `gitkit update`), and uninstall.
- [Quick Start](quickstart.md) — the wizard and the one-liner workflow.
- [Hooks](hooks.md) — built-in and custom hooks.
- [Lock](lock.md) — block commits and/or pushes for an agent session, and its limits.
- [Ignore & Attributes](ignore-and-attributes.md) — `.gitignore` and `.gitattributes`.
- [Config Presets](config-presets.md) — curated git config, scopes, idempotency.
- [Builds](builds.md) — save and reuse configurations.
- [CLI Reference](cli-reference.md) — every command and flag.

## Part of UniverLab

Gitkit is an experiment of [UniverLab](https://github.com/UniverLab),
an open computational laboratory. It follows the lab's engineering
principles: one tool one job, reproducibility first, offline-friendly
design.
