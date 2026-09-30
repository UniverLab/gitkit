---
title: Ignore & Attributes
description: Generate .gitignore from templates and apply .gitattributes presets.
order: 5
---

# Ignore & Attributes

## `.gitignore`

Gitkit generates and merges `.gitignore` files from
[gitignore.io](https://www.toptal.com/developers/gitignore) templates plus
its own built-ins (like `agentic` for AI coding agents):

```bash
gitkit ignore add rust,vscode,agentic   # generate/merge templates
gitkit ignore list                      # list available templates
gitkit ignore list python               # filter the list
```

Existing patterns are merged, not overwritten — your manual entries
survive.

The `agentic` template is derived from canopy's platform registry
(https://github.com/UniverLab/canopy-registry): `gitkit` fetches it with a
24-hour cache in `~/.gitkit/agent-registry.toml` and falls back to a snapshot
embedded at build time when offline. Every harness directory is ignored, but
instruction files (`AGENTS.md`, `.cursor/rules/`, …) stay committable and
`.github/` is never ignored.

## `.gitattributes`

```bash
gitkit attributes init
```

Applies a line-endings preset (`* text=auto eol=lf`) and binary-file
attributes so checkouts behave identically across Linux, macOS and
Windows.

Both files can also be configured interactively from the `gitkit` wizard,
which offers a filterable search across all templates.
