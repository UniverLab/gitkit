use std::collections::{BTreeSet, HashSet};

#[cfg(test)]
use super::agent_registry::snapshot_platforms;
use super::agent_registry::Platform;
#[cfg(not(test))]
use super::agent_registry::Resolution;

/// The header `assemble()` emits: the only one `gitkit ignore add agentic`
/// writes. Its presence in new content is what selects the managed merge.
pub(crate) const CANONICAL_HEADER: &str =
    "# AI coding agents (paths derived from the canopy registry)";
/// Marks the negation tail of the block; nothing but `!` lines may follow it.
pub(crate) const NEG_HEADER: &str = "# instruction files stay committable";
/// Marks entry lines carried over from a previous block that the registry no
/// longer produces but gitkit never shipped itself (user-added lines).
pub(crate) const KEPT_MARKER: &str = "# kept from a previous agentic block";

/// Every header that opens a managed agentic block. Older gitkit versions
/// appended one header per run (bare, canonical, negation), so a file may
/// hold several of them; all are stripped and rebuilt as a single block.
const MANAGED_HEADERS: &[&str] = &["# AI coding agents", CANONICAL_HEADER, NEG_HEADER];

/// Entries gitkit itself shipped in older `agentic` blocks. When the registry
/// no longer produces them they are dropped instead of kept under
/// [`KEPT_MARKER`]: the `dir/` forms would block the `!` negations and the
/// others were retired with their platforms. Exact list of the `previous`
/// fixture in `merge_gitignore_agentic_merges_cleanly_with_previous_version`.
const LEGACY_ENTRIES: &[&str] = &[
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
    "skills-lock.json",
];

/// True for the managed headers of the agentic block, matched on the trimmed
/// line so trailing spaces or a stray `\r` still count.
pub(crate) fn is_managed_header(line: &str) -> bool {
    MANAGED_HEADERS.contains(&line.trim())
}

/// An existing `.gitignore` split into the lines outside every managed block
/// (verbatim, blank runs collapsed), the entry lines collected from inside
/// the blocks, and the output index where the first managed header was found
/// (`None` means the block is appended at the end).
pub(crate) struct Stripped {
    pub(crate) outside: Vec<String>,
    pub(crate) collected: Vec<String>,
    pub(crate) first_idx: Option<usize>,
}

/// Removes every managed header together with the entry lines that follow it
/// up to the next blank line or the next `#` header, collecting those
/// entries. The block's [`KEPT_MARKER`] is consumed in place so a re-run
/// folds the previous kept section into the collected entries instead of
/// leaving it behind as an outside comment.
pub(crate) fn strip_managed(existing: &str) -> Stripped {
    let mut outside: Vec<String> = Vec::new();
    let mut collected: Vec<String> = Vec::new();
    let mut first_idx: Option<usize> = None;
    let mut collecting = false;
    for line in existing.lines() {
        if collecting {
            if line.trim().is_empty() {
                outside.push(line.to_string());
                collecting = false;
            } else if is_managed_header(line) {
                // Chained header (the old append-per-run growth): the block
                // continues, the header itself is rebuilt later.
            } else if line.trim() == KEPT_MARKER {
                // Rebuilt as part of the block; collect the entries below it.
            } else if line.starts_with('#') {
                outside.push(line.to_string());
                collecting = false;
            } else {
                collected.push(line.to_string());
            }
        } else if is_managed_header(line) {
            if first_idx.is_none() {
                first_idx = Some(outside.len());
            }
            collecting = true;
        } else {
            outside.push(line.to_string());
        }
    }
    let anchor = first_idx.unwrap_or(outside.len());
    let (outside, removed) = collapse_blank_runs(outside, anchor);
    Stripped {
        outside,
        collected,
        first_idx: first_idx.map(|index| index - removed),
    }
}

/// Folds runs of more than one empty line left behind by the removal into a
/// single blank. `anchor` is the raw index of the managed-block insertion
/// point; the returned count is how many blanks were dropped before it, so
/// the caller can shift the index. Blank lines are never reordered and
/// non-blank lines are never touched.
fn collapse_blank_runs(raw: Vec<String>, anchor: usize) -> (Vec<String>, usize) {
    let mut collapsed: Vec<String> = Vec::with_capacity(raw.len());
    let mut removed_before_anchor = 0usize;
    let mut previous_blank = false;
    for (index, line) in raw.into_iter().enumerate() {
        if line.is_empty() && previous_blank {
            if index < anchor {
                removed_before_anchor += 1;
            }
            continue;
        }
        previous_blank = line.is_empty();
        collapsed.push(line);
    }
    (collapsed, removed_before_anchor)
}

/// The registry-derived part of an `agentic` template, split the way the
/// managed block is written: ignore lines, negation lines, and a set for
/// membership tests. Nothing is recomputed here, so the registry's path and
/// negation rules stay untouched.
pub(crate) struct Fresh {
    pub(crate) ignores: Vec<String>,
    pub(crate) negations: Vec<String>,
    seen: HashSet<String>,
}

impl Fresh {
    /// True when the registry still produces this entry line.
    pub(crate) fn contains(&self, line: &str) -> bool {
        self.seen.contains(line.trim())
    }
}

pub(crate) fn split_fresh(new_content: &str) -> Fresh {
    let mut fresh = Fresh {
        ignores: Vec::new(),
        negations: Vec::new(),
        seen: HashSet::new(),
    };
    for line in new_content.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // One entry per line: a repeated template (`ignore add agentic,agentic`)
        // concatenates the built-in twice, and the managed block must still
        // hold each registry line exactly once.
        if !fresh.seen.insert(line.trim().to_string()) {
            continue;
        }
        if line.starts_with('!') {
            fresh.negations.push(line.to_string());
        } else {
            fresh.ignores.push(line.to_string());
        }
    }
    fresh
}

/// Collected entries that are neither produced by the registry anymore nor
/// gitkit's own legacy list: user lines carried over under [`KEPT_MARKER`],
/// in first-seen order and deduplicated.
pub(crate) fn compute_kept(collected: &[String], fresh: &Fresh) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for line in collected {
        let trimmed = line.trim();
        if fresh.contains(trimmed) || LEGACY_ENTRIES.contains(&trimmed) {
            continue;
        }
        if kept.iter().any(|previous| previous == line) {
            continue;
        }
        kept.push(line.clone());
    }
    kept
}

/// The single managed block: canonical header, registry lines, kept lines
/// under their marker, then the negations under [`NEG_HEADER`]. Never emits
/// the bare `# AI coding agents` header.
pub(crate) fn assemble_managed(
    ignores: &[String],
    kept: &[String],
    negations: &[String],
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(CANONICAL_HEADER.to_string());
    lines.extend(ignores.iter().cloned());
    if !kept.is_empty() {
        lines.push(KEPT_MARKER.to_string());
        lines.extend(kept.iter().cloned());
    }
    if !negations.is_empty() {
        lines.push(String::new());
        lines.push(NEG_HEADER.to_string());
        lines.extend(negations.iter().cloned());
    }
    lines
}

pub(crate) fn build(platforms: &[Platform]) -> (Vec<String>, Vec<String>) {
    let protected = protected_set(platforms);
    let (mut ignore, warnings) = ignore_lines(platforms, &protected);
    ignore.insert("skills-lock.json".to_string());
    let dirs: BTreeSet<String> = ignore
        .iter()
        .filter(|line| line.ends_with("/*"))
        .map(|line| line.trim_end_matches('*').to_string())
        .collect();
    let mut lines: Vec<String> = ignore.into_iter().collect();
    let mut negs = negations(&protected, &dirs);
    lines.append(&mut negs);
    (lines, warnings)
}

#[cfg(test)]
pub(crate) fn render(platforms: &[Platform]) -> String {
    let (lines, _) = build(platforms);
    assemble(&lines)
}

pub(crate) fn current_content() -> String {
    #[cfg(test)]
    {
        let platforms = snapshot_platforms();
        let (lines, warnings) = build(&platforms);
        emit_warnings(&warnings, None);
        assemble(&lines)
    }
    #[cfg(not(test))]
    {
        use super::agent_registry::{cache_path, resolve, HttpFetcher};
        let cache = cache_path();
        let Resolution { platforms, notice } = resolve(&HttpFetcher, cache.as_deref());
        let (lines, warnings) = build(&platforms);
        emit_warnings(&warnings, notice.as_deref());
        assemble(&lines)
    }
}

fn emit_warnings(warnings: &[String], notice: Option<&str>) {
    for warning in warnings {
        eprintln!("{warning}");
    }
    if let Some(notice) = notice {
        eprintln!("{notice}");
    }
}

fn assemble(lines: &[String]) -> String {
    let mut content = format!("\n{CANONICAL_HEADER}\n");
    let mut in_negations = false;
    for line in lines {
        if !in_negations && line.starts_with('!') {
            content.push_str(&format!("\n{NEG_HEADER}\n"));
            in_negations = true;
        }
        content.push_str(line);
        content.push('\n');
    }
    content
}

fn protected_set(platforms: &[Platform]) -> BTreeSet<String> {
    let project_paths: BTreeSet<&str> = platforms
        .iter()
        .flat_map(|p| p.project_paths.iter().map(String::as_str))
        .collect();
    let mut protected = BTreeSet::new();
    protected.insert("AGENTS.md".to_string());
    protected.insert("CLAUDE.md".to_string());
    for platform in platforms {
        if let Some(file) = &platform.instruction_file {
            protected.insert(file.clone());
        }
        for entry in &platform.instruction_precedence {
            if !project_paths.contains(entry.as_str()) {
                protected.insert(entry.clone());
            }
        }
    }
    protected
}

/// Never-ignore roots: gitkit must not hide `.github`/`.config` (its own
/// configuration directories), neither the bare root nor anything under it.
/// The `"<root>/"` equality cases are covered by the prefix checks, so only
/// the bare roots are compared literally.
fn is_forbidden(path: &str) -> bool {
    path == ".github"
        || path == ".config"
        || path.starts_with(".github/")
        || path.starts_with(".config/")
}

fn ignore_lines(
    platforms: &[Platform],
    protected: &BTreeSet<String>,
) -> (BTreeSet<String>, Vec<String>) {
    let mut lines = BTreeSet::new();
    let mut warnings = Vec::new();
    for platform in platforms {
        for entry in &platform.project_paths {
            if is_forbidden(entry) {
                warnings.push(format!(
                    "agentic: {}: '{entry}' skipped — .github/.config are never ignored",
                    platform.name
                ));
                continue;
            }
            if entry.ends_with('/') {
                lines.insert(format!("{entry}*"));
            } else if protected.contains(entry) {
                warnings.push(format!(
                    "agentic: {}: '{entry}' is a protected instruction file and stays committable",
                    platform.name
                ));
            } else {
                lines.insert(entry.clone());
            }
        }
    }
    (lines, warnings)
}

fn negations(protected: &BTreeSet<String>, dirs: &BTreeSet<String>) -> Vec<String> {
    let mut out = BTreeSet::new();
    for path in protected {
        if !path.contains('/') {
            continue;
        }
        let parent = ancestor_dir(path);
        let ignored = match dirs.iter().find(|d| path.starts_with(d.as_str())) {
            Some(dir) => dir.clone(),
            None => continue,
        };
        let mut current = parent;
        let mut chain = vec![path.clone()];
        // Bounded walk: every real step shortens `current`, so the chain of
        // ancestors runs out long before `path.len()` steps and always passes
        // `ignored` first. The cap keeps a non-productive `ancestor_dir` from
        // spinning here forever; it is never reached by the real chain.
        for _ in 0..=path.len() {
            if current == ignored {
                break;
            }
            chain.push(current.clone());
            current = ancestor_dir(&current);
        }
        for item in chain {
            out.insert(format_negation(&item));
        }
    }
    out.into_iter().collect()
}

fn ancestor_dir(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(index) => trimmed[..=index].to_string(),
        None => String::new(),
    }
}

fn format_negation(path: &str) -> String {
    if path.ends_with('/') {
        return format!("!{path}");
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    if last.contains('.') {
        format!("!{path}")
    } else {
        format!("!{path}/")
    }
}

#[cfg(test)]
mod tests {
    use super::super::agent_registry::parse_merged;
    use super::*;

    const FOO: &str = "[[platforms]]\nname = \"foo\"\nproject_paths = [\".foo/\"]\n";

    fn platforms_of(text: &str) -> Vec<Platform> {
        parse_merged(text).unwrap()
    }

    fn snapshot() -> Vec<Platform> {
        snapshot_platforms()
    }

    #[test]
    fn build_fixture_new_platform_foo() {
        let (lines, _) = build(&platforms_of(FOO));
        assert!(lines.contains(&".foo/*".to_string()));
    }

    #[test]
    fn build_fixture_github_path_warns_only() {
        let text = "[[platforms]]\nname = \"evil\"\nproject_paths = [\".github/prompts/\"]\n";
        let (lines, warnings) = build(&platforms_of(text));
        assert!(!lines.iter().any(|l| {
            let body = l.trim_start_matches('!');
            body == ".github" || body.starts_with(".github/") || body.starts_with(".github*")
        }));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("evil"));
        for entry in [
            ".github",
            ".config",
            ".github/",
            ".config/",
            ".config/x/",
            ".github/y",
        ] {
            let fixture = format!("[[platforms]]\nname = \"p\"\nproject_paths = [\"{entry}\"]\n");
            let (lines, warnings) = build(&platforms_of(&fixture));
            assert!(!lines.iter().any(|l| {
                let body = l.trim_start_matches('!');
                body.starts_with(".github") || body.starts_with(".config")
            }));
            assert_eq!(warnings.len(), 1);
        }
    }

    #[test]
    fn build_fixture_protected_root_file_dropped_with_warning() {
        let text = "[[platforms]]\nname = \"p\"\nproject_paths = [\"AGENTS.md\"]\ninstruction_file = \"AGENTS.md\"\n";
        let (lines, warnings) = build(&platforms_of(text));
        assert!(!lines.contains(&"AGENTS.md".to_string()));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains('p'));
    }

    #[test]
    fn protected_set_rules() {
        let text = "[[platforms]]\nname = \"claude\"\nproject_paths = [\".claude/\", \"CLAUDE.local.md\"]\n\
            instruction_file = \"AGENTS.md\"\n\
            instruction_precedence = [\"CLAUDE.md\", \".claude/CLAUDE.md\", \"CLAUDE.local.md\"]\n";
        let platforms = platforms_of(text);
        let protected = protected_set(&platforms);
        assert!(protected.contains("AGENTS.md"));
        assert!(protected.contains("CLAUDE.md"));
        assert!(protected.contains(".claude/CLAUDE.md"));
        assert!(!protected.contains("CLAUDE.local.md"));
    }

    #[test]
    fn render_snapshot_expected_lines() {
        let content = render(&snapshot());
        for expected in [
            ".kilo/*",
            "kilo.jsonc",
            "kilo.json",
            ".kilocode/*",
            ".opencode/*",
            "opencode.json",
            "opencode.jsonc",
            ".agents/*",
            ".claude/*",
            "skills-lock.json",
            ".mcp.json",
            ".clineignore",
            "!.claude/CLAUDE.md",
            "!.continue/rules/",
            "!.cursor/rules/",
        ] {
            assert!(content.lines().any(|l| l == expected), "missing {expected}");
        }
        assert_eq!(
            content.lines().filter(|l| *l == ".agents/*").count(),
            1,
            ".agents/* must be deduplicated"
        );
        assert!(!content.lines().any(|l| l == ".aider.chat.history.md"));
        assert!(!content.lines().any(|l| l == ".claude/settings.local.json"));
        assert!(!content.lines().any(|l| l.trim() == ".claude/"));
        assert!(!content.lines().any(|l| l.trim() == ".windsurf/"));
        assert!(!content.lines().any(|l| l.trim() == ".zencoder/"));
        for line in content.lines() {
            if line.starts_with('!') || line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert_ne!(line, "AGENTS.md");
            assert_ne!(line, "CLAUDE.md");
        }
    }

    #[test]
    fn render_negation_block_last() {
        let content = render(&snapshot());
        let mut seen_negation = false;
        for line in content.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('!') {
                seen_negation = true;
            } else if seen_negation {
                panic!("ignore line after negation block: {line}");
            }
        }
    }

    #[test]
    fn render_never_emits_github_or_config() {
        let (lines, _) = build(&snapshot());
        for line in &lines {
            let body = line.trim_start_matches('!');
            assert!(!is_forbidden(body), "forbidden line emitted: {line}");
        }
    }

    #[test]
    fn render_contains_only_expected_negations() {
        let (lines, _) = build(&snapshot());
        let negs: Vec<&String> = lines.iter().filter(|l| l.starts_with('!')).collect();
        assert_eq!(negs.len(), 3);
        assert!(negs.iter().any(|l| l.as_str() == "!.claude/CLAUDE.md"));
        assert!(negs.iter().any(|l| l.as_str() == "!.continue/rules/"));
        assert!(negs.iter().any(|l| l.as_str() == "!.cursor/rules/"));
    }

    /// The comment that marks the negation block is emitted exactly once, on
    /// the line before the first `!` entry — not before the first ignore line
    /// and not dropped when the input already starts with a negation.
    #[test]
    fn assemble_comment_marks_the_negation_block_in_place() {
        let content = assemble(&[
            ".foo/*".to_string(),
            "bar.json".to_string(),
            "!keep/one.md".to_string(),
            "!keep/two.md".to_string(),
        ]);
        let marker = "# instruction files stay committable";
        let lines: Vec<&str> = content.lines().collect();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| **line == marker)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "the marker must appear exactly once: {content:?}"
        );
        let at = hits[0];
        assert!(
            lines.get(at + 1).is_some_and(|next| next.starts_with('!')),
            "the line after the marker must be the first negation: {content:?}"
        );
        assert!(
            lines[at + 1..].iter().all(|line| line.starts_with('!')),
            "nothing but negations may follow the marker: {content:?}"
        );
        assert!(
            lines[..at].contains(&".foo/*"),
            "ignore lines must precede the marker: {content:?}"
        );
    }

    /// Plan §5(6): the rebuilt block puts the kept marker after the registry
    /// lines, before a single blank + negation header, and never re-emits the
    /// bare `# AI coding agents` header.
    #[test]
    fn assemble_managed_marker_placement() {
        let lines = assemble_managed(
            &[".foo/*".to_string()],
            &[".mytool/".to_string()],
            &["!keep/a.md".to_string()],
        );
        let rendered: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(
            rendered,
            [
                "# AI coding agents (paths derived from the canopy registry)",
                ".foo/*",
                "# kept from a previous agentic block",
                ".mytool/",
                "",
                "# instruction files stay committable",
                "!keep/a.md",
            ]
        );
        assert_eq!(
            rendered
                .iter()
                .filter(|l| **l == "# AI coding agents")
                .count(),
            0,
            "the bare header must never be re-emitted"
        );
        // Without kept lines the marker disappears, the rest stays ordered.
        let without_kept =
            assemble_managed(&[".foo/*".to_string()], &[], &["!keep/a.md".to_string()]);
        let rendered: Vec<&str> = without_kept.iter().map(String::as_str).collect();
        assert_eq!(
            rendered,
            [
                "# AI coding agents (paths derived from the canopy registry)",
                ".foo/*",
                "",
                "# instruction files stay committable",
                "!keep/a.md",
            ]
        );
    }

    /// A template name repeated in the list concatenates the built-in twice;
    /// the registry lines enter `Fresh` once — compared on trimmed bytes, so
    /// padded duplicates fold too — and `contains` still answers for them.
    #[test]
    fn split_fresh_dedupes_repeated_lines() {
        let fresh = split_fresh(".foo/*\n!keep/a.md\n\n .foo/*\n!keep/a.md \n.bar\n");
        let ignores: Vec<&str> = fresh.ignores.iter().map(String::as_str).collect();
        assert_eq!(ignores, [".foo/*", ".bar"]);
        let negations: Vec<&str> = fresh.negations.iter().map(String::as_str).collect();
        assert_eq!(negations, ["!keep/a.md"]);
        assert!(fresh.contains(".foo/*"));
        assert!(fresh.contains(".foo/*  "));
        assert!(fresh.contains("!keep/a.md"));
    }

    /// An input that opens with a negation still gets the marker, once, and
    /// never an ignore line after it.
    #[test]
    fn assemble_comment_when_the_first_line_is_a_negation() {
        let content = assemble(&["!keep/one.md".to_string()]);
        let marker = "# instruction files stay committable";
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.iter().filter(|l| **l == marker).count(), 1);
        assert!(
            lines.contains(&"!keep/one.md"),
            "the negation itself must be kept: {content:?}"
        );
        assert!(
            lines
                .iter()
                .skip_while(|l| **l != marker)
                .skip(1)
                .all(|l| l.starts_with('!')),
            "nothing but negations may follow the marker: {content:?}"
        );
    }
}

#[cfg(test)]
mod check_ignore_tests {
    use super::super::agent_registry::{parse_merged, snapshot_platforms};
    use super::*;
    use std::collections::BTreeSet;
    use std::process::Command;

    fn is_ignored(repo: &std::path::Path, path: &str) -> bool {
        let output = Command::new("git")
            .args(["check-ignore", "--no-index", "--", path])
            .current_dir(repo)
            .output()
            .expect("git binary must be available");
        match output.status.code() {
            Some(0) => true,
            Some(1) => false,
            other => panic!(
                "git check-ignore failed ({other:?}): {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }

    fn create_probe(repo: &std::path::Path, path: &str) {
        let full = repo.join(path);
        if path.ends_with('/') {
            std::fs::create_dir_all(&full).unwrap();
            std::fs::write(full.join("probe.txt"), "x").unwrap();
        } else if let Some(parent) = full.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&full, "x").unwrap();
        }
    }

    #[test]
    fn generated_template_via_git_check_ignore_is_behaved_exactly() {
        let platforms = snapshot_platforms();
        let content = render(&platforms);
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()
            .unwrap()
            .success());
        std::fs::write(repo.join(".gitignore"), &content).unwrap();

        let parsed = parse_merged(super::super::agent_registry::SNAPSHOT).unwrap();
        let protected = protected_set(&parsed);
        let mut project_entries: BTreeSet<String> = BTreeSet::new();
        for platform in &parsed {
            for entry in &platform.project_paths {
                project_entries.insert(entry.clone());
            }
        }
        for entry in &project_entries {
            if entry == ".github" || entry.starts_with(".github/") {
                continue;
            }
            if protected.contains(entry) {
                continue;
            }
            if entry.ends_with('/') {
                create_probe(repo, entry);
                assert!(
                    is_ignored(repo, &format!("{entry}probe.txt")),
                    "{entry}probe.txt should be ignored"
                );
            } else {
                create_probe(repo, entry);
                assert!(is_ignored(repo, entry), "{entry} should be ignored");
            }
        }

        for path in &protected {
            let last = path.rsplit('/').next().unwrap_or(path);
            let dir_like = path.ends_with('/') || (path.contains('/') && !last.contains('.'));
            if dir_like {
                let dir = if path.ends_with('/') {
                    path.clone()
                } else {
                    format!("{path}/")
                };
                create_probe(repo, &dir);
                assert!(
                    !is_ignored(repo, &format!("{dir}probe.mdc")),
                    "{path} should stay committable"
                );
            } else {
                create_probe(repo, path);
                assert!(!is_ignored(repo, path), "{path} should stay committable");
            }
        }
        create_probe(repo, ".github/copilot-instructions.md");
        assert!(!is_ignored(repo, ".github/copilot-instructions.md"));

        for ignored in [
            "CLAUDE.local.md",
            "skills-lock.json",
            ".mcp.json",
            ".clineignore",
            "kilo.jsonc",
            "opencode.json",
        ] {
            create_probe(repo, ignored);
            assert!(is_ignored(repo, ignored), "{ignored} should be ignored");
        }
        for kept in [
            "AGENTS.md",
            "CLAUDE.md",
            "GEMINI.md",
            "knowledge.md",
            ".clinerules",
            "AGENTS.override.md",
        ] {
            create_probe(repo, kept);
            assert!(!is_ignored(repo, kept), "{kept} should stay committable");
        }
    }
}
