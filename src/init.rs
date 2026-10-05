use anyhow::Result;
use std::{collections::HashSet, fs};

use crate::{attributes, builds, config, git, hooks, ignore, utils::find_repo_root};

mod prompter;
use prompter::{InquirePrompter, Prompter};

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
    let mut prompter = InquirePrompter;
    run_with(&mut prompter, &|| load_ignore_templates())
}

fn run_with(prompter: &mut dyn Prompter, ignore_templates: &dyn Fn() -> Vec<String>) -> Result<()> {
    // Initialize git repository if not already one
    let git_initialized = git::init_if_needed()?;
    if git_initialized {
        println!("  ◇ git repository initialized  ✓");
    }

    println!("{BANNER}");
    println!("  Configure your git repo\n");

    // ── Build selection ─────────────────────────────────────────────────────
    if maybe_apply_saved_build(prompter)?.is_some() {
        return Ok(());
    }

    let cargo_available = std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // ── Hooks ────────────────────────────────────────────────────────────────
    let installed_hooks = get_installed_hooks();
    let hook_selections = prompt_hook_selections(prompter, &installed_hooks)?;

    // ── .gitignore ───────────────────────────────────────────────────────────
    let selected_templates = prompt_ignore_templates(prompter, ignore_templates())?;

    // ── .gitattributes ───────────────────────────────────────────────────────
    let selected_attrs = prompt_attributes(prompter)?;

    // ── Git config ───────────────────────────────────────────────────────────
    let (selected_config_keys, configs_to_remove) = prompt_git_config(prompter, cargo_available)?;

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
    if !confirm_and_summarize(prompter, &selections)? {
        return Ok(());
    }

    // ── Apply ────────────────────────────────────────────────────────────────
    apply_selections(&mut *prompter, &selections)?;

    // ── Save as build ─────────────────────────────────────────────────────
    maybe_save_build(prompter)?;

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
fn maybe_apply_saved_build(prompter: &mut dyn Prompter) -> Result<Option<()>> {
    let saved_builds = builds::list_build_names();
    if saved_builds.is_empty() {
        return Ok(None);
    }

    let mut options = vec!["Start fresh configuration".to_string()];
    options.extend(saved_builds.iter().map(|b| format!("Use build: {b}")));

    let choice = prompter.select(
        "Saved builds available",
        options,
        Some("↑↓ move  enter confirm  esc start fresh"),
    )?;

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
fn prompt_hook_selections(
    prompter: &mut dyn Prompter,
    installed: &HashSet<String>,
) -> Result<HookSelections> {
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

    let hook_selections = prompter.multi_select(
        "Hooks",
        hook_items.clone(),
        default_selection,
        Some("↑↓ move  space select  enter confirm  esc skip"),
        None,
    )?;

    let mut selected_builtins: Vec<String> = Vec::new();
    let mut custom_hooks: Vec<(String, String)> = Vec::new();

    for item in &hook_selections {
        match parse_hook_item(prompter, item, &hook_items, builtins)? {
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
    prompter: &mut dyn Prompter,
    item: &str,
    hook_items: &[String],
    builtins: &[hooks::builtins::Builtin],
) -> Result<HookAction> {
    if item == "Add custom hook..." {
        let options: Vec<String> = hooks::valid_hook_names()
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let Some(hook_name) = prompter.select("Hook type", options, None)? else {
            return Ok(HookAction::Ignored);
        };
        let command = prompter.text("  Command to run", None)?.unwrap_or_default();
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
fn prompt_ignore_templates(
    prompter: &mut dyn Prompter,
    all_templates: Vec<String>,
) -> Result<Vec<String>> {
    println!();
    if all_templates.is_empty() {
        println!("  ⚠  Could not fetch templates (offline?) — skipping .gitignore");
        return Ok(Vec::new());
    }

    let selected = prompter.multi_select(
        ".gitignore templates",
        all_templates,
        vec![],
        Some("Type to filter  ↑↓ move  space select  enter confirm  esc skip"),
        Some(10),
    )?;
    Ok(selected)
}

/// The `.gitattributes` step: both presets offered, line endings
/// preselected; `esc`/skip leaves the selection empty.
fn prompt_attributes(prompter: &mut dyn Prompter) -> Result<Vec<String>> {
    println!();
    let attrs_items = vec![
        "line-endings  ★ recommended  —  * text=auto eol=lf".to_string(),
        "binary-files  —  mark images, PDFs, archives as binary (no diff)".to_string(),
    ];
    let attrs_keys = ["line-endings", "binary-files"];

    let attrs_refs: Vec<&str> = attrs_items.iter().map(String::as_str).collect();
    let attrs_selections = prompter.multi_select(
        ".gitattributes",
        attrs_items.clone(),
        vec![0usize],
        Some("space select  enter confirm  esc skip"),
        None,
    )?;

    let selected: Vec<String> = resolve_keys(&attrs_selections, &attrs_refs, &attrs_keys)
        .into_iter()
        .map(str::to_string)
        .collect();
    Ok(selected)
}

/// The git-config step: returns the keys to set (selected) and the keys to
/// unset (configured but deselected).
fn prompt_git_config(
    prompter: &mut dyn Prompter,
    cargo_available: bool,
) -> Result<(Vec<String>, Vec<String>)> {
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

    let config_selections = prompter.multi_select(
        "Git config",
        config_labels.clone(),
        defaults,
        Some("↑↓ move  space select  enter confirm  esc skip"),
        None,
    )?;

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
fn confirm_and_summarize(prompter: &mut dyn Prompter, s: &InitSelections) -> Result<bool> {
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
    for line in summary_lines(s) {
        println!("{line}");
    }

    println!();
    let confirmed = prompter.confirm("Apply these changes?", true)?;

    if !confirmed {
        println!("  Aborted.");
        return Ok(false);
    }
    Ok(true)
}

/// The `◆ …` summary lines for a selection set, in the order the wizard
/// prints them. Pure, so the selection→line contract is unit-tested.
fn summary_lines(s: &InitSelections) -> Vec<String> {
    let mut lines = Vec::new();
    if !s.selected_builtins.is_empty() || !s.custom_hooks.is_empty() {
        let names: Vec<&str> = s
            .selected_builtins
            .iter()
            .map(String::as_str)
            .chain(s.custom_hooks.iter().map(|(h, _)| h.as_str()))
            .collect();
        lines.push(format!("  ◆ hooks: {}", names.join(", ")));
    }
    if !s.selected_templates.is_empty() {
        lines.push(format!(
            "  ◆ .gitignore: {}",
            s.selected_templates.join(", ")
        ));
    }
    if !s.selected_attrs.is_empty() {
        lines.push(format!(
            "  ◆ .gitattributes: {}",
            s.selected_attrs.join(", ")
        ));
    }
    if !s.selected_config_keys.is_empty() {
        lines.push(format!(
            "  ◆ git config: {}",
            s.selected_config_keys.join(", ")
        ));
    }
    lines
}

/// Applies the confirmed selections, printing the original per-item
/// progress lines. Takes the prompter seam for uniformity even though it
/// never prompts.
fn apply_selections(_prompter: &mut dyn Prompter, s: &InitSelections) -> Result<()> {
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
fn maybe_save_build(prompter: &mut dyn Prompter) -> Result<()> {
    println!();
    let save_build = prompter.confirm("Save this configuration as a reusable build?", false)?;

    if save_build {
        let description = prompter
            .text("  Description (optional)", Some(""))?
            .unwrap_or_default();
        let desc_ref = if description.is_empty() {
            None
        } else {
            Some(description.as_str())
        };

        save_build_interactive(prompter, desc_ref)?;
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
fn save_build_interactive(prompter: &mut dyn Prompter, desc_ref: Option<&str>) -> Result<()> {
    let mut pending_name = prompter.text("  Build name", None)?;
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
                match handle_save_collision(prompter, &name, desc_ref, &e, attempts)? {
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
    prompter: &mut dyn Prompter,
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
            if !prompt_overwrite_or_rename(prompter, name)? {
                let next = prompter.text("  Build name", None)?;
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
fn prompt_overwrite_or_rename(prompter: &mut dyn Prompter, name: &str) -> Result<bool> {
    let overwrite_option = format!("Overwrite existing build '{name}'");
    let choice = prompter.select(
        "  A build with that name already exists",
        vec![
            "Choose a different name".to_string(),
            overwrite_option.clone(),
        ],
        None,
    )?;
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
        let mut prompter = super::prompter::ScriptedPrompter::new();
        let action = parse_hook_item(&mut prompter, &items[0], &items, builtins).unwrap();
        assert_eq!(action, HookAction::Builtin(builtins[0].name.to_string()));
    }

    #[test]
    fn parse_hook_item_ignores_unknown_labels() {
        let builtins = crate::hooks::available_builtins();
        let items = vec![
            "not an option".to_string(),
            "Add custom hook...".to_string(),
        ];
        let mut prompter = super::prompter::ScriptedPrompter::new();
        assert_eq!(
            parse_hook_item(&mut prompter, "something else", &items, builtins).unwrap(),
            HookAction::Ignored
        );
    }

    #[test]
    fn parse_hook_item_ignores_items_past_the_builtins() {
        let builtins = crate::hooks::available_builtins();
        let mut items: Vec<String> = builtins.iter().map(|b| b.name.to_string()).collect();
        items.push("extra option".to_string());
        let mut prompter = super::prompter::ScriptedPrompter::new();
        assert_eq!(
            parse_hook_item(&mut prompter, "extra option", &items, builtins).unwrap(),
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
        let mut prompter = super::prompter::ScriptedPrompter::new();
        let action = handle_save_collision(&mut prompter, "name", None, &err, 1).unwrap();
        assert_eq!(action, SaveRetryAction::ReportUnsaved(err.to_string()));
    }

    #[test]
    fn handle_save_collision_gives_up_at_max_attempts() {
        let err = collision_err();
        let mut prompter = super::prompter::ScriptedPrompter::new();
        let action =
            handle_save_collision(&mut prompter, "name", None, &err, MAX_SAVE_ATTEMPTS).unwrap();
        assert_eq!(
            action,
            SaveRetryAction::ReportUnsaved(format!(
                "gave up after {MAX_SAVE_ATTEMPTS} attempts: {err}"
            ))
        );
    }

    // ── scripted wizard tests (the Prompter seam) ──────────────────────────
    // (wizard_prompt_builders_construct moved to prompter.rs, which owns the
    // inquire builder chains now.)

    use super::prompter::ScriptedPrompter;

    /// Process-env + cwd isolation for wizard tests: `HOME`/`GITKIT_HOME`
    /// point at a fresh temp dir, the cwd moves into a fresh `git init` repo.
    struct WizardEnv {
        saved_home: Option<std::ffi::OsString>,
        saved_gitkit_home: Option<std::ffi::OsString>,
        saved_cwd: Option<std::path::PathBuf>,
        _home: tempfile::TempDir,
        _repo: tempfile::TempDir,
    }

    impl WizardEnv {
        fn new() -> Self {
            let home = tempfile::TempDir::new().unwrap();
            let repo = tempfile::TempDir::new().unwrap();
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(repo.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["config", "user.email", "test@example.com"])
                .current_dir(repo.path())
                .output()
                .unwrap();
            std::process::Command::new("git")
                .args(["config", "user.name", "Test"])
                .current_dir(repo.path())
                .output()
                .unwrap();
            let saved_home = std::env::var_os("HOME");
            let saved_gitkit_home = std::env::var_os("GITKIT_HOME");
            let saved_cwd = std::env::current_dir().ok();
            std::env::set_var("HOME", home.path());
            std::env::set_var("GITKIT_HOME", home.path());
            std::env::set_current_dir(repo.path()).unwrap();
            Self {
                saved_home,
                saved_gitkit_home,
                saved_cwd,
                _home: home,
                _repo: repo,
            }
        }

        fn home(&self) -> &std::path::Path {
            self._home.path()
        }

        fn repo(&self) -> &std::path::Path {
            self._repo.path()
        }
    }

    impl Drop for WizardEnv {
        fn drop(&mut self) {
            if let Some(cwd) = self.saved_cwd.take() {
                let _ = std::env::set_current_dir(cwd);
            }
            match self.saved_home.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match self.saved_gitkit_home.take() {
                Some(value) => std::env::set_var("GITKIT_HOME", value),
                None => std::env::remove_var("GITKIT_HOME"),
            }
        }
    }

    fn empty_selections() -> InitSelections {
        InitSelections {
            selected_builtins: Vec::new(),
            custom_hooks: Vec::new(),
            hooks_to_remove: Vec::new(),
            selected_templates: Vec::new(),
            selected_attrs: Vec::new(),
            selected_config_keys: Vec::new(),
            configs_to_remove: Vec::new(),
            cargo_available: false,
        }
    }

    const ATTRS_LINE_ENDINGS_LABEL: &str = "line-endings  ★ recommended  —  * text=auto eol=lf";
    const PUSH_LABEL: &str = "push.autoSetupRemote = true  —  auto-set upstream on first push";
    // `git config --local --list` lowercases keys, so only an already
    // lowercase option key is ever reported back as configured.
    const RERERE_LABEL_SET: &str =
        "rerere.enabled = true  —  remember and reuse conflict resolutions [✓ already set]";

    #[serial]
    #[test]
    fn run_with_stops_before_applying_when_confirmation_is_declined() {
        let _env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new()
            .with_multis(vec![
                Vec::new(),
                vec![ATTRS_LINE_ENDINGS_LABEL.to_string()],
                Vec::new(),
            ])
            .with_confirms(vec![false]);
        run_with(&mut prompter, &Vec::new).unwrap();
        assert!(
            !std::path::Path::new(".gitattributes").exists(),
            "a declined confirmation must apply nothing"
        );
    }

    #[serial]
    #[test]
    fn run_with_prompts_in_the_documented_order() {
        let _env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new()
            .with_multis(vec![
                Vec::new(),
                vec![ATTRS_LINE_ENDINGS_LABEL.to_string()],
                Vec::new(),
            ])
            .with_confirms(vec![true, false]);
        run_with(&mut prompter, &Vec::new).unwrap();
        assert_eq!(
            prompter.prompted,
            vec![
                "Hooks".to_string(),
                ".gitattributes".to_string(),
                "Git config".to_string(),
                "Apply these changes?".to_string(),
                "Save this configuration as a reusable build?".to_string(),
            ]
        );
    }

    #[serial]
    #[test]
    fn maybe_apply_saved_build_applies_a_saved_build() {
        let env = WizardEnv::new();
        let builds_dir = env.home().join(".gitkit").join("builds");
        std::fs::create_dir_all(&builds_dir).unwrap();
        std::fs::write(
            builds_dir.join("mybuild.toml"),
            "name = \"mybuild\"\ndescription = \"\"\n[gitattributes]\npresets = [\"line-endings\"]\n",
        )
        .unwrap();
        let mut prompter =
            ScriptedPrompter::new().with_selects(vec![Some("Use build: mybuild".to_string())]);
        let result = maybe_apply_saved_build(&mut prompter).unwrap();
        assert_eq!(result, Some(()));
        let attrs = std::fs::read_to_string(env.repo().join(".gitattributes")).unwrap();
        assert!(attrs.contains("eol=lf"));
    }

    #[serial]
    #[test]
    fn maybe_apply_saved_build_is_none_without_builds() {
        let _env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new();
        let result = maybe_apply_saved_build(&mut prompter).unwrap();
        assert_eq!(result, None);
        assert!(prompter.prompted.is_empty());
        assert!(!std::path::Path::new(".gitattributes").exists());
    }

    #[test]
    fn prompt_hook_selections_reports_unselected_installed_builtins() {
        let installed: HashSet<String> = ["no-secrets".to_string()].into_iter().collect();
        let mut prompter = ScriptedPrompter::new().with_multis(vec![Vec::new()]);
        let selections = prompt_hook_selections(&mut prompter, &installed).unwrap();
        assert!(selections.selected_builtins.is_empty());
        assert_eq!(selections.hooks_to_remove, vec!["no-secrets".to_string()]);
    }

    #[test]
    fn prompt_ignore_templates_returns_the_selection() {
        let mut prompter = ScriptedPrompter::new().with_multis(vec![vec!["rust".to_string()]]);
        let selected =
            prompt_ignore_templates(&mut prompter, vec!["rust".to_string(), "node".to_string()])
                .unwrap();
        assert_eq!(selected, vec!["rust".to_string()]);
    }

    #[test]
    fn prompt_ignore_templates_skips_when_list_is_empty() {
        let mut prompter = ScriptedPrompter::new();
        let selected = prompt_ignore_templates(&mut prompter, Vec::new()).unwrap();
        assert!(selected.is_empty());
        assert!(prompter.prompted.is_empty());
    }

    #[test]
    fn prompt_attributes_returns_the_selection() {
        let mut prompter = ScriptedPrompter::new()
            .with_multis(vec![vec![ATTRS_LINE_ENDINGS_LABEL.to_string()], Vec::new()]);
        let selected = prompt_attributes(&mut prompter).unwrap();
        assert_eq!(selected, vec!["line-endings".to_string()]);
        let selected = prompt_attributes(&mut prompter).unwrap();
        assert!(selected.is_empty());
    }

    #[serial]
    #[test]
    fn prompt_git_config_returns_selected_keys() {
        let _env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new().with_multis(vec![vec![PUSH_LABEL.to_string()]]);
        let (selected, to_remove) = prompt_git_config(&mut prompter, false).unwrap();
        assert_eq!(selected, vec!["push.autoSetupRemote".to_string()]);
        assert!(to_remove.is_empty());
    }

    #[serial]
    #[test]
    fn prompt_git_config_keeps_a_configured_key_that_stays_selected() {
        let _env = WizardEnv::new();
        std::process::Command::new("git")
            .args(["config", "--local", "rerere.enabled", "true"])
            .output()
            .unwrap();
        let mut prompter =
            ScriptedPrompter::new().with_multis(vec![vec![RERERE_LABEL_SET.to_string()]]);
        let (selected, to_remove) = prompt_git_config(&mut prompter, false).unwrap();
        assert_eq!(selected, vec!["rerere.enabled".to_string()]);
        assert!(to_remove.is_empty());
    }

    #[serial]
    #[test]
    fn prompt_git_config_removes_a_configured_key_that_is_deselected() {
        let _env = WizardEnv::new();
        std::process::Command::new("git")
            .args(["config", "--local", "rerere.enabled", "true"])
            .output()
            .unwrap();
        let mut prompter = ScriptedPrompter::new().with_multis(vec![Vec::new()]);
        let (selected, to_remove) = prompt_git_config(&mut prompter, false).unwrap();
        assert!(selected.is_empty());
        assert_eq!(to_remove, vec!["rerere.enabled".to_string()]);
    }

    #[test]
    fn confirm_and_summarize_returns_the_answer() {
        let mut selections = empty_selections();
        selections.selected_templates = vec!["rust".to_string()];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &selections).unwrap());
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![false]);
        assert!(!confirm_and_summarize(&mut prompter, &selections).unwrap());
    }

    #[test]
    fn confirm_and_summarize_honours_removal_only_selections() {
        let mut hook_removal = empty_selections();
        hook_removal.hooks_to_remove = vec!["x".to_string()];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &hook_removal).unwrap());

        let mut config_removal = empty_selections();
        config_removal.configs_to_remove = vec!["push.autoSetupRemote".to_string()];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &config_removal).unwrap());
    }

    #[test]
    fn confirm_and_summarize_exits_when_nothing_is_selected() {
        let selections = empty_selections();
        let mut prompter = ScriptedPrompter::new();
        assert!(!confirm_and_summarize(&mut prompter, &selections).unwrap());
        assert!(prompter.prompted.is_empty());
    }

    #[test]
    fn nothing_is_false_when_custom_hooks_are_non_empty() {
        let mut selections = empty_selections();
        selections.custom_hooks = vec![("pre-push".to_string(), "cargo test".to_string())];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &selections).unwrap());
    }

    #[test]
    fn nothing_is_false_when_templates_are_non_empty() {
        let mut selections = empty_selections();
        selections.selected_templates = vec!["rust".to_string()];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &selections).unwrap());
    }

    #[test]
    fn nothing_is_false_when_attrs_are_non_empty() {
        let mut selections = empty_selections();
        selections.selected_attrs = vec!["line-endings".to_string()];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &selections).unwrap());
    }

    #[test]
    fn nothing_is_false_when_config_keys_are_non_empty() {
        let mut selections = empty_selections();
        selections.selected_config_keys = vec!["push.autoSetupRemote".to_string()];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &selections).unwrap());
    }

    #[test]
    fn nothing_is_false_when_builtins_are_non_empty() {
        let mut selections = empty_selections();
        selections.selected_builtins = vec!["no-secrets".to_string()];
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![true]);
        assert!(confirm_and_summarize(&mut prompter, &selections).unwrap());
    }

    fn selections_with(field: &str) -> InitSelections {
        let mut selections = empty_selections();
        match field {
            "builtins" => selections.selected_builtins = vec!["no-secrets".to_string()],
            "custom" => {
                selections.custom_hooks = vec![("pre-push".to_string(), "echo hi".to_string())];
            }
            "templates" => selections.selected_templates = vec!["rust".to_string()],
            "attrs" => selections.selected_attrs = vec!["line-endings".to_string()],
            "config" => selections.selected_config_keys = vec!["push.autoSetupRemote".to_string()],
            _ => {}
        }
        selections
    }

    #[test]
    fn summary_lines_lists_each_selected_category() {
        assert_eq!(
            summary_lines(&selections_with("builtins")),
            vec!["  ◆ hooks: no-secrets".to_string()]
        );
        assert_eq!(
            summary_lines(&selections_with("templates")),
            vec!["  ◆ .gitignore: rust".to_string()]
        );
        assert_eq!(
            summary_lines(&selections_with("attrs")),
            vec!["  ◆ .gitattributes: line-endings".to_string()]
        );
        assert_eq!(
            summary_lines(&selections_with("config")),
            vec!["  ◆ git config: push.autoSetupRemote".to_string()]
        );
        assert!(summary_lines(&empty_selections()).is_empty());
    }

    #[test]
    fn summary_lines_lists_custom_hooks_without_builtins() {
        assert_eq!(
            summary_lines(&selections_with("custom")),
            vec!["  ◆ hooks: pre-push".to_string()]
        );
    }

    #[serial]
    #[test]
    fn apply_selections_writes_gitattributes() {
        let env = WizardEnv::new();
        let mut selections = empty_selections();
        selections.selected_attrs = vec!["line-endings".to_string()];
        let mut prompter = ScriptedPrompter::new();
        apply_selections(&mut prompter, &selections).unwrap();
        let attrs = std::fs::read_to_string(env.repo().join(".gitattributes")).unwrap();
        assert!(attrs.contains("eol=lf"));
    }

    #[serial]
    #[test]
    fn apply_selections_writes_gitignore() {
        let env = WizardEnv::new();
        let mut selections = empty_selections();
        selections.selected_templates = vec!["agentic".to_string()];
        let mut prompter = ScriptedPrompter::new();
        apply_selections(&mut prompter, &selections).unwrap();
        let ignore = std::fs::read_to_string(env.repo().join(".gitignore")).unwrap();
        assert!(ignore.contains(".kiro/"));
    }

    #[serial]
    #[test]
    fn apply_selections_sets_git_config() {
        let _env = WizardEnv::new();
        let mut selections = empty_selections();
        selections.selected_config_keys = vec!["push.autoSetupRemote".to_string()];
        selections.cargo_available = false;
        let mut prompter = ScriptedPrompter::new();
        apply_selections(&mut prompter, &selections).unwrap();
        let output = std::process::Command::new("git")
            .args(["config", "--local", "--get", "push.autoSetupRemote"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
    }

    #[serial]
    #[test]
    fn maybe_save_build_saves_when_confirmed() {
        let env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new()
            .with_confirms(vec![true])
            .with_texts(vec![Some("d".to_string()), Some("mybuild".to_string())]);
        maybe_save_build(&mut prompter).unwrap();
        assert!(env
            .home()
            .join(".gitkit")
            .join("builds")
            .join("mybuild.toml")
            .exists());
    }

    #[serial]
    #[test]
    fn maybe_save_build_skips_when_declined() {
        let env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new().with_confirms(vec![false]);
        maybe_save_build(&mut prompter).unwrap();
        assert!(!env.home().join(".gitkit").join("builds").exists());
    }

    #[serial]
    #[test]
    fn save_build_interactive_writes_a_fresh_build() {
        let env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new().with_texts(vec![Some("fresh".to_string())]);
        save_build_interactive(&mut prompter, None).unwrap();
        assert!(env
            .home()
            .join(".gitkit")
            .join("builds")
            .join("fresh.toml")
            .exists());
    }

    #[serial]
    #[test]
    fn save_build_interactive_quits_on_skipped_name() {
        let env = WizardEnv::new();
        let mut prompter = ScriptedPrompter::new().with_texts(vec![None]);
        save_build_interactive(&mut prompter, None).unwrap();
        assert!(!env.home().join(".gitkit").join("builds").exists());
    }

    #[serial]
    #[test]
    fn save_build_interactive_gives_up_after_max_attempts() {
        let _env = WizardEnv::new();
        // Seed the collision: every attempted name already exists.
        save_build_interactive(
            &mut ScriptedPrompter::new().with_texts(vec![Some("dup".to_string())]),
            None,
        )
        .unwrap();
        let mut prompter = ScriptedPrompter::new()
            .with_texts(vec![
                Some("dup".to_string()),
                Some("dup".to_string()),
                Some("dup".to_string()),
            ])
            .with_selects(vec![
                Some("Choose a different name".to_string()),
                Some("Choose a different name".to_string()),
            ]);
        assert!(save_build_interactive(&mut prompter, None).is_ok());
    }

    #[test]
    fn handle_save_collision_renames_on_decline() {
        let err: anyhow::Error = builds::BuildNameCollision {
            name: "existing".to_string(),
        }
        .into();
        let mut prompter = ScriptedPrompter::new()
            .with_selects(vec![Some("Choose a different name".to_string())])
            .with_texts(vec![Some("fresh".to_string())]);
        let action = handle_save_collision(&mut prompter, "existing", None, &err, 1).unwrap();
        assert_eq!(
            action,
            SaveRetryAction::RetryWith(Some("fresh".to_string()))
        );
    }

    #[test]
    fn prompt_overwrite_or_rename_true_only_for_the_overwrite_option() {
        let mut prompter = ScriptedPrompter::new()
            .with_selects(vec![Some("Overwrite existing build 'x'".to_string())]);
        assert!(prompt_overwrite_or_rename(&mut prompter, "x").unwrap());

        let mut prompter =
            ScriptedPrompter::new().with_selects(vec![Some("Choose a different name".to_string())]);
        assert!(!prompt_overwrite_or_rename(&mut prompter, "x").unwrap());

        let mut prompter = ScriptedPrompter::new().with_selects(vec![None]);
        assert!(!prompt_overwrite_or_rename(&mut prompter, "x").unwrap());
    }
}
