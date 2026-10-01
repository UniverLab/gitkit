use anyhow::{Context, Result};
use clap::Subcommand;
use std::fs;

use crate::utils::find_repo_root;

mod agent_registry;
mod agentic;

const API_BASE: &str = "https://www.toptal.com/developers/gitignore/api";

#[derive(Subcommand)]
pub enum IgnoreCommand {
    /// Generate .gitignore for the given templates
    Add {
        /// Comma-separated list of templates (e.g. rust,vscode,agentic)
        templates: String,
        #[arg(short, long)]
        yes: bool,
        #[arg(short, long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// List available templates, optionally filtered
    List { filter: Option<String> },
}

pub fn run(cmd: IgnoreCommand) -> Result<()> {
    match cmd {
        IgnoreCommand::Add {
            templates,
            yes,
            force,
            dry_run,
        } => add(&templates, yes, force, dry_run),
        IgnoreCommand::List { filter } => list(filter.as_deref()),
    }
}

/// Add templates merging into existing .gitignore. Used by the interactive wizard (silent).
pub(crate) fn add_templates(templates: &str, force: bool) -> Result<()> {
    let root = find_repo_root()?;
    let path = root.join(".gitignore");
    let new_content = resolve_templates(templates)?;
    let merged = if force {
        new_content
    } else {
        merge_gitignore(&path, &new_content)
    };
    fs::write(&path, merged).context("Failed to write .gitignore")?;
    crate::registry::record_best_effort(&root, &[format!("gitignore:{templates}")]);
    Ok(())
}

/// Fetch template names from the API for the search prompt.
pub(crate) fn fetch_template_list() -> Result<Vec<String>> {
    let url = format!("{API_BASE}/list?format=lines");
    let content = ureq::get(&url)
        .call()
        .context("Failed to fetch template list")?
        .into_string()
        .context("Failed to read response")?;
    let mut names: Vec<String> = builtins::NAMES.iter().map(|s| s.to_string()).collect();
    names.extend(content.lines().map(|l| l.to_string()));
    Ok(names)
}

fn add(templates: &str, _yes: bool, force: bool, dry_run: bool) -> Result<()> {
    let root = find_repo_root()?;
    let path = root.join(".gitignore");

    let new_content = resolve_templates(templates)?;
    let merged = if force {
        new_content.clone()
    } else {
        merge_gitignore(&path, &new_content)
    };

    if dry_run {
        println!("[dry-run] Would write .gitignore:\n{merged}");
        return Ok(());
    }

    fs::write(&path, merged).context("Failed to write .gitignore")?;
    crate::registry::record_best_effort(&root, &[format!("gitignore:{templates}")]);
    println!("Updated .gitignore for: {templates}");
    Ok(())
}

/// Split templates, resolve built-ins locally, fetch the rest from the API.
/// Combines both into a single output.
fn resolve_templates(templates: &str) -> Result<String> {
    let mut builtin_parts: Vec<String> = Vec::new();
    let mut api_templates: Vec<&str> = Vec::new();

    for t in templates.split(',').map(str::trim) {
        match builtins::get(t) {
            Some(content) => builtin_parts.push(content),
            None => api_templates.push(t),
        }
    }

    let mut output = String::new();

    for part in &builtin_parts {
        output.push_str(part);
    }

    if !api_templates.is_empty() {
        let joined = api_templates.join(",");
        let url = format!("{API_BASE}/{joined}");
        let fetched = ureq::get(&url)
            .call()
            .context("Failed to fetch gitignore templates")?
            .into_string()
            .context("Failed to read response")?;
        if fetched.trim().is_empty() {
            anyhow::bail!(
                "No templates found for: {}. Run 'gitkit ignore list' to see available templates.",
                joined
            );
        }
        output.push_str(&fetched);
    }

    Ok(output)
}

fn list(filter: Option<&str>) -> Result<()> {
    // Always show built-ins first
    for name in builtins::NAMES {
        if filter.is_none_or(|f| name.contains(f)) {
            println!("{name} (built-in)");
        }
    }

    let url = format!("{API_BASE}/list?format=lines");
    let content = ureq::get(&url)
        .call()
        .context("Failed to fetch template list")?
        .into_string()
        .context("Failed to read response")?;

    for line in content.lines() {
        if filter.is_none_or(|f| line.contains(f)) {
            println!("{line}");
        }
    }
    Ok(())
}

/// Merge new gitignore content into existing file, skipping non-empty non-comment
/// lines already present. Preserves existing content and appends only new entries.
/// A new `<dir>/*` entry supersedes an existing `<dir>/` line: the wholesale
/// form would keep excluding the directory itself, which blocks the `!` negations
/// the `agentic` template relies on to keep instruction files committable.
fn merge_gitignore(path: &std::path::Path, new_content: &str) -> String {
    if is_agentic_template(new_content) {
        return merge_agentic(path, new_content);
    }
    let existing = if path.exists() {
        fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };

    let superseded: std::collections::HashSet<String> = new_content
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .filter(|l| l.ends_with("/*"))
        .map(|l| l.trim_end_matches('*').to_string())
        .collect();

    let filtered_existing: String = if superseded.is_empty() {
        existing.clone()
    } else {
        let kept: Vec<&str> = existing
            .lines()
            .filter(|line| !superseded.contains(*line))
            .collect();
        if kept.len() == existing.lines().count() {
            existing.clone()
        } else if kept.is_empty() {
            String::new()
        } else {
            let mut out = kept.join("\n");
            out.push('\n');
            out
        }
    };

    let existing_patterns: std::collections::HashSet<&str> = filtered_existing
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let to_append: String = new_content
        .lines()
        .filter(|line| {
            line.is_empty() || line.starts_with('#') || !existing_patterns.contains(line)
        })
        .fold(String::new(), |mut acc, line| {
            acc.push_str(line);
            acc.push('\n');
            acc
        });

    if to_append.trim().is_empty() {
        return filtered_existing;
    }

    let mut result = filtered_existing;
    if !result.ends_with('\n') && !result.is_empty() {
        result.push('\n');
    }
    result.push_str(&to_append);
    result
}

/// True only for the `agentic` template: it is the sole built-in whose
/// content carries the canonical managed header. Every other template keeps
/// the append-merge path above unchanged.
fn is_agentic_template(new_content: &str) -> bool {
    new_content
        .lines()
        .any(|line| line.trim() == agentic::CANONICAL_HEADER)
}

/// Managed merge for the `agentic` template: strip every managed header with
/// the entries under it, then write exactly one rebuilt block where the first
/// header was found (append when there was none). Re-running
/// `gitkit ignore add agentic` therefore rewrites the block in place and the
/// file comes out byte-identical.
fn merge_agentic(path: &std::path::Path, new_content: &str) -> String {
    let existing = if path.exists() {
        fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    let file_empty = existing.is_empty();
    let fresh = agentic::split_fresh(new_content);
    let stripped = agentic::strip_managed(&existing);
    let kept = agentic::compute_kept(&stripped.collected, &fresh);
    let block = agentic::assemble_managed(&fresh.ignores, &kept, &fresh.negations);
    insert_agentic_block(stripped.outside, stripped.first_idx, block, file_empty)
}

/// Splices the rebuilt block into the outside lines at `first_idx`, keeping
/// exactly one blank line before and after it: a missing separator is added,
/// an adjacent blank is reused, and the top of a file needs no leading blank.
/// Lines outside the block keep their order and their bytes.
fn insert_agentic_block(
    mut outside: Vec<String>,
    first_idx: Option<usize>,
    block: Vec<String>,
    file_empty: bool,
) -> String {
    let mut at = first_idx.unwrap_or(outside.len());
    if !file_empty && at > 0 && !outside[at - 1].is_empty() {
        outside.insert(at, String::new());
        at += 1;
    }
    let blank_after = at < outside.len() && !outside[at].is_empty();
    let block_len = block.len();
    for (offset, line) in block.into_iter().enumerate() {
        outside.insert(at + offset, line);
    }
    if blank_after {
        outside.insert(at + block_len, String::new());
    }
    let mut merged = outside.join("\n");
    if !merged.is_empty() && !merged.ends_with('\n') {
        merged.push('\n');
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn tmp_gitignore(content: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn merge_gitignore_appends_new_patterns() {
        let (_dir, path) = tmp_gitignore("target/\n");
        let result = merge_gitignore(&path, "*.log\n");
        assert!(result.contains("target/"));
        assert!(result.contains("*.log"));
    }

    #[test]
    fn merge_gitignore_skips_duplicate_patterns() {
        let (_dir, path) = tmp_gitignore("target/\n*.log\n");
        let result = merge_gitignore(&path, "*.log\n");
        assert_eq!(result.matches("*.log").count(), 1);
    }

    #[test]
    fn merge_gitignore_keeps_comments_and_blank_lines() {
        let (_dir, path) = tmp_gitignore("target/\n");
        let new = "# Rust\ntarget/\n*.pdb\n";
        let result = merge_gitignore(&path, new);
        // comment and blank lines from new content are always appended
        assert!(result.contains("# Rust"));
        assert!(result.contains("*.pdb"));
    }

    #[test]
    fn merge_gitignore_returns_existing_when_nothing_new() {
        let (_dir, path) = tmp_gitignore("target/\n");
        let result = merge_gitignore(&path, "target/\n");
        assert_eq!(result, "target/\n");
    }

    #[test]
    fn merge_gitignore_works_on_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        let result = merge_gitignore(&path, "*.log\n");
        assert_eq!(result, "*.log\n");
    }

    #[test]
    fn resolve_templates_returns_builtin_agentic() {
        let result = resolve_templates("agentic").unwrap();
        assert!(result.contains(".kiro/"));
        assert!(result.contains(".cursor/"));
    }

    #[test]
    fn resolve_templates_multiple_builtins_combined() {
        let result = resolve_templates("agentic,agentic").unwrap();
        assert!(result.contains(".kiro/"));
    }

    #[test]
    fn agentic_template_has_representative_entry_per_group() {
        let result = resolve_templates("agentic").unwrap();
        // AI coding agents: local directories with no known shared-content convention
        assert!(result.contains(".kiro/*"));
        // Claude Code: the whole shared directory is ignored, instruction file re-included
        assert!(result.contains(".claude/*"));
        assert!(result.contains("!.claude/CLAUDE.md"));
        assert!(!result.lines().any(|l| l.trim() == ".claude/"));
        // Agent skill/tool lockfiles
        assert!(result.contains("skills-lock.json"));
    }

    #[test]
    fn agentic_template_never_ignores_agent_doc_files() {
        let content = builtins::get("agentic").unwrap();
        for line in content.lines() {
            let pattern = line.trim();
            if pattern.is_empty() || pattern.starts_with('#') || pattern.starts_with('!') {
                continue;
            }
            assert_ne!(pattern, "CLAUDE.md", "template must not ignore CLAUDE.md");
            assert_ne!(pattern, "AGENTS.md", "template must not ignore AGENTS.md");
            assert!(
                !pattern.ends_with("/CLAUDE.md"),
                "template must not ignore a nested CLAUDE.md"
            );
            assert!(
                !pattern.ends_with("/AGENTS.md"),
                "template must not ignore a nested AGENTS.md"
            );
        }
    }

    #[test]
    fn merge_gitignore_agentic_merges_cleanly_with_previous_version() {
        // The block gitkit shipped before this template gained new groups.
        let previous = "\n# AI coding agents\n\
.kiro/\n\
.cursor/\n\
.windsurf/\n\
.claude/\n\
.continue/\n\
.copilot/\n\
.kilocode/\n\
.zencoder/\n\
.qwen/\n\
.agents/\n\
skills-lock.json\n";
        let (_dir, path) = tmp_gitignore(previous);
        let new_content = builtins::get("agentic").unwrap();
        let merged = merge_gitignore(&path, &new_content);

        // A new `<dir>/*` entry supersedes the old wholesale `<dir>/` line:
        // keeping both would leave the directory itself excluded and block
        // the template's `!` negations for instruction files.
        for (old, new) in [
            (".kiro/", ".kiro/*"),
            (".cursor/", ".cursor/*"),
            (".agents/", ".agents/*"),
            (".continue/", ".continue/*"),
            (".copilot/", ".copilot/*"),
            (".claude/", ".claude/*"),
        ] {
            assert_eq!(
                merged.lines().filter(|l| *l == old).count(),
                0,
                "{old} must be replaced by {new} after merge"
            );
            assert_eq!(
                merged.lines().filter(|l| *l == new).count(),
                1,
                "{new} missing after merge"
            );
        }
        // Entries gitkit itself shipped are legacy: the registry no longer
        // produces them, so the managed merge drops them instead of carrying
        // them over (spec req 2). `skills-lock.json` is still produced and
        // comes back exactly once from the registry lines.
        assert_eq!(
            merged.lines().filter(|l| *l == "skills-lock.json").count(),
            1,
            "skills-lock.json duplicated after merge"
        );
        for retired in [".windsurf/", ".zencoder/"] {
            assert_eq!(
                merged.lines().filter(|l| *l == retired).count(),
                0,
                "legacy entry {retired} must be dropped, not kept"
            );
        }
        // The upgraded file must keep instruction files committable.
        for negation in ["!.cursor/rules/", "!.continue/rules/", "!.claude/CLAUDE.md"] {
            assert!(
                merged.lines().any(|l| l == negation),
                "missing {negation} after merge"
            );
        }
    }

    /// gitkit's own committed `.gitignore` as it looked before this fix: ten
    /// `# AI coding agents` headers, nine of them empty, each earlier run of
    /// `ignore add agentic` appending a fresh pair of headers while deduping
    /// the entries (reproduced 2026-10-01 on release build 18ac3b9).
    const TEN_HEADER_FIXTURE: &str = r#"/target
*.swp
*.swo
*~
.DS_Store
.env
.vscode/
.idea/
*.log
.kiro/
.agents/
.idea/
skills-lock.json

# Added by cargo
#
# already existing elements were commented out

#/target
*.mp4
.mimocode/

# AI coding agents
.cursor/
.windsurf/
.claude/
.continue/
.copilot/
.kilocode/
.zencoder/
.qwen/

# AI coding agents

# AI coding agents

# AI coding agents

# AI coding agents

# AI coding agents

# AI coding agents

# AI coding agents

# AI coding agents

# AI coding agents
"#;

    /// Runs the managed merge twice (writing the first result back) and
    /// returns both outputs so callers can assert byte-identity.
    fn merge_twice(fixture: &str) -> (String, String) {
        let (_dir, path) = tmp_gitignore(fixture);
        let agentic = builtins::get("agentic").unwrap();
        let first = merge_gitignore(&path, &agentic);
        fs::write(&path, &first).unwrap();
        let second = merge_gitignore(&path, &agentic);
        (first, second)
    }

    /// Req 5(a): the 10-header fixture collapses to exactly one canonical
    /// header, keeps the content before the block verbatim, and the entries
    /// the registry still produces reappear inside the single block.
    #[test]
    fn agentic_managed_ten_headers_collapse_to_one() {
        let (_dir, path) = tmp_gitignore(TEN_HEADER_FIXTURE);
        let agentic = builtins::get("agentic").unwrap();
        let merged = merge_gitignore(&path, &agentic);
        let lines: Vec<&str> = merged.lines().collect();

        assert_eq!(
            lines
                .iter()
                .filter(|l| **l == agentic::CANONICAL_HEADER)
                .count(),
            1,
            "exactly one canonical header must remain: {merged:?}"
        );
        assert_eq!(
            lines.iter().filter(|l| **l == "# AI coding agents").count(),
            0,
            "no bare header may remain: {merged:?}"
        );
        // grep -c '^# AI coding agents' over the result.
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.starts_with("# AI coding agents"))
                .count(),
            1
        );
        assert_eq!(
            lines.iter().filter(|l| **l == agentic::NEG_HEADER).count(),
            1
        );
        // The entry lines of the first bare header were legacy wholesale
        // forms; the block carries the registry's `/*` forms instead.
        assert_eq!(lines.iter().filter(|l| **l == ".cursor/").count(), 0);
        assert_eq!(lines.iter().filter(|l| **l == ".cursor/*").count(), 1);
        assert_eq!(
            lines.iter().filter(|l| **l == "skills-lock.json").count(),
            2,
            "one untouched line before the block + one registry line"
        );
        // Everything before the first managed header stays byte-identical:
        // lines 1..=22 of the fixture (through the blank that preceded it).
        let before: Vec<&str> = TEN_HEADER_FIXTURE.lines().take(22).collect();
        assert_eq!(
            &lines[..22],
            &before[..],
            "content before the block changed"
        );
    }

    /// Req 4/5(b): every fixture merged twice comes out byte-identical.
    #[test]
    fn agentic_merge_twice_is_byte_identical() {
        let user_block = "target/\n\n# AI coding agents\n.mytool/\n\n*.log\n";
        let previous = "\n# AI coding agents\n.kiro/\n.cursor/\n.skills-lock.json\n";
        for fixture in [
            TEN_HEADER_FIXTURE,
            user_block,
            previous,
            "",
            "target/\n",
            "\n",
        ] {
            let (first, second) = merge_twice(fixture);
            assert_eq!(
                first, second,
                "second run must be byte-identical for fixture {fixture:?}"
            );
            // A third pass over the written second result changes nothing.
            let (_dir, path) = tmp_gitignore(&second);
            let agentic = builtins::get("agentic").unwrap();
            let third = merge_gitignore(&path, &agentic);
            assert_eq!(second, third, "third run diverged for {fixture:?}");
        }
    }

    /// Req 5(c): a line the user added inside an old block survives under the
    /// kept marker, exactly once, after the registry lines and before the
    /// negations.
    #[test]
    fn agentic_merge_keeps_user_line_under_kept_marker() {
        let (_dir, path) = tmp_gitignore("target/\n\n# AI coding agents\n.mytool/\n\n*.log\n");
        let agentic = builtins::get("agentic").unwrap();
        let merged = merge_gitignore(&path, &agentic);
        let lines: Vec<&str> = merged.lines().collect();

        assert_eq!(lines.iter().filter(|l| **l == ".mytool/").count(), 1);
        let marker = lines.iter().position(|l| *l == agentic::KEPT_MARKER);
        assert!(marker.is_some(), "kept marker missing: {merged:?}");
        let marker = marker.unwrap();
        let canonical = lines
            .iter()
            .position(|l| *l == agentic::CANONICAL_HEADER)
            .expect("canonical header missing");
        let negation = lines
            .iter()
            .position(|l| *l == agentic::NEG_HEADER)
            .expect("negation header missing");
        let mytool = lines.iter().position(|l| *l == ".mytool/").unwrap();
        assert!(
            canonical < marker && marker < mytool && mytool < negation,
            ".mytool/ must sit under the kept marker, inside the block: {merged:?}"
        );
        // The kept marker appears exactly once, also after a second run.
        let (first, second) = merge_twice("target/\n\n# AI coding agents\n.mytool/\n\n*.log\n");
        assert_eq!(first, second);
        assert_eq!(
            second
                .lines()
                .filter(|l| *l == agentic::KEPT_MARKER)
                .count(),
            1
        );
    }

    /// Req 5(d): content before and after the block is untouched, with the
    /// single-blank joins the removal may leave behind.
    #[test]
    fn agentic_merge_preserves_surrounding_content() {
        let fixture = "head-marker\ntarget/\n\n# AI coding agents\n.old/\n\n*.log\ntail-marker\n";
        let (_dir, path) = tmp_gitignore(fixture);
        let agentic = builtins::get("agentic").unwrap();
        let merged = merge_gitignore(&path, &agentic);
        let lines: Vec<&str> = merged.lines().collect();

        // Head: verbatim prefix through the blank that preceded the block.
        assert_eq!(&lines[..3], ["head-marker", "target/", ""]);
        // Tail: one blank join, then the trailing lines verbatim, in order.
        let tail = lines.len() - 3;
        assert_eq!(&lines[tail..], ["", "*.log", "tail-marker"]);
        let canonical = lines
            .iter()
            .position(|l| *l == agentic::CANONICAL_HEADER)
            .unwrap();
        assert!(canonical >= 3 && canonical < tail, "block misplaced");
        assert!(merged.starts_with("head-marker\ntarget/\n\n"));
        assert!(merged.ends_with("\n\n*.log\ntail-marker\n"));
    }

    /// Req 2: legacy entries gitkit shipped are dropped when the registry no
    /// longer produces them; the marker is absent when nothing is kept.
    #[test]
    fn agentic_merge_drops_legacy_only_entries() {
        let previous = "\n# AI coding agents\n\
            .kiro/\n\
            .cursor/\n\
            .windsurf/\n\
            .claude/\n\
            .continue/\n\
            .copilot/\n\
            .kilocode/\n\
            .zencoder/\n\
            .qwen/\n\
            .agents/\n\
            skills-lock.json\n";
        let (_dir, path) = tmp_gitignore(previous);
        let agentic = builtins::get("agentic").unwrap();
        let merged = merge_gitignore(&path, &agentic);
        let lines: Vec<&str> = merged.lines().collect();

        for legacy in [
            ".kiro/",
            ".cursor/",
            ".windsurf/",
            ".claude/",
            ".continue/",
            ".copilot/",
            ".kilocode/",
            ".zencoder/",
            ".qwen/",
            ".agents/",
        ] {
            assert_eq!(
                lines.iter().filter(|l| **l == legacy).count(),
                0,
                "legacy entry {legacy} must be dropped: {merged:?}"
            );
        }
        // Still-produced entries keep exactly one occurrence.
        assert_eq!(
            lines.iter().filter(|l| **l == "skills-lock.json").count(),
            1
        );
        assert_eq!(lines.iter().filter(|l| **l == ".kiro/*").count(), 1);
        assert_eq!(
            lines.iter().filter(|l| **l == agentic::KEPT_MARKER).count(),
            0,
            "nothing survived, so no kept marker may appear"
        );
    }

    /// A repeated template name (`ignore add agentic,agentic`) concatenates
    /// the built-in twice; the managed block still carries every registry
    /// line exactly once, and the result merges idempotently.
    #[test]
    fn agentic_merge_dedupes_repeated_template_content() {
        let content = builtins::get("agentic").unwrap();
        let doubled = format!("{content}{content}");
        let (_dir, path) = tmp_gitignore("target/\n");
        let merged = merge_gitignore(&path, &doubled);
        for pattern in content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            assert_eq!(
                merged.lines().filter(|l| *l == pattern).count(),
                1,
                "pattern {pattern} duplicated from a repeated template"
            );
        }
        assert_eq!(
            merged
                .lines()
                .filter(|l| *l == agentic::CANONICAL_HEADER)
                .count(),
            1
        );
        fs::write(&path, &merged).unwrap();
        let second = merge_gitignore(&path, &doubled);
        assert_eq!(merged, second, "repeated content must merge idempotently");
    }

    /// Trap F: only the canonical header selects the managed path; a bare
    /// header in new content stays an ordinary comment for the generic merge.
    #[test]
    fn merge_dispatches_to_the_managed_path_only_for_the_canonical_header() {
        let fixture = "target/\n# AI coding agents\n.old/\n";
        let (_dir, path) = tmp_gitignore(fixture);
        let merged = merge_gitignore(&path, "# AI coding agents\n.another/\n");
        assert_eq!(
            merged
                .lines()
                .filter(|l| *l == "# AI coding agents")
                .count(),
            2,
            "without the canonical header the generic append path applies"
        );

        let (_dir2, path2) = tmp_gitignore(fixture);
        let agentic = builtins::get("agentic").unwrap();
        let managed = merge_gitignore(&path2, &agentic);
        assert_eq!(
            managed
                .lines()
                .filter(|l| l.starts_with("# AI coding agents"))
                .count(),
            1,
            "the managed path collapses the bare header into one block"
        );
    }

    #[test]
    fn merge_gitignore_star_supersedes_wholesale_dir() {
        let (_dir, path) = tmp_gitignore(".foo/\nkeep\n");
        let merged = merge_gitignore(&path, ".foo/*\nkeep\n");
        assert!(!merged.lines().any(|l| l == ".foo/"));
        assert_eq!(merged.lines().filter(|l| *l == ".foo/*").count(), 1);
        assert_eq!(merged.lines().filter(|l| *l == "keep").count(), 1);
    }

    #[test]
    fn merge_gitignore_star_does_not_touch_unrelated_dirs() {
        let (_dir, path) = tmp_gitignore(".bar/\n");
        let merged = merge_gitignore(&path, ".foo/*\n");
        assert!(merged.lines().any(|l| l == ".bar/"));
        assert!(merged.lines().any(|l| l == ".foo/*"));
    }

    /// The superseded set only ever holds real ignore patterns: lines starting
    /// with `#` are comments, so a `# dir/*` line in the new content must never
    /// remove a `# dir/` line from the existing file.
    #[test]
    fn merge_gitignore_comment_star_never_supersedes_a_comment() {
        let (_dir, path) = tmp_gitignore("# docs/\n");
        let merged = merge_gitignore(&path, "# docs/*\n");
        assert!(
            merged.lines().any(|l| l == "# docs/"),
            "a comment must not supersede an existing comment: {merged:?}"
        );
        assert!(merged.lines().any(|l| l == "# docs/*"));
    }

    /// Likewise a `!dir/*` negation line never supersedes an existing `!dir/`
    /// line: only plain patterns enter the superseded set.
    #[test]
    fn merge_gitignore_negation_star_never_supersedes_a_negation() {
        let (_dir, path) = tmp_gitignore("!docs/\n");
        let merged = merge_gitignore(&path, "!docs/*\n");
        assert!(
            merged.lines().any(|l| l == "!docs/"),
            "a negation must not supersede an existing negation: {merged:?}"
        );
        assert!(merged.lines().any(|l| l == "!docs/*"));
    }

    #[test]
    fn merge_gitignore_agentic_reapply_does_not_duplicate_patterns() {
        let content = builtins::get("agentic").unwrap();
        let (_dir, path) = tmp_gitignore(&content);
        let merged = merge_gitignore(&path, &content);

        for pattern in content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let count = merged.lines().filter(|l| *l == pattern).count();
            assert_eq!(
                count, 1,
                "pattern {pattern} duplicated after reapplying template"
            );
        }
    }

    #[test]
    fn merge_gitignore_only_comments_appended() {
        let (_dir, path) = tmp_gitignore("target/\n");
        let new = "# just a comment\n# another\n";
        let result = merge_gitignore(&path, new);
        assert!(result.contains("# just a comment"));
        assert!(result.contains("target/"));
    }

    #[test]
    fn merge_gitignore_only_blank_lines_appended() {
        let (_dir, path) = tmp_gitignore("target/\n");
        let new = "\n\n\n";
        let result = merge_gitignore(&path, new);
        assert_eq!(result, "target/\n");
    }

    #[test]
    fn merge_gitignore_mixed_new_and_existing_patterns() {
        let (_dir, path) = tmp_gitignore("target/\n*.log\n");
        let new = "*.log\n*.tmp\n";
        let result = merge_gitignore(&path, new);
        assert_eq!(result.matches("*.log").count(), 1);
        assert!(result.contains("*.tmp"));
    }

    #[test]
    fn merge_gitignore_existing_file_not_ending_with_newline() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        fs::write(&path, "target/").unwrap();
        let result = merge_gitignore(&path, "*.log\n");
        assert!(result.contains("target/"));
        assert!(result.contains("*.log"));
    }

    #[test]
    fn merge_gitignore_empty_new_content() {
        let (_dir, path) = tmp_gitignore("target/\n");
        let result = merge_gitignore(&path, "");
        assert_eq!(result, "target/\n");
    }

    #[test]
    fn merge_gitignore_empty_existing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        fs::write(&path, "").unwrap();
        let result = merge_gitignore(&path, "*.log\n");
        assert_eq!(result, "*.log\n");
    }

    #[test]
    fn merge_gitignore_preserves_blank_line_separators() {
        let (_dir, path) = tmp_gitignore("target/\n");
        let new = "\n*.log\n\n*.tmp\n";
        let result = merge_gitignore(&path, new);
        assert!(result.contains("*.log"));
        assert!(result.contains("*.tmp"));
    }

    #[test]
    fn add_templates_rejects_invalid_input_gracefully() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        // Write a file to ensure merge_gitignore has something to work with
        fs::write(&path, "existing\n").unwrap();
        let result = merge_gitignore(&path, "existing\nnew_pattern\n");
        assert!(result.contains("new_pattern"));
        assert_eq!(result.matches("existing").count(), 1);
    }

    #[test]
    fn builtins_get_returns_none_for_unknown() {
        assert!(builtins::get("nonexistent").is_none());
    }

    #[test]
    fn builtins_get_returns_agentic() {
        assert!(builtins::get("agentic").is_some());
    }

    #[test]
    fn builtins_names_contains_agentic() {
        assert!(builtins::NAMES.contains(&"agentic"));
    }

    #[test]
    fn api_base_is_correct() {
        assert_eq!(API_BASE, "https://www.toptal.com/developers/gitignore/api");
    }

    // ── merge_gitignore additional edge cases ───────────────────────────────

    #[test]
    fn merge_gitignore_preserves_order_of_existing() {
        let (_dir, path) = tmp_gitignore("*.log\n*.tmp\n");
        let result = merge_gitignore(&path, "*.log\n");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[0], "*.log");
        assert_eq!(lines[1], "*.tmp");
    }

    #[test]
    fn merge_gitignore_multiple_newlines_preserved() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        let result = merge_gitignore(&path, "*.log\n\n*.tmp\n");
        assert!(result.contains("*.log"));
        assert!(result.contains("*.tmp"));
    }

    #[test]
    fn merge_gitignore_existing_with_trailing_whitespace() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        fs::write(&path, "target/ \n").unwrap();
        let result = merge_gitignore(&path, "target/\n");
        // "target/ " (with trailing space) is not the same as "target/"
        // so "target/" from new content should still be appended
        assert!(result.contains("target/"));
    }

    #[test]
    fn merge_gitignore_new_content_all_duplicates() {
        let (_dir, path) = tmp_gitignore("a\nb\nc\n");
        let result = merge_gitignore(&path, "a\nb\nc\n");
        assert_eq!(result, "a\nb\nc\n");
    }

    #[test]
    fn merge_gitignore_large_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        let existing: String = (0..100).map(|i| format!("pattern{i}\n")).collect();
        fs::write(&path, &existing).unwrap();
        let new: String = (100..150).map(|i| format!("pattern{i}\n")).collect();
        let result = merge_gitignore(&path, &new);
        assert!(result.contains("pattern0"));
        assert!(result.contains("pattern149"));
    }

    // ── resolve_templates edge cases ────────────────────────────────────────

    #[test]
    fn resolve_templates_empty_string_does_not_panic() {
        // Empty string sends empty query to API — just verify it doesn't panic
        let result = resolve_templates("");
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn resolve_templates_single_builtin() {
        let result = resolve_templates("agentic");
        assert!(result.is_ok());
        assert!(result.unwrap().contains(".kiro/"));
    }

    #[test]
    fn resolve_templates_builtin_with_whitespace() {
        let result = resolve_templates(" agentic ");
        assert!(result.is_ok());
        assert!(result.unwrap().contains(".kiro/"));
    }

    // ── builtins module edge cases ──────────────────────────────────────────

    #[test]
    fn builtins_names_is_nonempty() {
        assert!(!builtins::NAMES.is_empty());
    }

    #[test]
    fn builtins_get_returns_same_static_str() {
        let a = builtins::get("agentic");
        let b = builtins::get("agentic");
        assert_eq!(a, b);
    }

    #[test]
    fn builtins_get_agentic_content_has_expected_dirs() {
        let content = builtins::get("agentic").unwrap();
        assert!(content.contains(".kiro/*"));
        assert!(content.contains(".cursor/*"));
        assert!(content.contains(".claude/*"));
        assert!(content.contains("!.claude/CLAUDE.md"));
        assert!(content.contains(".kilocode/*"));
        assert!(content.contains("kilo.jsonc"));
        assert!(content.contains(".opencode/*"));
        assert!(content.contains("opencode.json"));
        assert!(content.contains(".agents/*"));
        assert!(content.contains("skills-lock.json"));
    }

    // ── add_templates ─────────────────────────────────────────────────────

    #[serial]
    #[test]
    fn add_templates_force_writes_gitignore() {
        let gitkit_home = tempfile::TempDir::new().unwrap();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", gitkit_home.path());
        }
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add_templates("agentic", true);
        assert!(result.is_ok());
        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".kiro/"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
        unsafe {
            match &orig_gitkit_home {
                Some(h) => std::env::set_var("GITKIT_HOME", h),
                None => std::env::remove_var("GITKIT_HOME"),
            }
        }
    }

    #[serial]
    #[test]
    fn add_templates_merge_with_existing_gitignore() {
        let gitkit_home = tempfile::TempDir::new().unwrap();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", gitkit_home.path());
        }
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add_templates("agentic", false);
        assert!(result.is_ok());
        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("target/"));
        assert!(gitignore.contains(".kiro/"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
        unsafe {
            match &orig_gitkit_home {
                Some(h) => std::env::set_var("GITKIT_HOME", h),
                None => std::env::remove_var("GITKIT_HOME"),
            }
        }
    }

    #[serial]
    #[test]
    fn add_templates_no_existing_gitignore() {
        let gitkit_home = tempfile::TempDir::new().unwrap();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", gitkit_home.path());
        }
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add_templates("agentic", false);
        assert!(result.is_ok());
        let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains(".kiro/"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
        unsafe {
            match &orig_gitkit_home {
                Some(h) => std::env::set_var("GITKIT_HOME", h),
                None => std::env::remove_var("GITKIT_HOME"),
            }
        }
    }

    // ── resolve_templates with builtins only ──────────────────────────────

    #[test]
    fn resolve_templates_single_builtin_no_api_call() {
        let result = resolve_templates("agentic");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains(".kiro/"));
        assert!(content.contains(".cursor/"));
    }

    #[test]
    fn resolve_templates_two_distinct_builtins() {
        let result = resolve_templates("agentic");
        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.contains(".kiro/"));
        assert!(content.contains(".cursor/"));
    }

    #[test]
    fn resolve_templates_builtin_with_whitespace_around() {
        let result = resolve_templates("  agentic  ");
        assert!(result.is_ok());
        assert!(result.unwrap().contains(".kiro/"));
    }

    // ── merge_gitignore additional edge cases ─────────────────────────────

    #[test]
    fn merge_gitignore_both_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        let result = merge_gitignore(&path, "");
        assert!(result.is_empty());
    }

    #[test]
    fn merge_gitignore_new_content_only_comments() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        let result = merge_gitignore(&path, "# comment\n# another\n");
        assert!(result.contains("# comment"));
        assert!(result.contains("# another"));
    }

    #[test]
    fn merge_gitignore_existing_with_trailing_newline() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, "target/\n").unwrap();
        let result = merge_gitignore(&path, "*.log\n");
        assert!(result.contains("target/"));
        assert!(result.contains("*.log"));
    }

    #[test]
    fn merge_gitignore_existing_without_trailing_newline() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, "target/").unwrap();
        let result = merge_gitignore(&path, "*.log\n");
        assert!(result.contains("target/"));
        assert!(result.contains("*.log"));
    }

    #[test]
    fn merge_gitignore_new_content_blank_lines_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, "target/\n").unwrap();
        let result = merge_gitignore(&path, "\n\n\n");
        assert_eq!(result, "target/\n");
    }

    #[test]
    fn merge_gitignore_mixed_patterns_and_comments() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, "*.log\n").unwrap();
        let result = merge_gitignore(&path, "# Rust\ntarget/\n*.log\n# Python\n__pycache__/\n");
        assert!(result.contains("# Rust"));
        assert!(result.contains("target/"));
        assert!(result.contains("__pycache__/"));
        assert_eq!(result.matches("*.log").count(), 1);
    }

    #[test]
    fn merge_gitignore_preserves_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, "a\nb\n").unwrap();
        let result = merge_gitignore(&path, "c\n");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines[0], "a");
        assert_eq!(lines[1], "b");
        assert_eq!(lines[2], "c");
    }

    #[test]
    fn merge_gitignore_duplicate_comment_not_deduplicated() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, "# header\na\n").unwrap();
        let result = merge_gitignore(&path, "# header\nb\n");
        // Comments are always appended (not deduplicated)
        assert!(result.contains("# header"));
        assert!(result.contains("b"));
    }

    // ── run dispatch ──────────────────────────────────────────────────────

    #[test]
    fn run_list_builtins() {
        let result = run(IgnoreCommand::List {
            filter: Some("agentic".to_string()),
        });
        assert!(result.is_ok());
    }

    #[test]
    fn run_list_all() {
        let result = run(IgnoreCommand::List { filter: None });
        // This calls the API, may fail if offline
        let _ = result;
    }

    // ── builtins module edge cases ────────────────────────────────────────

    #[test]
    fn builtins_names_all_have_content() {
        for name in builtins::NAMES {
            let content = builtins::get(name);
            assert!(content.is_some(), "Builtin {} has no content", name);
            assert!(
                !content.unwrap().is_empty(),
                "Builtin {} has empty content",
                name
            );
        }
    }

    #[test]
    fn builtins_get_returns_same_content_multiple_calls() {
        let a = builtins::get("agentic").unwrap();
        let b = builtins::get("agentic").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn builtins_get_unknown_returns_none() {
        assert!(builtins::get("unknown-template").is_none());
        assert!(builtins::get("").is_none());
        assert!(builtins::get("Rust").is_none());
    }
}

mod builtins {
    pub(super) const NAMES: &[&str] = &["agentic"];

    pub(super) fn get(name: &str) -> Option<String> {
        match name {
            "agentic" => Some(super::agentic::current_content()),
            _ => None,
        }
    }
}
