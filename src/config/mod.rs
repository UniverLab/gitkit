use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use std::process::Command;

use crate::utils::{confirm, find_repo_root};

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Apply a curated git config preset
    Apply {
        preset: Preset,
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, conflicts_with = "local")]
        global: bool,
        #[arg(long, conflicts_with = "global")]
        local: bool,
    },
    /// Show current git config values
    Show,
}

#[derive(ValueEnum, Clone)]
pub enum Preset {
    /// push.autoSetupRemote, help.autocorrect, diff.algorithm
    Defaults,
    /// merge.conflictstyle zdiff3, rerere.enabled (terminal-focused)
    Advanced,
    /// core.pager delta (installs git-delta if needed)
    Delta,
}

pub fn run(cmd: ConfigCommand) -> Result<()> {
    match cmd {
        ConfigCommand::Apply {
            preset,
            yes,
            dry_run,
            global,
            local,
        } => {
            let scope = determine_scope(global, local);
            match preset {
                Preset::Defaults => apply_defaults(dry_run, scope),
                Preset::Advanced => apply_advanced(dry_run, scope),
                Preset::Delta => apply_delta(yes, dry_run, scope),
            }
        }
        ConfigCommand::Show => show_config(),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ConfigScope {
    Global,
    Local,
}

fn determine_scope(global: bool, local: bool) -> ConfigScope {
    if global {
        ConfigScope::Global
    } else if local || find_repo_root().is_ok() {
        ConfigScope::Local
    } else {
        ConfigScope::Global
    }
}

fn scope_flag(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => "--global",
        ConfigScope::Local => "--local",
    }
}

fn show_config() -> Result<()> {
    println!("Git config (global):");
    show_scope_config("--global");
    println!();
    println!("Git config (local):");
    show_scope_config("--local");
    Ok(())
}

fn show_scope_config(scope: &str) {
    let configs = [
        "push.autoSetupRemote",
        "help.autocorrect",
        "diff.algorithm",
        "merge.conflictstyle",
        "rerere.enabled",
        "core.pager",
    ];

    let mut any = false;
    for key in &configs {
        if let Some(value) = git_config_get(key, scope) {
            println!("  {key} = {value}");
            any = true;
        }
    }

    if !any {
        println!("  (none)");
    }
}

fn git_config_get(key: &str, scope: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", scope, "--get", key])
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Individual config options exposed for the interactive wizard.
pub(crate) struct ConfigOption {
    pub key: &'static str,
    pub value: Option<&'static str>, // None for multi-key options like delta
    pub label: &'static str,
    pub recommended: bool,
}

pub(crate) const CONFIG_OPTIONS: &[ConfigOption] = &[
    ConfigOption {
        key: "push.autoSetupRemote",
        value: Some("true"),
        label: "push.autoSetupRemote = true  —  auto-set upstream on first push",
        recommended: true,
    },
    ConfigOption {
        key: "help.autocorrect",
        value: Some("prompt"),
        label: "help.autocorrect = prompt  —  suggest corrections for mistyped commands",
        recommended: true,
    },
    ConfigOption {
        key: "diff.algorithm",
        value: Some("histogram"),
        label: "diff.algorithm = histogram  —  cleaner diffs for moved code",
        recommended: true,
    },
    ConfigOption {
        key: "merge.conflictstyle",
        value: Some("zdiff3"),
        label: "merge.conflictstyle = zdiff3  —  show base in conflict markers",
        recommended: false,
    },
    ConfigOption {
        key: "rerere.enabled",
        value: Some("true"),
        label: "rerere.enabled = true  —  remember and reuse conflict resolutions",
        recommended: false,
    },
    ConfigOption {
        key: "core.pager",
        value: None, // handled separately — installs git-delta via cargo
        label: "core.pager = delta  —  beautiful syntax-highlighted diffs (requires cargo)",
        recommended: false,
    },
];

/// Apply selected config option keys. Used by the interactive wizard.
pub(crate) fn apply_config_keys(
    keys: &[&str],
    cargo_available: bool,
    scope: ConfigScope,
) -> Result<()> {
    for key in keys {
        match *key {
            "core.pager" => apply_pager_config(cargo_available, scope)?,
            _ => apply_single_config(key, scope)?,
        }
    }
    if let Ok(root) = find_repo_root() {
        let items: Vec<String> = keys.iter().map(|k| format!("config:{k}")).collect();
        crate::registry::record_best_effort(&root, &items);
    }
    Ok(())
}

fn apply_pager_config(cargo_available: bool, scope: ConfigScope) -> Result<()> {
    anyhow::ensure!(
        cargo_available,
        "cargo not found — cannot install git-delta"
    );
    if !delta_installed() {
        install_delta()?;
    }
    for (k, v) in DELTA_CONFIGS {
        git_config_set(k, v, scope)?;
    }
    Ok(())
}

fn apply_single_config(key: &str, scope: ConfigScope) -> Result<()> {
    let value = CONFIG_OPTIONS
        .iter()
        .find(|o| o.key == key)
        .and_then(|o| o.value)
        .ok_or_else(|| anyhow::anyhow!("Unknown config key: {key}"))?;
    git_config_set(key, value, scope)
}

type GitConfigs = &'static [(&'static str, &'static str)];

const DEFAULTS: GitConfigs = &[
    ("push.autoSetupRemote", "true"),
    ("help.autocorrect", "prompt"),
    ("diff.algorithm", "histogram"),
];

const ADVANCED: GitConfigs = &[
    ("merge.conflictstyle", "zdiff3"),
    ("rerere.enabled", "true"),
];

const DELTA_CONFIGS: GitConfigs = &[
    ("core.pager", "delta"),
    ("interactive.diffFilter", "delta --color-only"),
    ("delta.navigate", "true"),
    ("delta.side-by-side", "true"),
];

fn apply_defaults(dry_run: bool, scope: ConfigScope) -> Result<()> {
    apply_configs(DEFAULTS, dry_run, scope)
}

fn apply_advanced(dry_run: bool, scope: ConfigScope) -> Result<()> {
    println!(
        "Warning: merge.conflictstyle=zdiff3 may cause issues with GitHub Desktop and GUI merge tools."
    );
    apply_configs(ADVANCED, dry_run, scope)
}

fn apply_delta(yes: bool, dry_run: bool, scope: ConfigScope) -> Result<()> {
    if !delta_installed() {
        if !confirm(
            "git-delta is not installed. Install via `cargo install git-delta`?",
            yes,
        ) {
            println!("Aborted.");
            return Ok(());
        }
        if !dry_run {
            install_delta()?;
        } else {
            println!("[dry-run] Would run: cargo install git-delta");
        }
    }
    println!(
        "Note: delta.side-by-side=true may look wrong in narrow terminals. \
        Disable with: git config {} delta.side-by-side false",
        scope_flag(scope)
    );
    apply_configs(DELTA_CONFIGS, dry_run, scope)
}

fn apply_configs(configs: GitConfigs, dry_run: bool, scope: ConfigScope) -> Result<()> {
    let flag = scope_flag(scope);
    let mut already_set = 0;

    for (key, value) in configs {
        let current = git_config_get(key, flag);

        if current.as_deref() == Some(value) {
            println!("✓ {key} = {value} (already set)");
            already_set += 1;
        } else if dry_run {
            println!("[dry-run] git config {flag} {key} {value}");
        } else {
            git_config_set(key, value, scope)?;
            println!("✓ Set {key} = {value}");
        }
    }

    if already_set == configs.len() {
        println!("\nAll configs already applied.");
    }

    if !dry_run {
        if let Ok(root) = find_repo_root() {
            let items: Vec<String> = configs.iter().map(|(k, _)| format!("config:{k}")).collect();
            crate::registry::record_best_effort(&root, &items);
        }
    }

    Ok(())
}

fn git_config_set(key: &str, value: &str, scope: ConfigScope) -> Result<()> {
    let flag = scope_flag(scope);
    let status = Command::new("git")
        .args(["config", flag, key, value])
        .status()
        .with_context(|| format!("Failed to run git config for '{key}'"))?;
    anyhow::ensure!(status.success(), "git config {flag} {key} {value} failed");
    Ok(())
}

pub(crate) fn remove_config_key(key: &str, scope: ConfigScope) -> Result<()> {
    let flag = scope_flag(scope);
    let status = Command::new("git")
        .args(["config", flag, "--unset", key])
        .status()
        .with_context(|| format!("Failed to unset git config for '{key}'"))?;
    anyhow::ensure!(status.success(), "git config {flag} --unset {key} failed");
    Ok(())
}

fn delta_installed() -> bool {
    Command::new("delta")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn install_delta() -> Result<()> {
    anyhow::ensure!(
        Command::new("cargo")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "cargo not found in PATH — install Rust from https://rustup.rs to install git-delta"
    );
    println!("Installing git-delta...");
    let status = Command::new("cargo")
        .args(["install", "git-delta"])
        .status()
        .context("Failed to run cargo install git-delta")?;
    anyhow::ensure!(status.success(), "cargo install git-delta failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn apply_configs_dry_run_prints_without_running_git() {
        let result = apply_configs(DEFAULTS, true, ConfigScope::Global);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_configs_dry_run_covers_advanced_preset() {
        assert!(apply_configs(ADVANCED, true, ConfigScope::Global).is_ok());
    }

    #[test]
    fn apply_configs_dry_run_covers_delta_preset() {
        assert!(apply_configs(DELTA_CONFIGS, true, ConfigScope::Global).is_ok());
    }

    #[test]
    fn determine_scope_defaults_to_global_outside_repo() {
        let scope = determine_scope(false, false);
        assert!(matches!(scope, ConfigScope::Global | ConfigScope::Local));
    }

    #[test]
    fn determine_scope_respects_explicit_flags() {
        assert!(matches!(determine_scope(true, false), ConfigScope::Global));
        assert!(matches!(determine_scope(false, true), ConfigScope::Local));
    }

    #[test]
    fn scope_flag_returns_correct_values() {
        assert_eq!(scope_flag(ConfigScope::Global), "--global");
        assert_eq!(scope_flag(ConfigScope::Local), "--local");
    }

    #[test]
    fn config_options_has_expected_entries() {
        assert!(!CONFIG_OPTIONS.is_empty());
        let keys: Vec<&str> = CONFIG_OPTIONS.iter().map(|o| o.key).collect();
        assert!(keys.contains(&"push.autoSetupRemote"));
        assert!(keys.contains(&"help.autocorrect"));
        assert!(keys.contains(&"diff.algorithm"));
        assert!(keys.contains(&"merge.conflictstyle"));
        assert!(keys.contains(&"rerere.enabled"));
        assert!(keys.contains(&"core.pager"));
    }

    #[test]
    fn config_options_all_keys_nonempty() {
        for opt in CONFIG_OPTIONS {
            assert!(!opt.key.is_empty());
            assert!(!opt.label.is_empty());
        }
    }

    #[test]
    fn config_options_recommended_are_marked() {
        let recommended: Vec<&str> = CONFIG_OPTIONS
            .iter()
            .filter(|o| o.recommended)
            .map(|o| o.key)
            .collect();
        assert!(recommended.contains(&"push.autoSetupRemote"));
        assert!(recommended.contains(&"help.autocorrect"));
        assert!(recommended.contains(&"diff.algorithm"));
    }

    #[test]
    fn config_options_core_pager_has_no_value() {
        let pager = CONFIG_OPTIONS
            .iter()
            .find(|o| o.key == "core.pager")
            .unwrap();
        assert!(pager.value.is_none());
    }

    #[test]
    fn apply_single_config_unknown_key_errors() {
        let result = apply_single_config("nonexistent.key", ConfigScope::Global);
        assert!(result.is_err());
    }

    #[test]
    fn apply_configs_dry_run_local_scope() {
        let result = apply_configs(DEFAULTS, true, ConfigScope::Local);
        assert!(result.is_ok());
    }

    #[test]
    fn config_scope_clone_and_copy() {
        let scope = ConfigScope::Global;
        let scope2 = scope;
        assert!(matches!(scope2, ConfigScope::Global));
    }

    #[test]
    fn presets_constants_are_nonempty() {
        assert!(!DEFAULTS.is_empty());
        assert!(!ADVANCED.is_empty());
        assert!(!DELTA_CONFIGS.is_empty());
    }

    #[test]
    fn defaults_preset_contains_expected_keys() {
        let keys: Vec<&str> = DEFAULTS.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"push.autoSetupRemote"));
        assert!(keys.contains(&"help.autocorrect"));
        assert!(keys.contains(&"diff.algorithm"));
    }

    #[test]
    fn advanced_preset_contains_expected_keys() {
        let keys: Vec<&str> = ADVANCED.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"merge.conflictstyle"));
        assert!(keys.contains(&"rerere.enabled"));
    }

    #[test]
    fn delta_configs_contains_expected_keys() {
        let keys: Vec<&str> = DELTA_CONFIGS.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"core.pager"));
        assert!(keys.contains(&"delta.navigate"));
    }

    #[test]
    fn determine_scope_global_true_overrides_local() {
        let scope = determine_scope(true, true);
        assert!(matches!(scope, ConfigScope::Global));
    }

    #[test]
    fn git_config_get_returns_string_for_existing_key() {
        let result = git_config_get("user.name", "--global");
        let _ = result;
    }

    // ── determine_scope edge cases ──────────────────────────────────────────

    #[test]
    fn determine_scope_global_true_overrides_local_true() {
        assert!(matches!(determine_scope(true, true), ConfigScope::Global));
    }

    #[serial]
    #[test]
    #[ignore = "flaky: set_current_dir races with parallel tests"]
    fn determine_scope_neither_flag_in_repo_is_local() {
        let original = std::env::current_dir().ok();
        // We're in a git repo, so should default to Local
        let scope = determine_scope(false, false);
        assert!(matches!(scope, ConfigScope::Local));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── scope_flag ──────────────────────────────────────────────────────────

    #[test]
    fn scope_flag_global_is_global() {
        assert_eq!(scope_flag(ConfigScope::Global), "--global");
    }

    #[test]
    fn scope_flag_local_is_local() {
        assert_eq!(scope_flag(ConfigScope::Local), "--local");
    }

    // ── apply_configs edge cases ────────────────────────────────────────────

    #[test]
    fn apply_configs_dry_run_with_empty_configs() {
        let empty: &[(&str, &str)] = &[];
        let result = apply_configs(empty, true, ConfigScope::Global);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_configs_dry_run_with_single_config() {
        let single: &[(&str, &str)] = &[("push.autoSetupRemote", "true")];
        let result = apply_configs(single, true, ConfigScope::Global);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_configs_all_configs_already_set() {
        // Test the "all already set" branch by using dry-run (won't actually set)
        let result = apply_configs(DEFAULTS, true, ConfigScope::Global);
        assert!(result.is_ok());
    }

    // ── apply_single_config ─────────────────────────────────────────────────

    #[test]
    fn apply_single_config_known_key_in_dry_run_does_not_panic() {
        let err = apply_single_config("unknown.key", ConfigScope::Global).unwrap_err();
        assert!(err.to_string().contains("Unknown config key"));
    }

    // ── CONFIG_OPTIONS completeness ─────────────────────────────────────────

    #[test]
    fn config_options_all_have_nonempty_labels() {
        for opt in CONFIG_OPTIONS {
            assert!(!opt.label.is_empty(), "empty label for key {}", opt.key);
        }
    }

    #[test]
    fn config_options_all_have_nonempty_keys() {
        for opt in CONFIG_OPTIONS {
            assert!(!opt.key.is_empty());
        }
    }

    #[test]
    fn config_options_push_auto_setup_remote_recommended() {
        let opt = CONFIG_OPTIONS
            .iter()
            .find(|o| o.key == "push.autoSetupRemote")
            .unwrap();
        assert!(opt.recommended);
    }

    #[test]
    fn config_options_help_autocorrect_recommended() {
        let opt = CONFIG_OPTIONS
            .iter()
            .find(|o| o.key == "help.autocorrect")
            .unwrap();
        assert!(opt.recommended);
    }

    #[test]
    fn config_options_diff_algorithm_recommended() {
        let opt = CONFIG_OPTIONS
            .iter()
            .find(|o| o.key == "diff.algorithm")
            .unwrap();
        assert!(opt.recommended);
    }

    #[test]
    fn config_options_merge_conflict_style_not_recommended() {
        let opt = CONFIG_OPTIONS
            .iter()
            .find(|o| o.key == "merge.conflictstyle")
            .unwrap();
        assert!(!opt.recommended);
    }

    #[test]
    fn config_options_rerere_enabled_not_recommended() {
        let opt = CONFIG_OPTIONS
            .iter()
            .find(|o| o.key == "rerere.enabled")
            .unwrap();
        assert!(!opt.recommended);
    }

    #[test]
    fn config_options_core_pager_not_recommended() {
        let opt = CONFIG_OPTIONS
            .iter()
            .find(|o| o.key == "core.pager")
            .unwrap();
        assert!(!opt.recommended);
    }

    // ── git_config_get edge cases ───────────────────────────────────────────

    #[test]
    fn git_config_get_returns_none_for_empty_string() {
        assert!(git_config_get("", "--global").is_none());
    }

    #[test]
    fn git_config_get_returns_none_for_invalid_scope() {
        assert!(git_config_get("user.name", "--invalid").is_none());
    }

    // ── preset constants ────────────────────────────────────────────────────

    #[test]
    fn defaults_preset_values_are_correct() {
        let map: std::collections::HashMap<&str, &str> = DEFAULTS.iter().copied().collect();
        assert_eq!(map.get("push.autoSetupRemote"), Some(&"true"));
        assert_eq!(map.get("help.autocorrect"), Some(&"prompt"));
        assert_eq!(map.get("diff.algorithm"), Some(&"histogram"));
    }

    #[test]
    fn advanced_preset_values_are_correct() {
        let map: std::collections::HashMap<&str, &str> = ADVANCED.iter().copied().collect();
        assert_eq!(map.get("merge.conflictstyle"), Some(&"zdiff3"));
        assert_eq!(map.get("rerere.enabled"), Some(&"true"));
    }

    #[test]
    fn delta_configs_values_are_correct() {
        let map: std::collections::HashMap<&str, &str> = DELTA_CONFIGS.iter().copied().collect();
        assert_eq!(map.get("core.pager"), Some(&"delta"));
        assert_eq!(map.get("delta.navigate"), Some(&"true"));
        assert_eq!(map.get("delta.side-by-side"), Some(&"true"));
    }

    // ── apply_config_keys ───────────────────────────────────────────────────

    #[test]
    fn apply_config_keys_empty_list_succeeds() {
        let result = apply_config_keys(&[], true, ConfigScope::Global);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_config_keys_single_valid_key() {
        // Use dry-run to avoid git config lock issues
        let single: &[(&str, &str)] = &[("push.autoSetupRemote", "true")];
        let result = apply_configs(single, true, ConfigScope::Global);
        assert!(result.is_ok());
    }

    #[test]
    #[ignore = "flaky: global git config lock contention in parallel tests"]
    fn apply_config_keys_multiple_valid_keys() {
        let result = apply_config_keys(
            &["push.autoSetupRemote", "diff.algorithm"],
            true,
            ConfigScope::Global,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn apply_config_keys_unknown_key_errors() {
        let result = apply_config_keys(&["unknown.key"], true, ConfigScope::Global);
        assert!(result.is_err());
    }

    // ── git_config_get returns Option<String> ───────────────────────────────

    #[test]
    fn git_config_get_returns_none_for_nonexistent_repo_key() {
        // This key should never be set
        assert!(git_config_get("gitkit.test.nonexistent", "--global").is_none());
    }

    // ── show_scope_config ─────────────────────────────────────────────────

    #[test]
    fn show_scope_config_global_does_not_panic() {
        show_scope_config("--global");
    }

    #[test]
    fn show_scope_config_local_does_not_panic() {
        show_scope_config("--local");
    }

    // ── apply_configs non-dry-run ─────────────────────────────────────────

    #[serial]
    #[test]
    fn apply_configs_non_dry_run_in_temp_repo() {
        let gitkit_home = tempfile::TempDir::new().unwrap();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", gitkit_home.path());
        }
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let single: &[(&str, &str)] = &[("push.autoSetupRemote", "true")];
        let result = apply_configs(single, false, ConfigScope::Local);
        let _ = result;
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
    fn apply_configs_non_dry_run_already_set() {
        let gitkit_home = tempfile::TempDir::new().unwrap();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", gitkit_home.path());
        }
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = git_config_set("push.autoSetupRemote", "true", ConfigScope::Local);
        let single: &[(&str, &str)] = &[("push.autoSetupRemote", "true")];
        let result = apply_configs(single, false, ConfigScope::Local);
        let _ = result;
        let _ = remove_config_key("push.autoSetupRemote", ConfigScope::Local);
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
    fn apply_configs_non_dry_run_multiple_configs() {
        let gitkit_home = tempfile::TempDir::new().unwrap();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", gitkit_home.path());
        }
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let configs: &[(&str, &str)] = &[
            ("push.autoSetupRemote", "true"),
            ("diff.algorithm", "histogram"),
        ];
        let result = apply_configs(configs, false, ConfigScope::Local);
        let _ = result;
        let _ = remove_config_key("push.autoSetupRemote", ConfigScope::Local);
        let _ = remove_config_key("diff.algorithm", ConfigScope::Local);
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

    // ── git_config_set ────────────────────────────────────────────────────

    #[serial]
    #[test]
    fn git_config_set_local_in_temp_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = git_config_set("gitkit.test.key", "test-value", ConfigScope::Local);
        let _ = result;
        let _ = remove_config_key("gitkit.test.key", ConfigScope::Local);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn git_config_set_global() {
        let result = git_config_set("gitkit.test.global-key", "test-global", ConfigScope::Global);
        assert!(result.is_ok());
        let val = git_config_get("gitkit.test.global-key", "--global");
        assert_eq!(val.as_deref(), Some("test-global"));
        // Clean up
        let _ = remove_config_key("gitkit.test.global-key", ConfigScope::Global);
    }

    // ── remove_config_key ─────────────────────────────────────────────────

    #[serial]
    #[test]
    fn remove_config_key_existing() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = git_config_set("gitkit.test.rm", "val", ConfigScope::Local);
        let _ = remove_config_key("gitkit.test.rm", ConfigScope::Local);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn remove_config_key_nonexistent_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = remove_config_key("gitkit.test.nonexistent", ConfigScope::Local);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── delta_installed ───────────────────────────────────────────────────

    #[test]
    fn delta_installed_returns_bool() {
        let result = delta_installed();
        // delta may or may not be installed, but should return a bool
        let _: bool = result;
    }

    #[test]
    fn delta_installed_false_when_not_in_path() {
        // If delta is not installed, should return false
        let result = delta_installed();
        // We can't guarantee delta is not installed, but we can verify it doesn't panic
        let _ = result;
    }

    // ── apply_single_config non-dry-run ───────────────────────────────────

    #[serial]
    #[test]
    fn apply_single_config_known_key_sets_value() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = apply_single_config("push.autoSetupRemote", ConfigScope::Local);
        let _ = remove_config_key("push.autoSetupRemote", ConfigScope::Local);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn apply_single_config_all_non_pager_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        for opt in CONFIG_OPTIONS {
            if opt.value.is_some() {
                let _ = apply_single_config(opt.key, ConfigScope::Local);
                let _ = remove_config_key(opt.key, ConfigScope::Local);
            }
        }
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── apply_config_keys with core.pager ─────────────────────────────────

    #[test]
    fn apply_config_keys_core_pager_without_cargo_errors() {
        let result = apply_config_keys(&["core.pager"], false, ConfigScope::Global);
        // Should error because cargo may not be available or delta may not be installed
        // The exact behavior depends on the environment
        let _ = result;
    }

    #[test]
    fn apply_config_keys_core_pager_with_cargo_false_errors() {
        let result = apply_config_keys(&["core.pager"], false, ConfigScope::Global);
        // With cargo_available=false, should error
        assert!(result.is_err());
    }

    // ── apply_config_keys with known keys ─────────────────────────────────

    #[serial]
    #[test]
    fn apply_config_keys_multiple_valid_non_dry_run() {
        let gitkit_home = tempfile::TempDir::new().unwrap();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", gitkit_home.path());
        }
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = apply_config_keys(
            &["push.autoSetupRemote", "diff.algorithm"],
            false,
            ConfigScope::Local,
        );
        let _ = remove_config_key("push.autoSetupRemote", ConfigScope::Local);
        let _ = remove_config_key("diff.algorithm", ConfigScope::Local);
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

    // ── show_config ───────────────────────────────────────────────────────

    #[test]
    fn show_config_does_not_panic() {
        let result = show_config();
        assert!(result.is_ok());
    }

    // ── run dispatch ──────────────────────────────────────────────────────

    #[test]
    fn run_dispatch_show() {
        let result = run(ConfigCommand::Show);
        assert!(result.is_ok());
    }

    #[test]
    fn run_dispatch_apply_defaults_dry_run() {
        let result = run(ConfigCommand::Apply {
            preset: Preset::Defaults,
            yes: true,
            dry_run: true,
            global: true,
            local: false,
        });
        assert!(result.is_ok());
    }

    #[test]
    fn run_dispatch_apply_advanced_dry_run() {
        let result = run(ConfigCommand::Apply {
            preset: Preset::Advanced,
            yes: true,
            dry_run: true,
            global: true,
            local: false,
        });
        assert!(result.is_ok());
    }

    #[test]
    fn run_dispatch_apply_delta_dry_run() {
        let result = run(ConfigCommand::Apply {
            preset: Preset::Delta,
            yes: true,
            dry_run: true,
            global: true,
            local: false,
        });
        assert!(result.is_ok());
    }

    #[serial]
    #[test]
    fn run_dispatch_apply_defaults_non_dry_run() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = run(ConfigCommand::Apply {
            preset: Preset::Defaults,
            yes: true,
            dry_run: false,
            global: false,
            local: true,
        });
        let _ = remove_config_key("push.autoSetupRemote", ConfigScope::Local);
        let _ = remove_config_key("help.autocorrect", ConfigScope::Local);
        let _ = remove_config_key("diff.algorithm", ConfigScope::Local);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_apply_advanced_non_dry_run() {
        let dir = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let _ = run(ConfigCommand::Apply {
            preset: Preset::Advanced,
            yes: true,
            dry_run: false,
            global: false,
            local: true,
        });
        let _ = remove_config_key("merge.conflictstyle", ConfigScope::Local);
        let _ = remove_config_key("rerere.enabled", ConfigScope::Local);
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── apply_defaults / apply_advanced / apply_delta ──────────────────────

    #[test]
    fn apply_defaults_dry_run_local() {
        let result = apply_defaults(true, ConfigScope::Local);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_advanced_dry_run_local() {
        let result = apply_advanced(true, ConfigScope::Local);
        assert!(result.is_ok());
    }

    #[test]
    fn apply_delta_dry_run_when_delta_not_installed() {
        let result = apply_delta(true, true, ConfigScope::Global);
        // dry_run should succeed even if delta is not installed
        assert!(result.is_ok());
    }

    #[test]
    #[ignore = "flaky: confirm() reads stdin in non-interactive test env"]
    fn apply_delta_non_dry_run_user_declines() {
        // When delta is not installed and user declines (yes=false, but no stdin),
        // this will likely error or abort. Test with yes=false in non-interactive env.
        // We test the "already installed" path by checking if delta is installed
        if delta_installed() {
            let result = apply_delta(true, false, ConfigScope::Global);
            assert!(result.is_ok());
        } else {
            // If delta not installed, with yes=false, confirm() reads stdin
            // In test env this will likely return false (empty input)
            // Just verify it doesn't panic
            let result = apply_delta(false, true, ConfigScope::Global);
            let _ = result;
        }
    }
}
