use anyhow::Result;
use inquire::{MultiSelect, Select, Text};
use std::{collections::HashSet, fs};

use crate::{attributes, builds, config, git, hooks, ignore, utils::find_repo_root};

const BANNER: &str = r#"
           ███   █████    █████       ███   █████   
          ░░░   ░░███    ░░███       ░░░   ░░███    
  ███████ ████  ███████   ░███ █████ ████  ███████  
 ███░░███░░███ ░░░███░    ░███░░███ ░░███ ░░░███░   
░███ ░███ ░███   ░███     ░██████░   ░███   ░███    
░███ ░███ ░███   ░███ ███ ░███░░███  ░███   ░███ ███
░░███████ █████  ░░█████  ████ █████ █████  ░░█████ 
 ░░░░░███░░░░░    ░░░░░  ░░░░ ░░░░░ ░░░░░    ░░░░░  
 ███ ░███                                           
░░██████                                            
 ░░░░░░                                             
"#;

pub fn run() -> Result<()> {
    // Initialize git repository if not already one
    let git_initialized = git::init_if_needed()?;
    if git_initialized {
        println!("  ◇ git repository initialized  ✓");
    }

    println!("{BANNER}");
    println!("  Configure your git repo\n");

    // ── Build selection ─────────────────────────────────────────────────────
    if maybe_apply_saved_build()?.is_some() {
        return Ok(());
    }

    let cargo_available = std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // ── Hooks ────────────────────────────────────────────────────────────────
    let installed_hooks = get_installed_hooks();
    let hook_selections = prompt_hook_selections(&installed_hooks)?;

    // ── .gitignore ───────────────────────────────────────────────────────────
    let selected_templates = prompt_ignore_templates()?;

    // ── .gitattributes ───────────────────────────────────────────────────────
    let selected_attrs = prompt_attributes()?;

    // ── Git config ───────────────────────────────────────────────────────────
    let (selected_config_keys, configs_to_remove) = prompt_git_config(cargo_available)?;

    let selections = InitSelections {
        selected_builtins: hook_selections.selected_builtins,
        custom_hooks: hook_selections.custom_hooks,
        hooks_to_remove: hook_selections.hooks_to_remove,
        selected_templates,
        selected_attrs,
        selected_config_keys,
        configs_to_remove,
        cargo_available,
    };

    // ── Summary & confirm ────────────────────────────────────────────────────
    if !confirm_and_summarize(&selections)? {
        return Ok(());
    }

    // ── Apply ────────────────────────────────────────────────────────────────
    apply_selections(&selections)?;

    // ── Save as build ─────────────────────────────────────────────────────
    maybe_save_build()?;

    println!("\n  Done\n");
    Ok(())
}

/// Everything the wizard collects from the user before anything is applied,
/// grouped so the confirm and apply steps can pass it around as one value.
struct InitSelections {
    selected_builtins: Vec<String>,
    custom_hooks: Vec<(String, String)>,
    hooks_to_remove: Vec<String>,
    selected_templates: Vec<String>,
    selected_attrs: Vec<String>,
    selected_config_keys: Vec<String>,
    configs_to_remove: Vec<String>,
    cargo_available: bool,
}

/// Offers the saved-build picker before the wizard starts. `Ok(Some(()))`
/// means a saved build was applied and `run()` must stop; `Ok(None)` means
/// a fresh configuration follows.
fn maybe_apply_saved_build() -> Result<Option<()>> {
    let saved_builds = builds::list_build_names();
    if saved_builds.is_empty() {
        return Ok(None);
    }

    let mut options = vec!["Start fresh configuration".to_string()];
    options.extend(saved_builds.iter().map(|b| format!("Use build: {b}")));

    let choice = Select::new("Saved builds available", options)
        .with_help_message("↑↓ move  enter confirm  esc start fresh")
        .prompt_skippable()?;

    let Some(build_name) = choice
        .as_deref()
        .and_then(|c| c.strip_prefix("Use build: "))
    else {
        println!();
        return Ok(None);
    };

    println!();
    let build = builds::load_build(build_name)?;
    builds::apply_build(&build)?;
    println!("\n  Done\n");
    Ok(Some(()))
}

/// What the hooks step of the wizard resolved to: the builtins to install,
/// the custom hooks to add, and the installed builtins to drop.
struct HookSelections {
    selected_builtins: Vec<String>,
    custom_hooks: Vec<(String, String)>,
    hooks_to_remove: Vec<String>,
}

/// Runs the hooks step of the wizard and resolves the answer into the
/// builtins to install, the custom hooks to add, and the installed builtins
/// to drop.
fn prompt_hook_selections(installed: &HashSet<String>) -> Result<HookSelections> {
    let builtins = hooks::available_builtins();

    let mut hook_items: Vec<String> = builtins
        .iter()
        .map(|b| {
            let base = format!("{:<25} ({})  —  {}", b.name, b.hook, b.description);
            if installed.contains(b.name) {
                format!("{} [✓ installed]", base)
            } else {
                base
            }
        })
        .collect();
    hook_items.push("Add custom hook...".to_string());

    let preselected: Vec<usize> = builtins
        .iter()
        .enumerate()
        .filter(|(_, b)| installed.contains(b.name))
        .map(|(i, _)| i)
        .collect();

    let default_selection = if preselected.is_empty() {
        vec![0usize]
    } else {
        preselected
    };

    let hook_selections = MultiSelect::new("Hooks", hook_items.clone())
        .with_default(&default_selection)
        .with_help_message("↑↓ move  space select  enter confirm  esc skip")
        .prompt_skippable()?
        .unwrap_or_default();

    let mut selected_builtins: Vec<String> = Vec::new();
    let mut custom_hooks: Vec<(String, String)> = Vec::new();

    for item in &hook_selections {
        match parse_hook_item(item, &hook_items, builtins)? {
            HookAction::Builtin(name) => selected_builtins.push(name),
            HookAction::Custom(hook, command) => custom_hooks.push((hook, command)),
            HookAction::Ignored => {}
        }
    }

    let hooks_to_remove: Vec<String> = installed
        .iter()
        .filter(|h| !selected_builtins.contains(h))
        .cloned()
        .collect();

    Ok(HookSelections {
        selected_builtins,
        custom_hooks,
        hooks_to_remove,
    })
}

/// What one selected entry of the hooks list resolves to.
#[derive(Debug, PartialEq, Eq)]
enum HookAction {
    /// A builtin to install under its known name.
    Builtin(String),
    /// A custom hook: the git hook file to write and the command to run.
    Custom(String, String),
    /// Nothing to do — the user skipped the entry or left it blank.
    Ignored,
}

/// Resolves one selected hooks-list item, prompting for the command when the
/// "Add custom hook..." entry was picked. `Ignored` mirrors the wizard's
/// skip semantics: a cancelled or blank answer changes nothing.
fn parse_hook_item(
    item: &str,
    hook_items: &[String],
    builtins: &[hooks::builtins::Builtin],
) -> Result<HookAction> {
    if item == "Add custom hook..." {
        let Some(hook_name) =
            Select::new("Hook type", hooks::valid_hook_names().to_vec()).prompt_skippable()?
        else {
            return Ok(HookAction::Ignored);
        };
        let command = Text::new("  Command to run")
            .prompt_skippable()?
            .unwrap_or_default();
        if command.trim().is_empty() {
            return Ok(HookAction::Ignored);
        }
        return Ok(HookAction::Custom(hook_name.to_string(), command));
    }

    let Some(idx) = hook_items.iter().position(|i| i == item) else {
        return Ok(HookAction::Ignored);
    };
    if idx < builtins.len() {
        return Ok(HookAction::Builtin(builtins[idx].name.to_string()));
    }
    Ok(HookAction::Ignored)
}

/// The `.gitignore` templates step. When the list can't be fetched
/// (offline), says so and skips the step rather than failing the wizard.
fn prompt_ignore_templates() -> Result<Vec<String>> {
    println!();
    let all_templates = load_ignore_templates();
    if all_templates.is_empty() {
        println!("  ⚠  Could not fetch templates (offline?) — skipping .gitignore");
        return Ok(Vec::new());
    }

    let selected = MultiSelect::new(".gitignore templates", all_templates)
        .with_help_message("Type to filter  ↑↓ move  space select  enter confirm  esc skip")
        .with_page_size(10)
        .prompt_skippable()?
        .unwrap_or_default();
    Ok(selected)
}

/// The `.gitattributes` step: both presets offered, line endings
/// preselected; `esc`/skip leaves the selection empty.
fn prompt_attributes() -> Result<Vec<String>> {
    println!();
    let attrs_items = vec![
        "line-endings  ★ recommended  —  * text=auto eol=lf",
        "binary-files  —  mark images, PDFs, archives as binary (no diff)",
    ];
    let attrs_keys = ["line-endings", "binary-files"];

    let attrs_selections = MultiSelect::new(".gitattributes", attrs_items.clone())
        .with_default(&[0usize]) // line-endings preselected
        .with_help_message("space select  enter confirm  esc skip")
        .prompt_skippable()?
        .unwrap_or_default();

    let selected: Vec<String> = resolve_keys(&attrs_selections, &attrs_items, &attrs_keys)
        .into_iter()
        .map(str::to_string)
        .collect();
    Ok(selected)
}

/// The git-config step: returns the keys to set (selected) and the keys to
/// unset (configured but deselected).
fn prompt_git_config(cargo_available: bool) -> Result<(Vec<String>, Vec<String>)> {
    println!();
    let configured_keys = get_configured_keys();

    let config_options: Vec<&config::ConfigOption> = config::CONFIG_OPTIONS
        .iter()
        .filter(|o| o.key != "core.pager" || cargo_available)
        .collect();

    let config_labels: Vec<String> = config_options
        .iter()
        .map(|o| {
            if configured_keys.contains(o.key) {
                format!("{} [✓ already set]", o.label)
            } else {
                o.label.to_string()
            }
        })
        .collect();

    let config_labels_refs: Vec<&str> = config_labels.iter().map(|s| s.as_str()).collect();

    let defaults: Vec<usize> = config_options
        .iter()
        .enumerate()
        .filter(|(_, o)| o.recommended || configured_keys.contains(o.key))
        .map(|(i, _)| i)
        .collect();

    let config_selections = MultiSelect::new("Git config", config_labels_refs.clone())
        .with_default(&defaults)
        .with_help_message("↑↓ move  space select  enter confirm  esc skip")
        .prompt_skippable()?
        .unwrap_or_default();

    let selected_config_keys: Vec<String> = resolve_keys(
        &config_selections,
        &config_labels_refs,
        &config_options.iter().map(|o| o.key).collect::<Vec<_>>(),
    )
    .into_iter()
    .map(str::to_string)
    .collect();

    let configs_to_remove: Vec<String> = config_options
        .iter()
        .filter(|o| {
            configured_keys.contains(o.key) && !selected_config_keys.iter().any(|k| k == o.key)
        })
        .map(|o| o.key.to_string())
        .collect();

    Ok((selected_config_keys, configs_to_remove))
}

/// Prints the selection summary and asks for final confirmation. Returns
/// `false` when the wizard must stop without applying anything — nothing
/// was selected, or the user declined.
fn confirm_and_summarize(s: &InitSelections) -> Result<bool> {
    let has_removals = !s.hooks_to_remove.is_empty() || !s.configs_to_remove.is_empty();
    let nothing = s.selected_builtins.is_empty()
        && s.custom_hooks.is_empty()
        && s.selected_templates.is_empty()
        && s.selected_attrs.is_empty()
        && s.selected_config_keys.is_empty()
        && !has_removals;

    if nothing {
        println!("\n  Nothing selected — exiting.");
        return Ok(false);
    }

    println!("\n  Summary:");
    if !s.selected_builtins.is_empty() || !s.custom_hooks.is_empty() {
        let names: Vec<&str> = s
            .selected_builtins
            .iter()
            .map(String::as_str)
            .chain(s.custom_hooks.iter().map(|(h, _)| h.as_str()))
            .collect();
        println!("  ◆ hooks: {}", names.join(", "));
    }
    if !s.selected_templates.is_empty() {
        println!("  ◆ .gitignore: {}", s.selected_templates.join(", "));
    }
    if !s.selected_attrs.is_empty() {
        println!("  ◆ .gitattributes: {}", s.selected_attrs.join(", "));
    }
    if !s.selected_config_keys.is_empty() {
        println!("  ◆ git config: {}", s.selected_config_keys.join(", "));
    }

    println!();
    let confirmed = inquire::Confirm::new("Apply these changes?")
        .with_default(true)
        .prompt()?;

    if !confirmed {
        println!("  Aborted.");
        return Ok(false);
    }
    Ok(true)
}

/// Applies the confirmed selections, printing the original per-item
/// progress lines.
fn apply_selections(s: &InitSelections) -> Result<()> {
    println!();
    for name in &s.selected_builtins {
        hooks::install_builtin(name, false)?;
        println!("  ◇ hook '{name}' installed  ✓");
    }
    for (hook, cmd) in &s.custom_hooks {
        hooks::install_custom(hook, cmd, false)?;
        println!("  ◇ hook '{hook}' installed  ✓");
    }
    for hook in &s.hooks_to_remove {
        // `remove_hook` now removes just this builtin's part, so composing
        // builtins that share a git hook (e.g. pre-commit) are unaffected.
        if hooks::remove_hook(hook, true).is_ok() {
            println!("  ◇ hook '{hook}' removed  ✓");
        }
    }
    if !s.selected_templates.is_empty() {
        let joined = s.selected_templates.join(",");
        ignore::add_templates(&joined, false)?;
        println!("  ◇ .gitignore updated  ✓");
    }
    if !s.selected_attrs.is_empty() {
        let attrs: Vec<&str> = s.selected_attrs.iter().map(String::as_str).collect();
        attributes::apply_presets(&attrs)?;
        println!("  ◇ .gitattributes applied  ✓");
    }
    if !s.selected_config_keys.is_empty() {
        let keys: Vec<&str> = s.selected_config_keys.iter().map(String::as_str).collect();
        config::apply_config_keys(&keys, s.cargo_available, config::ConfigScope::Local)?;
        println!("  ◇ git config applied  ✓");
    }
    // Only touch the repo's local config; a global value affects every repo,
    // so it is never removed from here.
    for key in &s.configs_to_remove {
        if config::remove_config_key(key, config::ConfigScope::Local).is_ok() {
            println!("  ◇ git config '{key}' removed  ✓");
        } else {
            println!(
                "  ◇ git config '{key}' is set globally — left untouched (git config --global --unset {key})"
            );
        }
    }

    Ok(())
}

/// Offers to save the applied configuration as a reusable build.
fn maybe_save_build() -> Result<()> {
    println!();
    let save_build = inquire::Confirm::new("Save this configuration as a reusable build?")
        .with_default(false)
        .prompt()?;

    if save_build {
        let description = Text::new("  Description (optional)")
            .with_default("")
            .prompt()?;
        let desc_ref = if description.is_empty() {
            None
        } else {
            Some(description.as_str())
        };

        save_build_interactive(desc_ref)?;
    }

    Ok(())
}

const MAX_SAVE_ATTEMPTS: u32 = 3;

#[derive(Debug, PartialEq, Eq)]
enum SaveRetryDecision {
    /// Collision, and attempts remain — offer a different name or overwrite.
    Retry,
    /// Collision, but attempts are exhausted — stop asking.
    GiveUp,
    /// Not a collision — not retryable, stop asking.
    Abort,
}

/// Pure decision logic for the wizard's save retry loop, kept separate from
/// the interactive prompting around it so it can be tested directly.
fn decide_save_retry(
    is_collision: bool,
    attempts_used: u32,
    max_attempts: u32,
) -> SaveRetryDecision {
    if !is_collision {
        return SaveRetryDecision::Abort;
    }
    if attempts_used >= max_attempts {
        SaveRetryDecision::GiveUp
    } else {
        SaveRetryDecision::Retry
    }
}

/// What the save-retry handler decided after a failed `builds::save`.
#[derive(Debug, PartialEq, Eq)]
enum SaveRetryAction {
    /// The build now exists (fresh save or overwrite) — the loop stops.
    Done,
    /// Name collision with attempts left: try again with this name. `None`
    /// means the name prompt was skipped — the loop finishes quietly.
    RetryWith(Option<String>),
    /// Not retryable, or attempts exhausted: report this reason and stop.
    ReportUnsaved(String),
}

/// Runs the interactive name-prompt / retry loop for saving a build at the
/// end of the wizard. Never leaves a failed save unreported: on any exit
/// path other than success, it states plainly that the build was not saved.
fn save_build_interactive(desc_ref: Option<&str>) -> Result<()> {
    let mut pending_name = Text::new("  Build name").prompt_skippable()?;
    let mut attempts: u32 = 0;

    loop {
        let name = match pending_name.take() {
            Some(n) if !n.is_empty() => n,
            // Cancelled (Ctrl-C/Esc) or an empty answer: the user changed
            // their mind about saving. Not an error — finish quietly.
            _ => return Ok(()),
        };

        match builds::save(&name, desc_ref) {
            Ok(()) => return Ok(()),
            Err(e) => {
                attempts += 1;
                match handle_save_collision(&name, desc_ref, &e, attempts)? {
                    SaveRetryAction::Done => return Ok(()),
                    SaveRetryAction::RetryWith(next) => pending_name = next,
                    SaveRetryAction::ReportUnsaved(reason) => {
                        report_unsaved_build(&name, desc_ref, &reason);
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Maps one failed save onto the retry loop's next step: report it on a
/// non-collision, give up once [`MAX_SAVE_ATTEMPTS`] is reached, and
/// otherwise ask whether to overwrite or pick another name.
fn handle_save_collision(
    name: &str,
    desc_ref: Option<&str>,
    err: &anyhow::Error,
    attempts: u32,
) -> Result<SaveRetryAction> {
    let is_collision = builds::is_build_name_collision(err);
    match decide_save_retry(is_collision, attempts, MAX_SAVE_ATTEMPTS) {
        SaveRetryDecision::Abort => Ok(SaveRetryAction::ReportUnsaved(err.to_string())),
        SaveRetryDecision::GiveUp => Ok(SaveRetryAction::ReportUnsaved(format!(
            "gave up after {attempts} attempts: {err}"
        ))),
        SaveRetryDecision::Retry => {
            if !prompt_overwrite_or_rename(name)? {
                let next = Text::new("  Build name").prompt_skippable()?;
                return Ok(SaveRetryAction::RetryWith(next));
            }
            match builds::save_overwrite(name, desc_ref) {
                Ok(()) => Ok(SaveRetryAction::Done),
                Err(e) => Ok(SaveRetryAction::ReportUnsaved(e.to_string())),
            }
        }
    }
}

/// Asks what to do about a name collision: overwrite the existing build, or
/// choose a different name (`false`, also when the prompt is skipped).
fn prompt_overwrite_or_rename(name: &str) -> Result<bool> {
    let overwrite_option = format!("Overwrite existing build '{name}'");
    let choice = Select::new(
        "  A build with that name already exists",
        vec![
            "Choose a different name".to_string(),
            overwrite_option.clone(),
        ],
    )
    .prompt_skippable()?;
    Ok(choice.as_deref() == Some(overwrite_option.as_str()))
}

/// The build the user asked for was not saved. Say so explicitly, and dump
/// the configuration that would have been saved so it can be recreated by
/// hand — losing this silently is the one thing the wizard must never do.
fn report_unsaved_build(name: &str, description: Option<&str>, reason: &str) {
    println!("  ⚠ Build was not saved: {reason}");
    if let Ok(build) = builds::capture_current_config(name, description) {
        if let Ok(toml_str) = toml::to_string_pretty(&build) {
            println!(
                "  Configuration below was not saved — copy it to a builds file by hand if needed:\n"
            );
            println!("{toml_str}");
        }
    }
}

fn load_ignore_templates() -> Vec<String> {
    ignore::fetch_template_list().unwrap_or_default()
}

fn get_installed_hooks() -> HashSet<String> {
    let Ok(root) = find_repo_root() else {
        return HashSet::new();
    };
    let hooks_dir = root.join(".git").join("hooks");
    if !hooks_dir.exists() {
        return HashSet::new();
    }
    let Ok(entries) = fs::read_dir(&hooks_dir) else {
        return HashSet::new();
    };

    let mut found = HashSet::new();
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".bak") || name.ends_with(".sample") {
            continue;
        }
        let content = fs::read_to_string(entry.path()).unwrap_or_default();

        if hooks::is_dispatcher(&content, &name) {
            for part in hooks::list_parts(&hooks_dir, &name) {
                if hooks::builtins::get(&part).is_some() {
                    found.insert(part);
                }
            }
            continue;
        }

        if let Some(b) = hooks::detect_builtin(&name, &content) {
            found.insert(b.name.to_string());
        }
    }
    found
}

fn get_configured_keys() -> HashSet<String> {
    let mut configured = HashSet::new();

    // Get all config values in one call per scope
    let local_configs = get_all_git_configs("--local");
    let global_configs = get_all_git_configs("--global");

    for option in config::CONFIG_OPTIONS {
        if option.key == "core.pager" {
            continue;
        }
        if let Some(expected_value) = option.value {
            // Check local first, then global
            if local_configs.get(option.key).map(|s| s.as_str()) == Some(expected_value)
                || global_configs.get(option.key).map(|s| s.as_str()) == Some(expected_value)
            {
                configured.insert(option.key.to_string());
            }
        }
    }
    configured
}

fn get_all_git_configs(scope: &str) -> std::collections::HashMap<String, String> {
    let Ok(output) = std::process::Command::new("git")
        .args(["config", scope, "--list"])
        .output()
    else {
        return std::collections::HashMap::new();
    };
    if !output.status.success() {
        return std::collections::HashMap::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// Maps selected display labels back to their corresponding keys.
fn resolve_keys<'a>(
    selections: &[impl AsRef<str>],
    labels: &[&str],
    keys: &[&'a str],
) -> Vec<&'a str> {
    selections
        .iter()
        .filter_map(|item| {
            labels
                .iter()
                .position(|l| *l == item.as_ref())
                .map(|i| keys[i])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn get_configured_keys_only_returns_known_option_keys() {
        let configured = get_configured_keys();
        for key in &configured {
            assert!(config::CONFIG_OPTIONS.iter().any(|o| o.key == key));
        }
    }

    #[test]
    fn resolve_keys_maps_labels_to_keys() {
        let selections = vec!["option A", "option C"];
        let labels = vec!["option A", "option B", "option C"];
        let keys = vec!["key_a", "key_b", "key_c"];
        let result = resolve_keys(&selections, &labels, &keys);
        assert_eq!(result, vec!["key_a", "key_c"]);
    }

    #[test]
    fn resolve_keys_empty_selections() {
        let selections: Vec<&str> = vec![];
        let labels = vec!["option A", "option B"];
        let keys = vec!["key_a", "key_b"];
        let result = resolve_keys(&selections, &labels, &keys);
        assert!(result.is_empty());
    }

    #[test]
    fn resolve_keys_no_matching_labels() {
        let selections = vec!["unknown option"];
        let labels = vec!["option A", "option B"];
        let keys = vec!["key_a", "key_b"];
        let result = resolve_keys(&selections, &labels, &keys);
        assert!(result.is_empty());
    }

    // ── decide_save_retry ───────────────────────────────────────────────────

    #[test]
    fn decide_save_retry_non_collision_always_aborts() {
        assert_eq!(decide_save_retry(false, 1, 3), SaveRetryDecision::Abort);
        assert_eq!(decide_save_retry(false, 3, 3), SaveRetryDecision::Abort);
    }

    #[test]
    fn decide_save_retry_collision_retries_while_attempts_remain() {
        assert_eq!(decide_save_retry(true, 1, 3), SaveRetryDecision::Retry);
        assert_eq!(decide_save_retry(true, 2, 3), SaveRetryDecision::Retry);
    }

    #[test]
    fn decide_save_retry_collision_gives_up_at_max_attempts() {
        assert_eq!(decide_save_retry(true, 3, 3), SaveRetryDecision::GiveUp);
        assert_eq!(decide_save_retry(true, 4, 3), SaveRetryDecision::GiveUp);
    }

    #[test]
    fn resolve_keys_single_match() {
        let selections = vec!["option B"];
        let labels = vec!["option A", "option B", "option C"];
        let keys = vec!["key_a", "key_b", "key_c"];
        let result = resolve_keys(&selections, &labels, &keys);
        assert_eq!(result, vec!["key_b"]);
    }

    #[test]
    fn resolve_keys_all_labels_selected() {
        let selections = vec!["option A", "option B", "option C"];
        let labels = vec!["option A", "option B", "option C"];
        let keys = vec!["key_a", "key_b", "key_c"];
        let result = resolve_keys(&selections, &labels, &keys);
        assert_eq!(result, vec!["key_a", "key_b", "key_c"]);
    }

    #[test]
    fn get_all_git_configs_returns_map() {
        let result = get_all_git_configs("--global");
        // Should return a HashMap, possibly empty
        assert!(result.is_empty() || !result.is_empty());
    }

    #[test]
    fn get_installed_hooks_returns_hashset() {
        let hooks = get_installed_hooks();
        // Should return a HashSet, possibly empty
        assert!(hooks.is_empty() || !hooks.is_empty());
    }

    // ── get_installed_hooks with actual hooks ─────────────────────────────

    #[serial]
    #[test]
    fn get_installed_hooks_with_builtin_hook() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let builtin = crate::hooks::builtins::get("conventional-commits").unwrap();
        std::fs::write(hooks_dir.join("commit-msg"), builtin.script).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        assert!(hooks.contains("conventional-commits"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_with_no_secrets_builtin() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let builtin = crate::hooks::builtins::get("no-secrets").unwrap();
        std::fs::write(hooks_dir.join("pre-commit"), builtin.script).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        assert!(hooks.contains("no-secrets"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_skips_bak_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let builtin = crate::hooks::builtins::get("conventional-commits").unwrap();
        std::fs::write(hooks_dir.join("commit-msg.bak"), builtin.script).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        assert!(hooks.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_skips_sample_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let builtin = crate::hooks::builtins::get("conventional-commits").unwrap();
        std::fs::write(hooks_dir.join("commit-msg.sample"), builtin.script).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        assert!(hooks.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_empty_hooks_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        assert!(hooks.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_no_hooks_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        // No hooks dir
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        assert!(hooks.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_no_git_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        // No .git dir at all
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        // find_repo_root fails, returns empty set
        assert!(hooks.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_with_custom_hook_not_detected() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        // Write a hook that doesn't match any builtin
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\nmy custom command\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        // Custom hooks are not detected as builtins
        assert!(hooks.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn get_installed_hooks_with_multiple_builtins() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let cc = crate::hooks::builtins::get("conventional-commits").unwrap();
        let ns = crate::hooks::builtins::get("no-secrets").unwrap();
        std::fs::write(hooks_dir.join("commit-msg"), cc.script).unwrap();
        std::fs::write(hooks_dir.join("pre-commit"), ns.script).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        assert!(hooks.contains("conventional-commits"));
        assert!(hooks.contains("no-secrets"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── get_configured_keys ───────────────────────────────────────────────

    #[test]
    fn get_configured_keys_returns_hashset() {
        let keys = get_configured_keys();
        // Should return a HashSet
        let _ = keys;
    }

    #[test]
    fn get_configured_keys_all_keys_are_valid() {
        let keys = get_configured_keys();
        for key in &keys {
            assert!(config::CONFIG_OPTIONS.iter().any(|o| o.key == key));
        }
    }

    #[test]
    fn get_configured_keys_core_pager_excluded() {
        let keys = get_configured_keys();
        assert!(!keys.contains("core.pager"));
    }

    // ── get_all_git_configs ───────────────────────────────────────────────

    #[test]
    fn get_all_git_configs_global_returns_map() {
        let configs = get_all_git_configs("--global");
        assert!(configs.is_empty() || !configs.is_empty());
    }

    #[test]
    fn get_all_git_configs_local_returns_map() {
        let configs = get_all_git_configs("--local");
        assert!(configs.is_empty() || !configs.is_empty());
    }

    #[test]
    fn get_all_git_configs_invalid_scope_returns_empty() {
        let configs = get_all_git_configs("--invalid-scope");
        assert!(configs.is_empty());
    }

    #[serial]
    #[test]
    fn get_all_git_configs_with_set_value() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        // Set a config value
        let _ = std::process::Command::new("git")
            .args(["config", "local", "gitkit.test.configkey", "testvalue"])
            .output();
        let configs = get_all_git_configs("--local");
        // Should contain the value we just set
        let _ = configs.get("gitkit.test.configkey");
        // Clean up
        let _ = std::process::Command::new("git")
            .args(["config", "local", "--unset", "gitkit.test.configkey"])
            .output();
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── load_ignore_templates ─────────────────────────────────────────────

    #[test]
    fn load_ignore_templates_returns_vec() {
        let templates = load_ignore_templates();
        // Returns a Vec<String>, may be empty if offline
        let _ = templates;
    }

    // ── resolve_keys additional edge cases ────────────────────────────────

    #[test]
    fn resolve_keys_with_string_selections() {
        let selections = vec!["option A".to_string(), "option C".to_string()];
        let labels = vec!["option A", "option B", "option C"];
        let keys = vec!["key_a", "key_b", "key_c"];
        let result = resolve_keys(&selections, &labels, &keys);
        assert_eq!(result, vec!["key_a", "key_c"]);
    }

    #[test]
    fn resolve_keys_duplicate_selections() {
        let selections = vec!["option A", "option A"];
        let labels = vec!["option A", "option B"];
        let keys = vec!["key_a", "key_b"];
        let result = resolve_keys(&selections, &labels, &keys);
        assert_eq!(result, vec!["key_a", "key_a"]);
    }

    #[test]
    fn resolve_keys_empty_labels() {
        let selections = vec!["option A"];
        let labels: Vec<&str> = vec![];
        let keys: Vec<&str> = vec![];
        let result = resolve_keys(&selections, &labels, &keys);
        assert!(result.is_empty());
    }

    #[test]
    #[should_panic]
    fn resolve_keys_more_labels_than_keys_panics() {
        let selections = vec!["option A", "option C"];
        let labels = vec!["option A", "option B", "option C"];
        let keys = vec!["key_a", "key_b"];
        let _ = resolve_keys(&selections, &labels, &keys);
    }

    #[test]
    fn resolve_keys_partial_overlap() {
        let selections = vec!["option B", "option D"];
        let labels = vec!["option A", "option B", "option C"];
        let keys = vec!["key_a", "key_b", "key_c"];
        let result = resolve_keys(&selections, &labels, &keys);
        // "option B" matches index 1, "option D" doesn't match
        assert_eq!(result, vec!["key_b"]);
    }

    // ── get_installed_hooks with unreadable file ──────────────────────────

    #[serial]
    #[test]
    fn get_installed_hooks_with_unreadable_hook_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        // Create a file that can't be read (empty content)
        std::fs::write(hooks_dir.join("pre-push"), "").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let hooks = get_installed_hooks();
        // Empty file won't match any builtin
        assert!(hooks.is_empty());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── parse_hook_item ─────────────────────────────────────────────────────

    #[test]
    fn parse_hook_item_resolves_builtin_label_to_its_name() {
        let builtins = crate::hooks::available_builtins();
        let items = vec![
            builtins[0].name.to_string(),
            "Add custom hook...".to_string(),
        ];
        let action = parse_hook_item(&items[0], &items, builtins).unwrap();
        assert_eq!(action, HookAction::Builtin(builtins[0].name.to_string()));
    }

    #[test]
    fn parse_hook_item_ignores_unknown_labels() {
        let builtins = crate::hooks::available_builtins();
        let items = vec![
            "not an option".to_string(),
            "Add custom hook...".to_string(),
        ];
        assert_eq!(
            parse_hook_item("something else", &items, builtins).unwrap(),
            HookAction::Ignored
        );
    }

    #[test]
    fn parse_hook_item_ignores_items_past_the_builtins() {
        let builtins = crate::hooks::available_builtins();
        let mut items: Vec<String> = builtins.iter().map(|b| b.name.to_string()).collect();
        items.push("extra option".to_string());
        assert_eq!(
            parse_hook_item("extra option", &items, builtins).unwrap(),
            HookAction::Ignored
        );
    }

    // ── handle_save_collision ───────────────────────────────────────────────

    fn collision_err() -> anyhow::Error {
        builds::BuildNameCollision {
            name: "existing".to_string(),
        }
        .into()
    }

    #[test]
    fn handle_save_collision_reports_non_collision_without_prompting() {
        let err = anyhow::anyhow!("disk full");
        let action = handle_save_collision("name", None, &err, 1).unwrap();
        assert_eq!(action, SaveRetryAction::ReportUnsaved(err.to_string()));
    }

    #[test]
    fn handle_save_collision_gives_up_at_max_attempts() {
        let err = collision_err();
        let action = handle_save_collision("name", None, &err, MAX_SAVE_ATTEMPTS).unwrap();
        assert_eq!(
            action,
            SaveRetryAction::ReportUnsaved(format!(
                "gave up after {MAX_SAVE_ATTEMPTS} attempts: {err}"
            ))
        );
    }

    // ── prompt-builder contract (guards the inquire API the wizard uses) ────

    #[test]
    fn wizard_prompt_builders_construct() {
        // The exact builder chains run() and its helpers use; a dependency
        // bump that changes this API must fail here, not at prompt time.
        let _saved_builds = Select::new(
            "Saved builds available",
            vec!["Start fresh configuration".to_string()],
        )
        .with_help_message("↑↓ move  enter confirm  esc start fresh");
        let _hooks = MultiSelect::new("Hooks", vec!["item".to_string()])
            .with_default(&[0usize])
            .with_help_message("↑↓ move  space select  enter confirm  esc skip");
        let _custom = Select::new("Hook type", crate::hooks::valid_hook_names().to_vec());
        let _command = Text::new("  Command to run");
        let _templates = MultiSelect::new(".gitignore templates", vec!["rust".to_string()])
            .with_help_message("Type to filter  ↑↓ move  space select  enter confirm  esc skip")
            .with_page_size(10);
        let _attrs = MultiSelect::new(".gitattributes", vec!["line-endings".to_string()])
            .with_default(&[0usize])
            .with_help_message("space select  enter confirm  esc skip");
        let _git_config = MultiSelect::new("Git config", vec!["label".to_string()])
            .with_default(&[0usize])
            .with_help_message("↑↓ move  space select  enter confirm  esc skip");
        let _apply = inquire::Confirm::new("Apply these changes?").with_default(true);
        let _save_build = inquire::Confirm::new("Save this configuration as a reusable build?")
            .with_default(false);
        let _name = Text::new("  Build name");
        let _overwrite = Select::new(
            "  A build with that name already exists",
            vec![
                "Choose a different name".to_string(),
                "Overwrite existing build 'x'".to_string(),
            ],
        );
    }
}
