#!/bin/sh
# Regenerate src/ignore/agent_registry_snapshot.toml from the live canopy registry.
# Usage: scripts/regen-agent-registry.sh
# The embedded snapshot is the offline fallback for the `agentic` template.
set -eu

BASE="https://raw.githubusercontent.com/UniverLab/canopy-registry/main"
OUT="src/ignore/agent_registry_snapshot.toml"

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT INT TERM

curl -fsSL "$BASE/index.toml" -o "$tmpdir/index.toml"

names=$(awk -F'"' '/^name = /{print $2}' "$tmpdir/index.toml")
if [ -z "$names" ]; then
    echo "regen-agent-registry: no platform names found in index.toml" >&2
    exit 1
fi

{
    echo "# Merged canopy platform registry snapshot for the gitkit \`agentic\` template."
    echo "# Source: $BASE (index.toml + platforms/<name>.toml)."
    echo "# Do not edit by hand. Regenerate with: scripts/regen-agent-registry.sh"
    echo "# Only name, project_paths, instruction_file and instruction_precedence are kept."
    echo "fetched_at = $(date +%s)"
} > "$tmpdir/snapshot.toml"

for name in $names; do
    curl -fsSL "$BASE/platforms/$name.toml" -o "$tmpdir/$name.toml"
    if ! grep -q '^project_paths = ' "$tmpdir/$name.toml"; then
        echo "regen-agent-registry: platform '$name' has no project_paths yet" >&2
        exit 1
    fi
    if ! grep -q '^instruction_file = ' "$tmpdir/$name.toml"; then
        echo "regen-agent-registry: platform '$name' has no instruction_file yet" >&2
        exit 1
    fi
    {
        echo ""
        echo "[[platforms]]"
        # Top-level keys only: stop at the first section header. Single-line
        # keys print as-is; array keys keep printing continuation lines
        # until the closing bracket.
        awk '
            /^\[/ { exit }
            /^(name|instruction_file) = / { print; next }
            /^(project_paths|instruction_precedence) = / { keep = 1 }
            keep { print }
            keep && /\]/ { keep = 0 }
        ' "$tmpdir/$name.toml"
    } >> "$tmpdir/snapshot.toml"
done

mv "$tmpdir/snapshot.toml" "$OUT"
count=$(grep -c '^name = ' "$OUT")
echo "regen-agent-registry: wrote $OUT ($count platforms)"
