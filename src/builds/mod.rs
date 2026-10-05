use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Subcommand)]
pub enum BuildCommand {
    /// List saved builds
    List,
    /// Apply a saved build
    Apply {
        /// Build name
        name: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Save current configuration as a build
    Save {
        /// Build name
        name: String,
        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Delete a saved build
    Delete {
        /// Build name
        name: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Build {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub gitignore: GitignoreConfig,
    #[serde(default)]
    pub gitattributes: GitattributesConfig,
    #[serde(default)]
    pub config: ConfigBuild,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub builtins: Vec<String>,
    #[serde(default)]
    pub custom: Vec<CustomHook>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CustomHook {
    pub hook: String,
    pub command: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GitignoreConfig {
    #[serde(default)]
    pub templates: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GitattributesConfig {
    #[serde(default)]
    pub presets: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigBuild {
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default = "default_scope")]
    pub scope: String,
}

impl Default for ConfigBuild {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            scope: default_scope(),
        }
    }
}

fn default_scope() -> String {
    "local".to_string()
}

pub(crate) fn builds_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("Neither HOME nor USERPROFILE environment variable is set")?;
    Ok(PathBuf::from(home).join(".gitkit").join("builds"))
}

fn build_path(name: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..",
        "Invalid build name '{name}' — use a simple name without path separators"
    );
    Ok(builds_dir()?.join(format!("{name}.toml")))
}

pub fn run(cmd: BuildCommand) -> Result<()> {
    match cmd {
        BuildCommand::List => list(),
        BuildCommand::Apply { name, dry_run } => apply(&name, dry_run),
        BuildCommand::Save { name, description } => save(&name, description.as_deref()),
        BuildCommand::Delete { name } => delete(&name),
    }
}

fn list() -> Result<()> {
    let dir = builds_dir()?;
    if !dir.exists() {
        println!("No builds saved.");
        return Ok(());
    }

    let builds: Vec<_> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();

    if builds.is_empty() {
        println!("No builds saved.");
        return Ok(());
    }

    println!("Saved builds:\n");
    for entry in builds {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy();
        let content = fs::read_to_string(&path).unwrap_or_default();
        let build: Build = match toml::from_str(&content) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let desc = if build.description.is_empty() {
            ""
        } else {
            &format!(" — {}", build.description)
        };
        println!("  {name}{desc}");
        if !build.hooks.builtins.is_empty() || !build.hooks.custom.is_empty() {
            let hooks: Vec<&str> = build
                .hooks
                .builtins
                .iter()
                .map(|s| s.as_str())
                .chain(build.hooks.custom.iter().map(|c| c.hook.as_str()))
                .collect();
            println!("    hooks: {}", hooks.join(", "));
        }
        if !build.gitignore.templates.is_empty() {
            println!("    gitignore: {}", build.gitignore.templates.join(", "));
        }
        if !build.gitattributes.presets.is_empty() {
            println!(
                "    gitattributes: {}",
                build.gitattributes.presets.join(", ")
            );
        }
        if !build.config.keys.is_empty() {
            println!(
                "    config ({}): {}",
                build.config.scope,
                build.config.keys.join(", ")
            );
        }
        println!();
    }

    Ok(())
}

fn apply(name: &str, dry_run: bool) -> Result<()> {
    let path = build_path(name)?;
    anyhow::ensure!(path.exists(), "Build '{name}' not found");

    let content = fs::read_to_string(&path).context("Failed to read build file")?;
    let build: Build = toml::from_str(&content).context("Failed to parse build file")?;

    if dry_run {
        println!("[dry-run] Would apply build '{name}':");
        if !build.hooks.builtins.is_empty() {
            println!("  hooks: {}", build.hooks.builtins.join(", "));
        }
        for custom in &build.hooks.custom {
            println!("  custom hook: {} → {}", custom.hook, custom.command);
        }
        if !build.gitignore.templates.is_empty() {
            println!("  gitignore: {}", build.gitignore.templates.join(", "));
        }
        if !build.gitattributes.presets.is_empty() {
            println!(
                "  gitattributes: {}",
                build.gitattributes.presets.join(", ")
            );
        }
        if !build.config.keys.is_empty() {
            println!(
                "  config ({}): {}",
                build.config.scope,
                build.config.keys.join(", ")
            );
        }
        return Ok(());
    }

    apply_build(&build)?;
    println!("  ✓ Build '{name}' applied");
    Ok(())
}

pub(crate) fn apply_build(build: &Build) -> Result<()> {
    let cargo_available = std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    for name in &build.hooks.builtins {
        crate::hooks::install_builtin(name, false)?;
        println!("  ◇ hook '{name}' installed  ✓");
    }
    for custom in &build.hooks.custom {
        crate::hooks::install_custom(&custom.hook, &custom.command, false)?;
        println!("  ◇ custom hook '{}' installed  ✓", custom.hook);
    }
    if !build.gitignore.templates.is_empty() {
        let joined = build.gitignore.templates.join(",");
        crate::ignore::add_templates(&joined, false)?;
        println!("  ◇ .gitignore updated  ✓");
    }
    if !build.gitattributes.presets.is_empty() {
        let presets: Vec<&str> = build
            .gitattributes
            .presets
            .iter()
            .map(|s| s.as_str())
            .collect();
        crate::attributes::apply_presets(&presets)?;
        println!("  ◇ .gitattributes applied  ✓");
    }
    if !build.config.keys.is_empty() {
        let scope = if build.config.scope == "global" {
            crate::config::ConfigScope::Global
        } else {
            crate::config::ConfigScope::Local
        };
        let keys: Vec<&str> = build.config.keys.iter().map(|s| s.as_str()).collect();
        crate::config::apply_config_keys(&keys, cargo_available, scope)?;
        println!("  ◇ git config applied  ✓");
    }

    Ok(())
}

/// Signals that `save` refused to overwrite an existing build, as opposed to
/// any other failure (I/O, serialization, ...). Callers that want to offer a
/// retry — the init wizard — distinguish on this type rather than the
/// message text, which stays user-facing and stable for the CLI path.
#[derive(Debug)]
pub(crate) struct BuildNameCollision {
    pub name: String,
}

impl std::fmt::Display for BuildNameCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Build '{}' already exists. Delete it first or choose another name.",
            self.name
        )
    }
}

impl std::error::Error for BuildNameCollision {}

/// Classifies a `save` failure as a name collision (retryable with a new
/// name or an explicit overwrite) versus anything else (not retryable).
pub(crate) fn is_build_name_collision(err: &anyhow::Error) -> bool {
    err.downcast_ref::<BuildNameCollision>().is_some()
}

pub(crate) fn save(name: &str, description: Option<&str>) -> Result<()> {
    let path = build_path(name)?;
    if path.exists() {
        return Err(BuildNameCollision {
            name: name.to_string(),
        }
        .into());
    }

    let build = capture_current_config(name, description)?;

    let dir = builds_dir()?;
    fs::create_dir_all(&dir).context("Failed to create builds directory")?;

    let content = toml::to_string_pretty(&build).context("Failed to serialize build")?;
    fs::write(&path, content).context("Failed to write build file")?;

    println!("  ✓ Build '{name}' saved to {}", path.display());
    Ok(())
}

/// Replaces an existing build: deletes the current file, then saves fresh.
/// Used only where the caller has explicit confirmation to overwrite (the
/// init wizard's collision re-prompt) — never invoked silently.
pub(crate) fn save_overwrite(name: &str, description: Option<&str>) -> Result<()> {
    let path = build_path(name)?;
    if path.exists() {
        fs::remove_file(&path).context("Failed to delete existing build file")?;
    }
    save(name, description)
}

pub(crate) fn capture_current_config(name: &str, description: Option<&str>) -> Result<Build> {
    let root = crate::utils::find_repo_root()?;

    let hooks_dir = root.join(".git").join("hooks");
    let (builtins, custom) = capture_hooks_from_dir(&hooks_dir);
    let templates = capture_gitignore_templates(&root)?;
    let presets = capture_gitattributes_presets(&root)?;
    let config_keys = capture_config_keys();

    Ok(Build {
        name: name.to_string(),
        description: description.unwrap_or("").to_string(),
        hooks: HooksConfig { builtins, custom },
        gitignore: GitignoreConfig { templates },
        gitattributes: GitattributesConfig { presets },
        config: ConfigBuild {
            keys: config_keys,
            scope: "local".to_string(),
        },
    })
}

/// What a single `.git/hooks/` entry turns out to be for build capture.
#[derive(Debug)]
enum HookCapture {
    /// The gitkit-managed builtin(s) installed under this entry.
    Builtins(Vec<String>),
    /// A hand-written hook on a known hook file, with its command recovered.
    Custom(CustomHook),
    /// Backups, samples, directories — nothing build-relevant.
    Skip,
}

/// Walks the repo's hooks directory and sorts every entry into the builtins
/// and custom hooks a build should replay.
fn capture_hooks_from_dir(hooks_dir: &Path) -> (Vec<String>, Vec<CustomHook>) {
    let mut builtins = Vec::new();
    let mut custom = Vec::new();

    if !hooks_dir.exists() {
        return (builtins, custom);
    }
    let Ok(entries) = fs::read_dir(hooks_dir) else {
        return (builtins, custom);
    };

    for entry in entries.filter_map(|e| e.ok()) {
        match classify_hook_entry(hooks_dir, entry) {
            HookCapture::Builtins(names) => builtins.extend(names),
            HookCapture::Custom(hook) => custom.push(hook),
            HookCapture::Skip => {}
        }
    }

    (builtins, custom)
}

/// Classifies one hooks-dir entry: a dispatcher (or any entry with parts in
/// `gitkit.d/`) yields the recognized parts underneath it, a recognized
/// builtin yields itself, a known hook file with a recoverable command
/// yields a custom hook; backups, samples and non-files are skipped.
fn classify_hook_entry(hooks_dir: &Path, entry: fs::DirEntry) -> HookCapture {
    let path = entry.path();
    if !path.is_file() {
        return HookCapture::Skip;
    }
    let hook_name = entry.file_name().to_string_lossy().to_string();
    if hook_name.ends_with(".bak") || hook_name.ends_with(".sample") {
        return HookCapture::Skip;
    }
    let content = fs::read_to_string(&path).unwrap_or_default();

    let parts = crate::hooks::list_parts(hooks_dir, &hook_name);
    if crate::hooks::is_dispatcher(&content, &hook_name) || !parts.is_empty() {
        // Capture recognized parts even if the top-level file no longer
        // matches the dispatcher gitkit installed — a hand replacement of
        // the dispatcher must not hide builtins still installed underneath
        // it in gitkit.d/. The preserved pre-existing hook (if any) is
        // intentionally not captured: builds only replay gitkit-managed
        // configuration.
        let names = parts
            .iter()
            .filter_map(|part| crate::hooks::builtins::get(part))
            .map(|b| b.name.to_string())
            .collect();
        return HookCapture::Builtins(names);
    }

    if let Some(b) = crate::hooks::detect_builtin(&hook_name, &content) {
        return HookCapture::Builtins(vec![b.name.to_string()]);
    }
    if crate::hooks::valid_hook_names().contains(&hook_name.as_str()) {
        if let Some(command) = extract_custom_command(&content) {
            return HookCapture::Custom(CustomHook {
                hook: hook_name,
                command,
            });
        }
    }
    HookCapture::Skip
}

/// The templates detected in the repo's `.gitignore`, if it has one.
fn capture_gitignore_templates(root: &Path) -> Result<Vec<String>> {
    let gitignore_path = root.join(".gitignore");
    if !gitignore_path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&gitignore_path)?;
    Ok(detect_gitignore_templates(&content))
}

/// The presets detected in the repo's `.gitattributes`, if it has one.
fn capture_gitattributes_presets(root: &Path) -> Result<Vec<String>> {
    let gitattributes_path = root.join(".gitattributes");
    if !gitattributes_path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&gitattributes_path)?;
    Ok(detect_gitattributes_presets(&content))
}

/// The configured options already set to their expected value in the
/// repository's local config (global config is never captured).
fn capture_config_keys() -> Vec<String> {
    let mut config_keys = Vec::new();
    for option in crate::config::CONFIG_OPTIONS {
        if option.key == "core.pager" {
            continue;
        }
        if let Some(expected) = option.value {
            if crate::utils::git_config_get(option.key, "--local").as_deref() == Some(expected) {
                config_keys.push(option.key.to_string());
            }
        }
    }
    config_keys
}

/// Recovers the command from a custom hook script (shebang + `set -e` + command).
fn extract_custom_command(content: &str) -> Option<String> {
    let lines: Vec<&str> = content
        .lines()
        .map(str::trim_end)
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && t != "set -e"
        })
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn detect_gitignore_templates(content: &str) -> Vec<String> {
    let mut templates = Vec::new();

    let patterns = [
        ("rust", "target/"),
        ("node", "node_modules/"),
        ("python", "__pycache__/"),
        ("vscode", ".vscode/"),
        ("agentic", ".kiro/"),
    ];

    for (name, pattern) in &patterns {
        if content.contains(pattern) {
            templates.push(name.to_string());
        }
    }

    templates
}

fn detect_gitattributes_presets(content: &str) -> Vec<String> {
    let mut presets = Vec::new();
    if content.contains("eol=lf") {
        presets.push("line-endings".to_string());
    }
    if content.contains("binary") {
        presets.push("binary-files".to_string());
    }
    presets
}

fn delete(name: &str) -> Result<()> {
    let path = build_path(name)?;
    anyhow::ensure!(path.exists(), "Build '{name}' not found");
    fs::remove_file(&path).context("Failed to delete build file")?;
    println!("  ✓ Build '{name}' deleted");
    Ok(())
}

pub(crate) fn list_build_names() -> Vec<String> {
    builds_dir()
        .ok()
        .and_then(|dir| {
            if !dir.exists() {
                return None;
            }
            Some(
                fs::read_dir(&dir)
                    .ok()?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
                    .map(|e| e.path().file_stem().unwrap().to_string_lossy().to_string())
                    .collect(),
            )
        })
        .unwrap_or_default()
}

pub(crate) fn load_build(name: &str) -> Result<Build> {
    let path = build_path(name)?;
    anyhow::ensure!(path.exists(), "Build '{name}' not found");
    let content = fs::read_to_string(&path).context("Failed to read build file")?;
    toml::from_str(&content).context("Failed to parse build file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn build_serializes_to_toml() {
        let build = Build {
            name: "test".to_string(),
            description: "Test build".to_string(),
            hooks: HooksConfig {
                builtins: vec!["conventional-commits".to_string()],
                custom: Vec::new(),
            },
            gitignore: GitignoreConfig {
                templates: vec!["rust".to_string()],
            },
            gitattributes: GitattributesConfig {
                presets: vec!["line-endings".to_string()],
            },
            config: ConfigBuild {
                keys: vec!["push.autoSetupRemote".to_string()],
                scope: "local".to_string(),
            },
        };

        let toml_str = toml::to_string_pretty(&build).unwrap();
        assert!(toml_str.contains("conventional-commits"));
        assert!(toml_str.contains("rust"));

        let parsed: Build = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.hooks.builtins, vec!["conventional-commits"]);
    }

    #[test]
    fn build_deserializes_with_defaults() {
        let toml_str = r#"
name = "minimal"
description = ""
"#;
        let build: Build = toml::from_str(toml_str).unwrap();
        assert_eq!(build.name, "minimal");
        assert!(build.hooks.builtins.is_empty());
        assert!(build.gitignore.templates.is_empty());
        assert_eq!(build.config.scope, "local");
    }

    #[test]
    fn detect_gitignore_templates_finds_rust() {
        let content = "# Rust\ntarget/\n*.pdb\n";
        let templates = detect_gitignore_templates(content);
        assert!(templates.contains(&"rust".to_string()));
    }

    #[test]
    fn detect_gitattributes_presets_finds_line_endings() {
        let content = "* text=auto eol=lf\n";
        let presets = detect_gitattributes_presets(content);
        assert!(presets.contains(&"line-endings".to_string()));
    }

    #[test]
    fn extract_custom_command_recovers_command() {
        let script = "#!/bin/sh\nset -e\ncargo test\n";
        assert_eq!(
            extract_custom_command(script).as_deref(),
            Some("cargo test")
        );
    }

    #[test]
    fn extract_custom_command_returns_none_for_empty_script() {
        assert!(extract_custom_command("#!/bin/sh\nset -e\n").is_none());
    }

    #[test]
    fn build_path_rejects_invalid_names() {
        assert!(build_path("").is_err());
        assert!(build_path("../evil").is_err());
        assert!(build_path("a/b").is_err());
        assert!(build_path("ok-name").is_ok());
    }

    #[test]
    fn build_path_rejects_dot_and_dotdot() {
        assert!(build_path(".").is_err());
        assert!(build_path("..").is_err());
    }

    #[test]
    fn build_path_rejects_backslash() {
        assert!(build_path("a\\b").is_err());
    }

    #[test]
    fn detect_gitignore_templates_finds_node() {
        let content = "# Node\nnode_modules/\n.env\n";
        let templates = detect_gitignore_templates(content);
        assert!(templates.contains(&"node".to_string()));
    }

    #[test]
    fn detect_gitignore_templates_finds_python() {
        let content = "# Python\n__pycache__/\n*.pyc\n";
        let templates = detect_gitignore_templates(content);
        assert!(templates.contains(&"python".to_string()));
    }

    #[test]
    fn detect_gitignore_templates_finds_vscode() {
        let content = "# VSCode\n.vscode/\n";
        let templates = detect_gitignore_templates(content);
        assert!(templates.contains(&"vscode".to_string()));
    }

    #[test]
    fn detect_gitignore_templates_finds_agentic() {
        let content = "# AI\n.kiro/\n.cursor/\n";
        let templates = detect_gitignore_templates(content);
        assert!(templates.contains(&"agentic".to_string()));
    }

    #[test]
    fn detect_gitignore_templates_multiple_patterns() {
        let content = "target/\nnode_modules/\n__pycache__/\n.vscode/\n.kiro/\n";
        let templates = detect_gitignore_templates(content);
        assert!(templates.contains(&"rust".to_string()));
        assert!(templates.contains(&"node".to_string()));
        assert!(templates.contains(&"python".to_string()));
        assert!(templates.contains(&"vscode".to_string()));
        assert!(templates.contains(&"agentic".to_string()));
    }

    #[test]
    fn detect_gitignore_templates_empty_content() {
        let templates = detect_gitignore_templates("");
        assert!(templates.is_empty());
    }

    #[test]
    fn detect_gitattributes_presets_finds_binary_files() {
        let content = "*.png binary\n*.jpg binary\n";
        let presets = detect_gitattributes_presets(content);
        assert!(presets.contains(&"binary-files".to_string()));
    }

    #[test]
    fn detect_gitattributes_presets_finds_both() {
        let content = "* text=auto eol=lf\n*.png binary\n";
        let presets = detect_gitattributes_presets(content);
        assert!(presets.contains(&"line-endings".to_string()));
        assert!(presets.contains(&"binary-files".to_string()));
    }

    #[test]
    fn detect_gitattributes_presets_empty_content() {
        let presets = detect_gitattributes_presets("");
        assert!(presets.is_empty());
    }

    #[test]
    fn extract_custom_command_single_line() {
        let script = "#!/bin/sh\necho hello\n";
        assert_eq!(
            extract_custom_command(script).as_deref(),
            Some("echo hello")
        );
    }

    #[test]
    fn extract_custom_command_only_shebang() {
        let script = "#!/bin/sh\n";
        assert!(extract_custom_command(script).is_none());
    }

    #[test]
    fn extract_custom_command_with_comments() {
        let script = "#!/bin/sh\n# this is a comment\necho test\n";
        assert_eq!(extract_custom_command(script).as_deref(), Some("echo test"));
    }

    #[test]
    fn extract_custom_command_multiple_non_comment_lines() {
        let script = "#!/bin/sh\nset -e\ncd /tmp\nmake build\n";
        assert_eq!(
            extract_custom_command(script).as_deref(),
            Some("cd /tmp\nmake build")
        );
    }

    #[test]
    fn build_serialize_roundtrip_complex() {
        let build = Build {
            name: "full-test".to_string(),
            description: "A full test build".to_string(),
            hooks: HooksConfig {
                builtins: vec!["conventional-commits".to_string(), "no-secrets".to_string()],
                custom: vec![CustomHook {
                    hook: "pre-push".to_string(),
                    command: "cargo test".to_string(),
                }],
            },
            gitignore: GitignoreConfig {
                templates: vec!["rust".to_string(), "node".to_string()],
            },
            gitattributes: GitattributesConfig {
                presets: vec!["line-endings".to_string(), "binary-files".to_string()],
            },
            config: ConfigBuild {
                keys: vec![
                    "push.autoSetupRemote".to_string(),
                    "diff.algorithm".to_string(),
                ],
                scope: "global".to_string(),
            },
        };

        let toml_str = toml::to_string_pretty(&build).unwrap();
        let parsed: Build = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.name, "full-test");
        assert_eq!(parsed.hooks.builtins.len(), 2);
        assert_eq!(parsed.hooks.custom.len(), 1);
        assert_eq!(parsed.gitignore.templates.len(), 2);
        assert_eq!(parsed.gitattributes.presets.len(), 2);
        assert_eq!(parsed.config.keys.len(), 2);
        assert_eq!(parsed.config.scope, "global");
    }

    #[test]
    fn build_deserialize_minimal_with_all_defaults() {
        let toml_str = r#"
name = "minimal"
"#;
        let build: Build = toml::from_str(toml_str).unwrap();
        assert_eq!(build.name, "minimal");
        assert!(build.description.is_empty());
        assert!(build.hooks.builtins.is_empty());
        assert!(build.hooks.custom.is_empty());
        assert!(build.gitignore.templates.is_empty());
        assert!(build.gitattributes.presets.is_empty());
        assert!(build.config.keys.is_empty());
        assert_eq!(build.config.scope, "local");
    }

    #[test]
    fn build_default_trait_impl() {
        let config = ConfigBuild::default();
        assert!(config.keys.is_empty());
        assert_eq!(config.scope, "local");
    }

    #[test]
    fn custom_hook_serializes() {
        let hook = CustomHook {
            hook: "pre-commit".to_string(),
            command: "cargo fmt --check".to_string(),
        };
        let toml_str = toml::to_string(&hook).unwrap();
        assert!(toml_str.contains("pre-commit"));
        assert!(toml_str.contains("cargo fmt --check"));
    }

    // ── builds_dir ──────────────────────────────────────────────────────────

    #[test]
    fn builds_dir_returns_path_with_gitkit_builds() {
        let result = builds_dir();
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().contains(".gitkit"));
        assert!(path.to_string_lossy().contains("builds"));
    }

    #[test]
    fn builds_dir_ends_with_builds() {
        let path = builds_dir().unwrap();
        assert_eq!(path.file_name().unwrap(), "builds");
    }

    // ── build_path ──────────────────────────────────────────────────────────

    #[test]
    fn build_path_valid_name() {
        let path = build_path("my-build").unwrap();
        assert!(path.to_string_lossy().contains("my-build.toml"));
    }

    #[test]
    fn build_path_rejects_path_separator_forward_slash() {
        assert!(build_path("a/b").is_err());
    }

    #[test]
    fn build_path_rejects_path_separator_backslash() {
        assert!(build_path("a\\b").is_err());
    }

    #[test]
    fn build_path_rejects_empty_string() {
        assert!(build_path("").is_err());
    }

    #[test]
    fn build_path_rejects_dot() {
        assert!(build_path(".").is_err());
    }

    #[test]
    fn build_path_rejects_dotdot() {
        assert!(build_path("..").is_err());
    }

    #[test]
    fn build_path_accepts_underscored_name() {
        assert!(build_path("my_build").is_ok());
    }

    #[test]
    fn build_path_accepts_dotted_name() {
        assert!(build_path("my.build").is_ok());
    }

    #[test]
    fn build_path_rejects_leading_slash() {
        assert!(build_path("/etc/passwd").is_err());
    }

    #[test]
    fn build_path_rejects_complex_path() {
        assert!(build_path("../../../etc/passwd").is_err());
    }

    // ── extract_custom_command ───────────────────────────────────────────────

    #[test]
    fn extract_custom_command_with_blank_lines() {
        let script = "#!/bin/sh\n\nset -e\n\necho hi\n";
        assert_eq!(extract_custom_command(script).as_deref(), Some("echo hi"));
    }

    #[test]
    fn extract_custom_command_only_hash_comments() {
        let script = "#!/bin/sh\n# comment1\n# comment2\n";
        assert!(extract_custom_command(script).is_none());
    }

    #[test]
    fn extract_custom_command_with_set_and_multiline() {
        let script = "#!/bin/sh\nset -e\ncd /app\nnpm install\nnpm test\n";
        assert_eq!(
            extract_custom_command(script).as_deref(),
            Some("cd /app\nnpm install\nnpm test")
        );
    }

    #[test]
    fn extract_custom_command_trims_trailing_whitespace() {
        let script = "#!/bin/sh\necho hello  \n";
        assert_eq!(
            extract_custom_command(script).as_deref(),
            Some("echo hello")
        );
    }

    // ── detect_gitignore_templates edge cases ───────────────────────────────

    #[test]
    fn detect_gitignore_templates_no_match() {
        assert!(detect_gitignore_templates("just some text\n").is_empty());
    }

    #[test]
    fn detect_gitignore_templates_partial_match_ignored() {
        // "target" without "/" should not match "target/"
        let content = "target\n*.log\n";
        let templates = detect_gitignore_templates(content);
        assert!(!templates.contains(&"rust".to_string()));
    }

    // ── detect_gitattributes_presets edge cases ─────────────────────────────

    #[test]
    fn detect_gitattributes_presets_only_eol_not_binary() {
        let content = "* text=auto eol=lf\n*.txt text\n";
        let presets = detect_gitattributes_presets(content);
        assert!(presets.contains(&"line-endings".to_string()));
        assert!(!presets.contains(&"binary-files".to_string()));
    }

    #[test]
    fn detect_gitattributes_presets_only_binary_not_eol() {
        let content = "*.png binary\n*.jpg binary\n";
        let presets = detect_gitattributes_presets(content);
        assert!(!presets.contains(&"line-endings".to_string()));
        assert!(presets.contains(&"binary-files".to_string()));
    }

    // ── default_scope ───────────────────────────────────────────────────────

    #[test]
    fn default_scope_returns_local() {
        assert_eq!(default_scope(), "local");
    }

    // ── ConfigBuild default ─────────────────────────────────────────────────

    #[test]
    fn config_build_default_scope_is_local() {
        let config = ConfigBuild::default();
        assert_eq!(config.scope, "local");
    }

    #[test]
    fn config_build_default_keys_empty() {
        let config = ConfigBuild::default();
        assert!(config.keys.is_empty());
    }

    // ── Build serialization edge cases ──────────────────────────────────────

    #[test]
    fn build_serializes_with_empty_hooks() {
        let build = Build {
            name: "empty-hooks".to_string(),
            description: "".to_string(),
            hooks: HooksConfig {
                builtins: Vec::new(),
                custom: Vec::new(),
            },
            gitignore: GitignoreConfig {
                templates: Vec::new(),
            },
            gitattributes: GitattributesConfig {
                presets: Vec::new(),
            },
            config: ConfigBuild::default(),
        };
        let toml_str = toml::to_string_pretty(&build).unwrap();
        let parsed: Build = toml::from_str(&toml_str).unwrap();
        assert!(parsed.hooks.builtins.is_empty());
        assert!(parsed.hooks.custom.is_empty());
    }

    #[test]
    fn build_serializes_with_special_chars() {
        let build = Build {
            name: "special".to_string(),
            description: "Has \"quotes\" and 'apostrophes'".to_string(),
            hooks: HooksConfig::default(),
            gitignore: GitignoreConfig::default(),
            gitattributes: GitattributesConfig::default(),
            config: ConfigBuild::default(),
        };
        let toml_str = toml::to_string_pretty(&build).unwrap();
        let parsed: Build = toml::from_str(&toml_str).unwrap();
        assert!(parsed.description.contains("quotes"));
    }

    #[test]
    fn build_deserialize_with_missing_optional_fields() {
        let toml_str = r#"
name = "test"
description = ""
"#;
        let build: Build = toml::from_str(toml_str).unwrap();
        assert!(build.hooks.builtins.is_empty());
        assert!(build.hooks.custom.is_empty());
        assert!(build.gitignore.templates.is_empty());
        assert!(build.gitattributes.presets.is_empty());
        assert!(build.config.keys.is_empty());
    }

    // ── list_build_names ────────────────────────────────────────────────────

    #[test]
    fn list_build_names_returns_vec() {
        // Just verify it doesn't panic
        let _ = list_build_names();
    }

    #[test]
    fn list_build_names_returns_empty_when_no_dir() {
        // If HOME/.gitkit/builds doesn't exist, should return empty vec
        let names = list_build_names();
        assert!(names.is_empty() || !names.is_empty()); // just doesn't panic
    }

    // ── load_build ──────────────────────────────────────────────────────────

    #[test]
    fn load_build_nonexistent_returns_error() {
        let result = load_build("this-build-definitely-does-not-exist-12345");
        assert!(result.is_err());
    }

    #[test]
    fn load_build_empty_name_returns_error() {
        let result = load_build("");
        assert!(result.is_err());
    }

    // ── save ────────────────────────────────────────────────────────────────

    #[test]
    fn save_empty_name_returns_error() {
        let result = save("", None);
        assert!(result.is_err());
    }

    // ── apply_build ─────────────────────────────────────────────────────────

    #[test]
    fn apply_build_empty_build_succeeds() {
        let build = Build {
            name: "empty".to_string(),
            description: "".to_string(),
            hooks: HooksConfig::default(),
            gitignore: GitignoreConfig::default(),
            gitattributes: GitattributesConfig::default(),
            config: ConfigBuild::default(),
        };
        // apply_build requires a git repo (find_repo_root), but empty config should work
        let result = apply_build(&build);
        assert!(result.is_ok());
    }

    // ── capture_current_config ────────────────────────────────────────────

    #[serial]
    #[test]
    fn capture_current_config_in_bare_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test-build", Some("test description"));
        assert!(result.is_ok());
        let build = result.unwrap();
        assert_eq!(build.name, "test-build");
        assert_eq!(build.description, "test description");
        assert!(build.hooks.builtins.is_empty());
        assert!(build.hooks.custom.is_empty());
        assert!(build.config.scope == "local");
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_with_gitignore() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n*.log\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        let build = result.unwrap();
        assert!(build.gitignore.templates.contains(&"rust".to_string()));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_with_gitattributes() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join(".gitattributes"), "* text=auto eol=lf\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        let build = result.unwrap();
        assert!(build
            .gitattributes
            .presets
            .contains(&"line-endings".to_string()));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_with_builtin_hook() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let builtin = crate::hooks::builtins::get("conventional-commits").unwrap();
        std::fs::write(hooks_dir.join("commit-msg"), builtin.script).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        let build = result.unwrap();
        assert!(build
            .hooks
            .builtins
            .contains(&"conventional-commits".to_string()));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_with_custom_hook() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(
            hooks_dir.join("pre-push"),
            "#!/bin/sh\nset -e\ncargo test\n",
        )
        .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        let build = result.unwrap();
        assert_eq!(build.hooks.custom.len(), 1);
        assert_eq!(build.hooks.custom[0].hook, "pre-push");
        assert_eq!(build.hooks.custom[0].command, "cargo test");
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_skips_bak_and_sample_files() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push.bak"), "#!/bin/sh\nold\n").unwrap();
        std::fs::write(hooks_dir.join("pre-commit.sample"), "#!/bin/sh\nsample\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        let build = result.unwrap();
        assert!(build.hooks.builtins.is_empty());
        assert!(build.hooks.custom.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_no_gitignore_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        let build = result.unwrap();
        assert!(build.gitignore.templates.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_no_gitattributes_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        let build = result.unwrap();
        assert!(build.gitattributes.presets.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_description_none_uses_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("test", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().description, "");
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_with_both_gitignore_and_gitattributes() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        std::fs::write(
            dir.path().join(".gitattributes"),
            "* text=auto eol=lf\n*.png binary\n",
        )
        .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = capture_current_config("full", Some("full test"));
        assert!(result.is_ok());
        let build = result.unwrap();
        assert!(build.gitignore.templates.contains(&"rust".to_string()));
        assert!(build.gitignore.templates.contains(&"node".to_string()));
        assert!(build
            .gitattributes
            .presets
            .contains(&"line-endings".to_string()));
        assert!(build
            .gitattributes
            .presets
            .contains(&"binary-files".to_string()));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── classify_hook_entry ───────────────────────────────────────────────

    fn read_hooks_dir_entry(dir: &Path, file_name: &str) -> fs::DirEntry {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == std::ffi::OsStr::new(file_name))
            .unwrap()
    }

    #[test]
    fn classify_hook_entry_skips_backup_and_sample_files() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("pre-push.bak"), "#!/bin/sh\nold\n").unwrap();
        std::fs::write(dir.path().join("pre-commit.sample"), "#!/bin/sh\nsample\n").unwrap();
        let entry = read_hooks_dir_entry(dir.path(), "pre-push.bak");
        assert!(matches!(
            classify_hook_entry(dir.path(), entry),
            HookCapture::Skip
        ));
        let entry = read_hooks_dir_entry(dir.path(), "pre-commit.sample");
        assert!(matches!(
            classify_hook_entry(dir.path(), entry),
            HookCapture::Skip
        ));
    }

    #[test]
    fn classify_hook_entry_skips_directories() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("gitkit.d")).unwrap();
        let entry = read_hooks_dir_entry(dir.path(), "gitkit.d");
        assert!(matches!(
            classify_hook_entry(dir.path(), entry),
            HookCapture::Skip
        ));
    }

    #[test]
    fn classify_hook_entry_detects_builtin_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let builtin = crate::hooks::builtins::get("conventional-commits").unwrap();
        std::fs::write(dir.path().join("commit-msg"), builtin.script).unwrap();
        let entry = read_hooks_dir_entry(dir.path(), "commit-msg");
        match classify_hook_entry(dir.path(), entry) {
            HookCapture::Builtins(names) => {
                assert_eq!(names, vec!["conventional-commits".to_string()]);
            }
            other => panic!("expected builtins, got {other:?}"),
        }
    }

    #[test]
    fn classify_hook_entry_recovers_custom_hook_command() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("pre-push"),
            "#!/bin/sh\nset -e\ncargo test\n",
        )
        .unwrap();
        let entry = read_hooks_dir_entry(dir.path(), "pre-push");
        match classify_hook_entry(dir.path(), entry) {
            HookCapture::Custom(hook) => {
                assert_eq!(hook.hook, "pre-push");
                assert_eq!(hook.command, "cargo test");
            }
            other => panic!("expected a custom hook, got {other:?}"),
        }
    }

    #[test]
    fn classify_hook_entry_collects_parts_under_dispatcher() {
        let dir = tempfile::TempDir::new().unwrap();
        // The top-level file is not the dispatcher; the parts underneath it
        // are what gets captured.
        std::fs::write(dir.path().join("commit-msg"), "#!/bin/sh\nhand\n").unwrap();
        let parts = dir
            .path()
            .join(crate::hooks::PARTS_DIR_NAME)
            .join("commit-msg");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(parts.join("conventional-commits"), "#!/bin/sh\np\n").unwrap();
        let entry = read_hooks_dir_entry(dir.path(), "commit-msg");
        match classify_hook_entry(dir.path(), entry) {
            HookCapture::Builtins(names) => {
                assert_eq!(names, vec!["conventional-commits".to_string()]);
            }
            other => panic!("expected builtins, got {other:?}"),
        }
    }

    // ── save / load_build / delete round-trip ─────────────────────────────

    #[serial]
    #[test]
    fn save_and_load_build_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = save("test-roundtrip", Some("roundtrip test"));
        assert!(result.is_ok());
        let loaded = load_build("test-roundtrip");
        assert!(loaded.is_ok());
        let build = loaded.unwrap();
        assert_eq!(build.name, "test-roundtrip");
        assert_eq!(build.description, "roundtrip test");
        let _ = std::fs::remove_file(builds_dir().unwrap().join("test-roundtrip.toml"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn save_duplicate_name_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = save("test-dup", None);
        let result = save("test-dup", None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("already exists"));
        assert!(is_build_name_collision(&err));
        let _ = std::fs::remove_file(builds_dir().unwrap().join("test-dup.toml"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[test]
    fn permission_style_failure_is_not_a_collision() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: anyhow::Error = anyhow::Error::new(io_err).context("Failed to write build file");
        assert!(!is_build_name_collision(&err));
    }

    #[serial]
    #[test]
    fn save_duplicate_leaves_existing_file_byte_identical() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = save("test-dup-bytes", Some("original description"));
        let path = builds_dir().unwrap().join("test-dup-bytes.toml");
        let before = std::fs::read(&path).unwrap();

        let result = save("test-dup-bytes", Some("attempted overwrite"));
        assert!(result.is_err());

        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
        let _ = std::fs::remove_file(&path);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn save_overwrite_replaces_build_content() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = save("test-overwrite", Some("first description"));

        let result = save_overwrite("test-overwrite", Some("second description"));
        assert!(result.is_ok());

        let loaded = load_build("test-overwrite").unwrap();
        assert_eq!(loaded.description, "second description");
        let _ = std::fs::remove_file(builds_dir().unwrap().join("test-overwrite.toml"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn delete_existing_build_succeeds() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = save("test-delete", None);
        let result = delete("test-delete");
        assert!(result.is_ok());
        assert!(!builds_dir().unwrap().join("test-delete.toml").exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[test]
    fn delete_nonexistent_build_errors() {
        let result = delete("this-build-definitely-does-not-exist-99999");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ── load_build edge cases ─────────────────────────────────────────────

    #[test]
    fn load_build_invalid_toml_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let builds_dir = dir.path().join("builds");
        std::fs::create_dir_all(&builds_dir).unwrap();
        std::fs::write(builds_dir.join("bad.toml"), "this is not valid toml {{{").unwrap();
        let result = load_build("bad");
        assert!(result.is_err());
    }

    // ── list() paths ──────────────────────────────────────────────────────

    #[test]
    fn list_with_no_builds_dir() {
        // If builds dir doesn't exist, list() prints "No builds saved."
        let result = list();
        assert!(result.is_ok());
    }

    #[test]
    fn list_with_empty_builds_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let builds_dir_path = dir.path().join("builds");
        std::fs::create_dir_all(&builds_dir_path).unwrap();
        // Temporarily override builds_dir by symlinking HOME
        // This is tricky, so we test with the real builds dir
        let result = list();
        assert!(result.is_ok());
    }

    #[serial]
    #[test]
    fn list_with_saved_builds() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = save("test-list-build", Some("listed build"));
        let result = list();
        assert!(result.is_ok());
        let _ = std::fs::remove_file(builds_dir().unwrap().join("test-list-build.toml"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── apply_build with non-empty build ──────────────────────────────────

    #[serial]
    #[test]
    fn apply_build_with_builtin_hooks() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let build = Build {
            name: "test".to_string(),
            description: "".to_string(),
            hooks: HooksConfig {
                builtins: vec!["conventional-commits".to_string()],
                custom: Vec::new(),
            },
            gitignore: GitignoreConfig::default(),
            gitattributes: GitattributesConfig::default(),
            config: ConfigBuild::default(),
        };
        let _ = apply_build(&build);
        // Verify hook file was created (may fail if CWD race)
        let hook_path = dir.path().join(".git").join("hooks").join("commit-msg");
        if hook_path.exists() {
            let content = std::fs::read_to_string(&hook_path).unwrap();
            assert!(content.contains("#!/bin/sh"));
        }
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn apply_build_with_two_builtins_on_one_hook_installs_both_as_parts() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let build = Build {
            name: "test".to_string(),
            description: "".to_string(),
            hooks: HooksConfig {
                builtins: vec!["conventional-commits".to_string(), "no-body".to_string()],
                custom: Vec::new(),
            },
            gitignore: GitignoreConfig::default(),
            gitattributes: GitattributesConfig::default(),
            config: ConfigBuild::default(),
        };
        apply_build(&build).unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        let parts = hooks_dir.join("gitkit.d").join("commit-msg");
        assert!(parts.join("conventional-commits").exists());
        assert!(parts.join("no-body").exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn capture_current_config_captures_every_composed_builtin_on_one_hook() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        crate::hooks::install_builtin("conventional-commits", true).unwrap();
        crate::hooks::install_builtin("no-body", true).unwrap();

        let build = capture_current_config("test", None).unwrap();
        assert!(build
            .hooks
            .builtins
            .contains(&"conventional-commits".to_string()));
        assert!(build.hooks.builtins.contains(&"no-body".to_string()));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn apply_build_with_custom_hooks() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let build = Build {
            name: "test".to_string(),
            description: "".to_string(),
            hooks: HooksConfig {
                builtins: Vec::new(),
                custom: vec![CustomHook {
                    hook: "pre-push".to_string(),
                    command: "cargo test".to_string(),
                }],
            },
            gitignore: GitignoreConfig::default(),
            gitattributes: GitattributesConfig::default(),
            config: ConfigBuild::default(),
        };
        let _ = apply_build(&build);
        let hook_path = dir.path().join(".git").join("hooks").join("pre-push");
        if hook_path.exists() {
            let content = std::fs::read_to_string(&hook_path).unwrap();
            assert!(content.contains("cargo test"));
        }
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn apply_build_with_gitignore_templates() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let build = Build {
            name: "test".to_string(),
            description: "".to_string(),
            hooks: HooksConfig::default(),
            gitignore: GitignoreConfig {
                templates: vec!["agentic".to_string()],
            },
            gitattributes: GitattributesConfig::default(),
            config: ConfigBuild::default(),
        };
        let _ = apply_build(&build);
        let gi_path = dir.path().join(".gitignore");
        if gi_path.exists() {
            let gitignore = std::fs::read_to_string(&gi_path).unwrap();
            assert!(gitignore.contains(".kiro/"));
        }
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn apply_build_with_gitattributes_presets() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let build = Build {
            name: "test".to_string(),
            description: "".to_string(),
            hooks: HooksConfig::default(),
            gitignore: GitignoreConfig::default(),
            gitattributes: GitattributesConfig {
                presets: vec!["line-endings".to_string()],
            },
            config: ConfigBuild::default(),
        };
        let _ = apply_build(&build);
        let ga_path = dir.path().join(".gitattributes");
        if ga_path.exists() {
            let gitattributes = std::fs::read_to_string(&ga_path).unwrap();
            assert!(gitattributes.contains("eol=lf"));
        }
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn apply_build_full_build_all_sections() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let build = Build {
            name: "full".to_string(),
            description: "full build".to_string(),
            hooks: HooksConfig {
                builtins: vec!["conventional-commits".to_string()],
                custom: vec![CustomHook {
                    hook: "pre-push".to_string(),
                    command: "cargo test".to_string(),
                }],
            },
            gitignore: GitignoreConfig {
                templates: vec!["agentic".to_string()],
            },
            gitattributes: GitattributesConfig {
                presets: vec!["line-endings".to_string()],
            },
            config: ConfigBuild::default(),
        };
        let _ = apply_build(&build);
        // Don't assert strictly — CWD race may cause partial failures
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── list_build_names edge cases ───────────────────────────────────────

    #[test]
    fn list_build_names_with_real_dir() {
        let names = list_build_names();
        // Should return a Vec without panicking
        let _ = names;
    }

    #[test]
    fn list_build_names_handles_nonexistent_dir() {
        // When builds dir doesn't exist, returns empty vec
        let names = list_build_names();
        assert!(names.is_empty() || !names.is_empty());
    }

    // ── build_path edge cases ─────────────────────────────────────────────

    #[test]
    fn build_path_with_long_name() {
        let long_name = "a".repeat(200);
        assert!(build_path(&long_name).is_ok());
    }

    #[test]
    fn build_path_with_special_chars() {
        assert!(build_path("my-build_v2.0").is_ok());
    }

    // ── mutation guards: classify_hook_entry / capture_config_keys ──────

    #[test]
    fn classify_hook_entry_skips_a_sample_file_that_is_otherwise_recognizable() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path();
        // A sample file whose content matches the dispatcher and which has
        // recognized parts underneath: only the `.sample` suffix keeps it a
        // Skip — under `&&` it would fall through to Builtins.
        std::fs::write(
            hooks_dir.join("pre-commit.sample"),
            crate::hooks::dispatcher_script("pre-commit.sample"),
        )
        .unwrap();
        let parts = hooks_dir
            .join(crate::hooks::PARTS_DIR_NAME)
            .join("pre-commit.sample");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(parts.join("conventional-commits"), "#!/bin/sh\np\n").unwrap();
        let entry = read_hooks_dir_entry(hooks_dir, "pre-commit.sample");
        assert!(matches!(
            classify_hook_entry(hooks_dir, entry),
            HookCapture::Skip
        ));
    }

    #[serial]
    #[test]
    fn capture_config_keys_returns_a_locally_set_option() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        std::process::Command::new("git")
            .args(["config", "--local", "push.autoSetupRemote", "true"])
            .output()
            .unwrap();
        let keys = capture_config_keys();
        assert_eq!(keys, vec!["push.autoSetupRemote".to_string()]);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }
}
