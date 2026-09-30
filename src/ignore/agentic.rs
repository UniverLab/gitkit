use std::collections::BTreeSet;

#[cfg(test)]
use super::agent_registry::snapshot_platforms;
use super::agent_registry::Platform;
#[cfg(not(test))]
use super::agent_registry::Resolution;

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
    let mut content =
        String::from("\n# AI coding agents (paths derived from the canopy registry)\n");
    let mut in_negations = false;
    for line in lines {
        if !in_negations && line.starts_with('!') {
            content.push_str("\n# instruction files stay committable\n");
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

fn is_forbidden(path: &str) -> bool {
    path == ".github"
        || path == ".config"
        || path == ".github/"
        || path == ".config/"
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
        while current != ignored {
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
        for entry in [".github", ".config", ".config/x/", ".github/y"] {
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
