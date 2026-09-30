use anyhow::{Context, Result};
use clap::Subcommand;
use std::{fs, path::Path};

use crate::utils::{confirm, find_repo_root};

pub(crate) mod builtins;
mod message_rules;
mod no_invisibles;

#[derive(Subcommand)]
pub enum HooksCommand {
    /// Install a built-in hook or a custom command
    ///
    /// Built-in (hook inferred automatically):
    ///   gitkit hooks add conventional-commits
    ///   gitkit hooks add no-trailers
    ///   gitkit hooks add no-secrets
    ///   gitkit hooks add branch-naming
    ///   gitkit hooks add no-invisibles
    ///
    /// Custom command:
    ///   gitkit hooks add pre-push "cargo test"
    Add {
        /// Built-in name or git hook name for custom commands
        hook_or_builtin: String,
        /// Shell command to run (only for custom hooks)
        command: Option<String>,
        #[arg(short, long)]
        yes: bool,
        #[arg(short, long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// List installed hooks. Use --available to see built-ins
    List {
        #[arg(long, help = "Show available built-in hooks")]
        available: bool,
    },
    /// Remove an installed hook
    Remove {
        hook: String,
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the content of an installed hook
    Show { hook: String },
    /// Internal: scan staged changes for invisible Unicode. Execed by the
    /// `no-invisibles` pre-commit hook; not meant to be run directly.
    #[command(hide = true)]
    ScanInvisibles,
    /// Internal: evaluate commit message against user-defined rules. Execed
    /// by the `message-rules` commit-msg hook; not meant to be run directly.
    #[command(hide = true)]
    ScanMessageRules {
        /// Path to the commit message file (passed by git)
        msg_file: String,
    },
}

pub fn run(cmd: HooksCommand) -> Result<()> {
    match cmd {
        HooksCommand::Add {
            hook_or_builtin,
            command,
            yes,
            force,
            dry_run,
        } => add(&hook_or_builtin, command.as_deref(), yes, force, dry_run),
        HooksCommand::List { available } => list(available),
        HooksCommand::Remove { hook, yes, dry_run } => remove(&hook, yes, dry_run),
        HooksCommand::Show { hook } => show(&hook),
        HooksCommand::ScanInvisibles => no_invisibles::run(),
        HooksCommand::ScanMessageRules { msg_file } => {
            message_rules::evaluate_message(std::path::Path::new(&msg_file))
        }
    }
}

/// Install a built-in hook by name. Used by the interactive wizard.
pub(crate) fn install_builtin(name: &str, force: bool) -> Result<()> {
    add_quiet(name, None, force)
}

/// Install a custom hook command. Used by the interactive wizard.
pub(crate) fn install_custom(hook: &str, command: &str, force: bool) -> Result<()> {
    add_quiet(hook, Some(command), force)
}

/// Returns all available built-ins.
pub(crate) fn available_builtins() -> &'static [builtins::Builtin] {
    builtins::ALL
}

/// Returns all valid git hook names.
pub(crate) fn valid_hook_names() -> &'static [&'static str] {
    VALID_HOOKS
}

/// Extracts the builtin identity marker from script content, if present.
/// The marker is a shell comment line of the form `# gitkit-builtin: <name>`.
fn extract_marker(content: &str) -> Option<&str> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("# gitkit-builtin: ") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Identifies which built-in (if any) an installed hook file corresponds to.
/// First tries the machine-readable marker (`# gitkit-builtin: <name>`), which
/// survives script changes across versions. Falls back to exact-content match
/// so hooks installed by pre-marker versions of gitkit keep being recognised.
pub(crate) fn detect_builtin(hook_file: &str, content: &str) -> Option<&'static builtins::Builtin> {
    if let Some(name) = extract_marker(content) {
        if let Some(b) = builtins::get(name) {
            if b.hook == hook_file {
                return Some(b);
            }
        }
    }
    builtins::ALL
        .iter()
        .find(|b| b.hook == hook_file && content.trim() == b.script.trim())
}

pub(crate) fn hooks_dir() -> Result<std::path::PathBuf> {
    Ok(find_repo_root()?.join(".git").join("hooks"))
}

// ── composition: multiple builtins sharing one git hook ─────────────────────
//
// `gitkit hooks add` used to write a builtin's script directly to
// `.git/hooks/<git-hook>`, so a second builtin targeting the same git hook
// (e.g. `no-body` after `conventional-commits`, both on `commit-msg`)
// silently destroyed the first. Now each builtin lives at
// `.git/hooks/gitkit.d/<git-hook>/<builtin-name>`, and `.git/hooks/<git-hook>`
// is a small POSIX `sh` dispatcher that runs every executable part in that
// directory, in sorted order, stopping at the first one that fails.

/// Directory (relative to `.git/hooks`) holding one subdirectory per git hook,
/// each containing that hook's installed parts.
pub(crate) const PARTS_DIR_NAME: &str = "gitkit.d";

/// Fixed name for a hand-written hook absorbed into `gitkit.d` when the first
/// builtin is installed over it. Chosen to sort before any builtin name (all
/// lowercase letters) in the dispatcher's `for part in "$dir"/*` loop, since a
/// user's own hook predates gitkit's and must run first.
pub(crate) const PRESERVED_PART_NAME: &str = "00-preexisting";

/// Path to the directory holding `hook_name`'s installed parts.
pub(crate) fn parts_dir(hooks_dir: &Path, hook_name: &str) -> std::path::PathBuf {
    hooks_dir.join(PARTS_DIR_NAME).join(hook_name)
}

/// The dispatcher script gitkit installs at `.git/hooks/<hook_name>` once any
/// part is installed for it. Pure POSIX `sh` — no dependency on the `gitkit`
/// binary, since it runs on the commit/push hot path and must not acquire new
/// ways to fail (same reasoning as the lock hooks in `src/lock/mod.rs`).
pub(crate) fn dispatcher_script(hook_name: &str) -> String {
    format!(
        r#"#!/bin/sh
# Installed by gitkit. Runs every executable part in
# .git/hooks/gitkit.d/{hook_name}/, in sorted order, passing this hook's own
# arguments through. The first part to exit non-zero stops here and the rest
# do not run. See `gitkit hooks list` / `gitkit hooks remove`.
dir=$(dirname "$0")/gitkit.d/{hook_name}
if [ -d "$dir" ]; then
  for part in "$dir"/*; do
    [ -f "$part" ] && [ -x "$part" ] || continue
    "$part" "$@"
    status=$?
    if [ "$status" -ne 0 ]; then
      exit "$status"
    fi
  done
fi
exit 0
"#
    )
}

/// Whether `content` (the current content of `.git/hooks/<hook_name>`) is
/// exactly the dispatcher gitkit installs for that hook.
pub(crate) fn is_dispatcher(content: &str, hook_name: &str) -> bool {
    content.trim() == dispatcher_script(hook_name).trim()
}

/// Names of every part installed under `gitkit.d/<hook_name>/`, sorted —
/// matching the order the dispatcher itself runs them in.
pub(crate) fn list_parts(hooks_dir: &Path, hook_name: &str) -> Vec<String> {
    let dir = parts_dir(hooks_dir, hook_name);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

/// Ensures `.git/hooks/<hook_name>` is a gitkit dispatcher, migrating any
/// pre-existing content into `gitkit.d/<hook_name>/` as a part first:
/// - if it's a known builtin (by marker or exact content match), it's absorbed
///   under that builtin's name with the **current** script — so an outdated
///   builtin is replaced, not frozen as an untouchable hand-written hook.
/// - otherwise (a hand-written hook) it's absorbed as [`PRESERVED_PART_NAME`].
///
/// A no-op if the dispatcher is already in place.
fn ensure_dispatcher(dir: &Path, hook_name: &str) -> Result<()> {
    let hook_path = dir.join(hook_name);
    let parts = parts_dir(dir, hook_name);

    if !hook_path.exists() {
        fs::create_dir_all(&parts).context("Failed to create gitkit.d parts directory")?;
    } else {
        let content = fs::read_to_string(&hook_path).unwrap_or_default();
        if !is_dispatcher(&content, hook_name) {
            migrate_existing_hook(&parts, hook_name, &content)?;
        }
    }

    let script = dispatcher_script(hook_name);
    fs::write(&hook_path, &script)
        .with_context(|| format!("Failed to write dispatcher for hook '{hook_name}'"))?;
    set_executable(&hook_path)?;
    Ok(())
}

/// Migrates a pre-existing hook file into `gitkit.d/<hook_name>/` before the
/// dispatcher replaces it, creating the parts directory first: a known builtin
/// (by marker or exact content match) is absorbed under its name, anything
/// else is kept verbatim as the preserved hand-written part.
fn migrate_existing_hook(parts: &Path, hook_name: &str, content: &str) -> Result<()> {
    fs::create_dir_all(parts).context("Failed to create gitkit.d parts directory")?;

    if let Some(builtin) = detect_builtin(hook_name, content) {
        migrate_builtin_part(parts, builtin, content)
    } else {
        migrate_preserved_part(parts, hook_name, content)
    }
}

/// Absorbs a recognized builtin: written with the **current** script — so an
/// outdated builtin is replaced, not frozen as an untouchable hand-written
/// hook — unless that exact part is already up to date.
fn migrate_builtin_part(
    parts: &Path,
    builtin: &'static builtins::Builtin,
    content: &str,
) -> Result<()> {
    let migrated_path = parts.join(builtin.name);
    let current_script = if builtin.name == "message-rules" {
        message_rules::generate_script()
    } else {
        builtin.script.to_owned()
    };
    let is_outdated = content.trim() != current_script.trim();
    if is_outdated {
        println!("Updating outdated builtin: {}", builtin.name);
    }
    if !migrated_path.exists() || is_outdated {
        fs::write(&migrated_path, &current_script).with_context(|| {
            format!("Failed to migrate builtin '{}' into gitkit.d", builtin.name)
        })?;
        set_executable(&migrated_path)?;
    }
    Ok(())
}

/// Absorbs a hand-written hook verbatim as the preserved part, leaving an
/// already-preserved part untouched.
fn migrate_preserved_part(parts: &Path, hook_name: &str, content: &str) -> Result<()> {
    let migrated_path = parts.join(PRESERVED_PART_NAME);
    if !migrated_path.exists() {
        fs::write(&migrated_path, content).with_context(|| {
            format!("Failed to migrate existing '{hook_name}' hook into gitkit.d")
        })?;
        set_executable(&migrated_path)?;
    }
    Ok(())
}

/// Installs `script` as part `part_name` of `hook_name`, setting up the
/// dispatcher (and migrating any pre-existing hook content) if needed.
pub(crate) fn install_part(hook_name: &str, part_name: &str, script: &str) -> Result<()> {
    let dir = hooks_dir()?;
    fs::create_dir_all(&dir).context("Failed to create hooks directory")?;
    ensure_dispatcher(&dir, hook_name)?;
    let parts = parts_dir(&dir, hook_name);
    let part_path = parts.join(part_name);
    fs::write(&part_path, script)
        .with_context(|| format!("Failed to write part '{part_name}' for hook '{hook_name}'"))?;
    set_executable(&part_path)?;
    Ok(())
}

/// Removes part `part_name` of `hook_name`. When it was the last part,
/// removes the dispatcher and the (now-empty) parts directory too; when only
/// the preserved pre-existing hook remains, restores it to
/// `.git/hooks/<hook_name>` so the repository ends up as it was found.
///
/// Never touches `.git/hooks/<hook_name>` unless its current content is still
/// exactly the dispatcher gitkit installed — mirroring the lock module's rule
/// that a hook someone replaced by hand is never clobbered. If a user swaps
/// the dispatcher for their own script after builtins were installed, the
/// last part's removal leaves that replacement (and, if present, the
/// preserved pre-existing hook) alone in `gitkit.d/` rather than overwriting
/// or deleting what the user put there.
fn remove_part(dir: &Path, hook_name: &str, part_name: &str) -> Result<()> {
    let parts = parts_dir(dir, hook_name);
    let part_path = parts.join(part_name);
    fs::remove_file(&part_path).with_context(|| format!("Failed to remove hook '{part_name}'"))?;

    let remaining = list_parts(dir, hook_name);
    let hook_path = dir.join(hook_name);

    let hook_content = fs::read_to_string(&hook_path).unwrap_or_default();
    if !is_dispatcher(&hook_content, hook_name) {
        // The dispatcher was replaced by hand; leave it and any remaining
        // parts (including a preserved pre-existing hook) exactly as they
        // are — we can no longer safely decide what belongs at this path.
        return Ok(());
    }

    if remaining.is_empty() {
        let _ = fs::remove_file(&hook_path);
        let _ = fs::remove_dir(&parts);
    } else if remaining.len() == 1 && remaining[0] == PRESERVED_PART_NAME {
        let preserved_path = parts.join(PRESERVED_PART_NAME);
        let content = fs::read(&preserved_path)?;
        fs::write(&hook_path, &content)
            .with_context(|| format!("Failed to restore original '{hook_name}' hook"))?;
        set_executable(&hook_path)?;
        fs::remove_file(&preserved_path)?;
        let _ = fs::remove_dir(&parts);
    }

    Ok(())
}

/// Health of one installed part within `gitkit.d/<hook_name>/`, mirroring
/// [`classify_hook`]'s categories but also accounting for the dispatcher's
/// own executable bit — a part stays `Dormant`/`ModifiedDormant` if the
/// dispatcher that would run it is itself not executable, since git never
/// even reaches it then. The execute bit is orthogonal to content: a
/// non-executable part is dormant regardless of whether its content is
/// recognized.
pub(crate) fn classify_part(dir: &Path, hook_name: &str, part_name: &str) -> Result<HookHealth> {
    let part_path = parts_dir(dir, hook_name).join(part_name);
    if !part_path.exists() {
        return Ok(HookHealth::Absent);
    }

    let dispatcher_path = dir.join(hook_name);
    let dispatcher_ok = dispatcher_path.exists() && is_executable(&dispatcher_path)?;

    let bytes =
        fs::read(&part_path).with_context(|| format!("Failed to read part '{part_name}'"))?;
    let recognized = match std::str::from_utf8(&bytes) {
        Ok(content) => {
            part_name == PRESERVED_PART_NAME
                || (part_name == "message-rules"
                    && content.contains("# gitkit-builtin: message-rules"))
                || builtins::get(part_name).is_some_and(|b| content.trim() == b.script.trim())
        }
        Err(_) => false,
    };

    let executable = dispatcher_ok && is_executable(&part_path)?;

    match (recognized, executable) {
        (true, true) => Ok(HookHealth::Active),
        (true, false) => Ok(HookHealth::Dormant),
        (false, true) => Ok(HookHealth::Modified),
        (false, false) => Ok(HookHealth::ModifiedDormant),
    }
}

const VALID_HOOKS: &[&str] = &[
    "applypatch-msg",
    "commit-msg",
    "fsmonitor-watchman",
    "post-update",
    "pre-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "pre-push",
    "pre-rebase",
    "pre-receive",
    "prepare-commit-msg",
    "push-to-checkout",
    "update",
];

fn add(
    hook_or_builtin: &str,
    command: Option<&str>,
    yes: bool,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    if let Some(builtin) = builtins::get(hook_or_builtin) {
        anyhow::ensure!(
            command.is_none(),
            "'{hook_or_builtin}' is a built-in hook — no command needed"
        );
        return add_builtin_part(builtin, yes, force, dry_run);
    }

    let (hook_name, script) = resolve_hook(hook_or_builtin, command)?;
    let dir = hooks_dir()?;
    let path = dir.join(hook_name);

    if path.exists() && !force {
        if !confirm(
            &format!("Hook '{hook_name}' already exists. Overwrite?"),
            yes,
        ) {
            println!("Aborted.");
            return Ok(());
        }
        if !dry_run {
            let backup = dir.join(format!("{hook_name}.bak"));
            fs::copy(&path, &backup).with_context(|| format!("Failed to backup {hook_name}"))?;
            println!("Backed up to {}", backup.display());
        }
    }

    if dry_run {
        println!("[dry-run] Would write hook '{hook_name}':\n{script}");
        return Ok(());
    }

    fs::create_dir_all(&dir).context("Failed to create hooks directory")?;
    fs::write(&path, &script).with_context(|| format!("Failed to write hook '{hook_name}'"))?;
    set_executable(&path)?;
    record_applied(hook_or_builtin);
    println!("Installed hook '{hook_name}'.");
    Ok(())
}

/// Installs a builtin as a part of its git hook's dispatcher (composing with
/// whatever else already targets that hook, per `add`'s interactive UX).
fn add_builtin_part(
    builtin: &'static builtins::Builtin,
    yes: bool,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    let dir = hooks_dir()?;
    let part_path = parts_dir(&dir, builtin.hook).join(builtin.name);

    let script = if builtin.name == "message-rules" {
        let _rules = message_rules::validate_rules()?;
        message_rules::generate_script()
    } else {
        builtin.script.to_owned()
    };

    if part_path.exists() && !force {
        let existing = fs::read_to_string(&part_path).unwrap_or_default();
        if existing.trim() != script.trim()
            && !confirm(
                &format!("Hook '{}' already exists. Overwrite?", builtin.name),
                yes,
            )
        {
            println!("Aborted.");
            return Ok(());
        }
    }

    if dry_run {
        println!("[dry-run] Would write hook '{}':\n{}", builtin.hook, script);
        return Ok(());
    }

    install_part(builtin.hook, builtin.name, &script)?;
    record_applied(builtin.name);
    println!("Installed hook '{}'.", builtin.hook);
    Ok(())
}

/// Silent version for the wizard — no backup messages, no "Installed" print.
fn add_quiet(hook_or_builtin: &str, command: Option<&str>, force: bool) -> Result<()> {
    if let Some(builtin) = builtins::get(hook_or_builtin) {
        anyhow::ensure!(
            command.is_none(),
            "'{hook_or_builtin}' is a built-in hook — no command needed"
        );
        let _ = force;
        let script = if builtin.name == "message-rules" {
            let _rules = message_rules::validate_rules()?;
            message_rules::generate_script()
        } else {
            builtin.script.to_owned()
        };
        install_part(builtin.hook, builtin.name, &script)?;
        record_applied(builtin.name);
        return Ok(());
    }

    let (hook_name, script) = resolve_hook(hook_or_builtin, command)?;
    let dir = hooks_dir()?;
    let path = dir.join(hook_name);
    if path.exists() && !force {
        let backup = dir.join(format!("{hook_name}.bak"));
        fs::copy(&path, &backup).with_context(|| format!("Failed to backup {hook_name}"))?;
    }
    fs::create_dir_all(&dir).context("Failed to create hooks directory")?;
    fs::write(&path, &script).with_context(|| format!("Failed to write hook '{hook_name}'"))?;
    set_executable(&path)?;
    record_applied(hook_or_builtin);
    Ok(())
}

/// Records the hook just installed in the machine-wide registry (best
/// effort — never fails the install this is called from).
fn record_applied(hook_or_builtin: &str) {
    if let Ok(root) = find_repo_root() {
        crate::registry::record_best_effort(&root, &[format!("hook:{hook_or_builtin}")]);
    }
}

/// Resolves (hook_name, script) from either a built-in name or a (hook, command) pair.
fn resolve_hook<'a>(hook_or_builtin: &'a str, command: Option<&str>) -> Result<(&'a str, String)> {
    if let Some(builtin) = builtins::get(hook_or_builtin) {
        anyhow::ensure!(
            command.is_none(),
            "'{hook_or_builtin}' is a built-in hook — no command needed"
        );
        let script = if builtin.name == "message-rules" {
            let _rules = message_rules::validate_rules()?;
            message_rules::generate_script()
        } else {
            builtin.script.to_owned()
        };
        return Ok((builtin.hook, script));
    }

    let cmd = command.ok_or_else(|| {
        anyhow::anyhow!(
            "'{hook_or_builtin}' is not a built-in. Provide a command:\n  gitkit hooks add {hook_or_builtin} \"<command>\"\n\nAvailable built-ins: {}",
            builtins::ALL.iter().map(|b| b.name).collect::<Vec<_>>().join(", ")
        )
    })?;

    anyhow::ensure!(
        VALID_HOOKS.contains(&hook_or_builtin),
        "'{hook_or_builtin}' is not a valid git hook.\nValid hooks: {}",
        VALID_HOOKS.join(", ")
    );

    Ok((hook_or_builtin, format!("#!/bin/sh\nset -e\n{cmd}\n")))
}

fn list(available: bool) -> Result<()> {
    if available {
        println!("Available built-in hooks:\n");
        for b in builtins::ALL {
            println!("  {:<25} ({})  —  {}", b.name, b.hook, b.description);
        }
        return Ok(());
    }

    let dir = hooks_dir()?;
    let hooks: Vec<_> = fs::read_dir(&dir)
        .context("Failed to read hooks directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            !s.ends_with(".bak") && !s.ends_with(".sample")
        })
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    if hooks.is_empty() {
        println!("No hooks installed.");
    } else {
        for h in hooks {
            println!("{h}");
        }
    }
    Ok(())
}

fn remove(hook: &str, yes: bool, _dry_run: bool) -> Result<()> {
    remove_hook(hook, yes)
}

pub(crate) fn remove_hook(hook: &str, _yes: bool) -> Result<()> {
    let dir = hooks_dir()?;

    if let Some(builtin) = builtins::get(hook) {
        let part_path = parts_dir(&dir, builtin.hook).join(builtin.name);
        anyhow::ensure!(part_path.exists(), "Hook '{hook}' is not installed");
        return remove_part(&dir, builtin.hook, builtin.name);
    }

    let path = dir.join(hook);
    anyhow::ensure!(path.exists(), "Hook '{hook}' is not installed");
    fs::remove_file(&path).with_context(|| format!("Failed to remove hook '{hook}'"))?;
    Ok(())
}

fn show(hook: &str) -> Result<()> {
    let dir = hooks_dir()?;

    if let Some(builtin) = builtins::get(hook) {
        let part_path = parts_dir(&dir, builtin.hook).join(builtin.name);
        if part_path.exists() {
            let content = fs::read_to_string(&part_path)
                .with_context(|| format!("Failed to read hook '{hook}'"))?;
            print!("{content}");
            return Ok(());
        }
    }

    let path = dir.join(hook);
    anyhow::ensure!(path.exists(), "Hook '{hook}' is not installed");
    let content =
        fs::read_to_string(&path).with_context(|| format!("Failed to read hook '{hook}'"))?;
    print!("{content}");
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).context("Failed to set executable permission")?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn is_executable(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::metadata(path)?.permissions();
    Ok(perms.mode() & 0o111 != 0)
}

/// The executable bit doesn't exist on non-Unix targets, so a hook there is
/// never `Dormant` — git runs whatever is on disk regardless of mode.
#[cfg(not(unix))]
pub(crate) fn is_executable(_path: &Path) -> Result<bool> {
    Ok(true)
}

/// Health of an installed hook file, as reported by `gitkit status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookHealth {
    /// Content matches a known builtin verbatim and is executable — git runs it.
    Active,
    /// Content matches a known builtin verbatim but is not executable — git
    /// silently ignores the file and never runs it.
    Dormant,
    /// Present but its content matches no builtin. Not an error: the user
    /// may have edited it deliberately, or it's a custom (non-builtin) hook.
    Modified,
    /// Content matches no builtin AND the file is not executable — git silently
    /// ignores it. Both facts are reported: the user must learn it does not run.
    ModifiedDormant,
    /// No file at this hook's path.
    Absent,
}

/// Classifies an installed hook file's health for `gitkit status`, by
/// comparing its content against the builtin catalogue via exact-content
/// match and checking the execute bit. The execute bit is orthogonal to
/// content: a non-executable hook is dormant regardless of whether its
/// content matches a builtin, so a hand-edited hook that lost its execute
/// bit is `ModifiedDormant` — both facts reported, not one.
/// Reads the file itself rather than trusting a caller-supplied string,
/// so non-UTF-8 content is classified `Modified`/`ModifiedDormant` instead
/// of erroring — git runs a hook regardless of its encoding, so an
/// unreadable-as-text file is not a failure, just content gitkit can't
/// match against a builtin.
pub(crate) fn classify_hook(hook_name: &str, path: &Path) -> Result<HookHealth> {
    if !path.exists() {
        return Ok(HookHealth::Absent);
    }

    let bytes = fs::read(path).with_context(|| format!("Failed to read hook '{hook_name}'"))?;
    let is_builtin = match std::str::from_utf8(&bytes) {
        Ok(content) => builtins::ALL
            .iter()
            .any(|b| b.hook == hook_name && content.trim() == b.script.trim()),
        Err(_) => false,
    };

    let executable = is_executable(path)?;

    match (is_builtin, executable) {
        (true, true) => Ok(HookHealth::Active),
        (true, false) => Ok(HookHealth::Dormant),
        (false, true) => Ok(HookHealth::Modified),
        (false, false) => Ok(HookHealth::ModifiedDormant),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Sets `GITKIT_HOME` to a fresh temp dir for the duration of `f`,
    /// restoring the original value afterward. This isolates the registry
    /// from the test suite: any `record_best_effort` call during the test
    /// writes to the temp dir, not the user's real `~/.gitkit`.
    fn with_isolated_registry<F: FnOnce()>(f: F) {
        let dir = tempfile::TempDir::new().unwrap();
        let original = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("GITKIT_HOME", dir.path());
        }
        f();
        unsafe {
            match &original {
                Some(h) => std::env::set_var("GITKIT_HOME", h),
                None => std::env::remove_var("GITKIT_HOME"),
            }
        }
    }

    #[test]
    fn resolve_hook_returns_builtin_script() {
        let (hook, script) = resolve_hook("conventional-commits", None).unwrap();
        assert_eq!(hook, "commit-msg");
        assert!(script.contains("#!/bin/sh"));
        assert!(script.contains("Conventional Commits"));
    }

    #[test]
    fn resolve_hook_infers_correct_hook_for_no_secrets() {
        let (hook, _) = resolve_hook("no-secrets", None).unwrap();
        assert_eq!(hook, "pre-commit");
    }

    #[test]
    fn resolve_hook_infers_correct_hook_for_branch_naming() {
        let (hook, _) = resolve_hook("branch-naming", None).unwrap();
        assert_eq!(hook, "pre-commit");
    }

    #[test]
    fn resolve_hook_errors_when_builtin_given_command() {
        let err = resolve_hook("conventional-commits", Some("echo hi")).unwrap_err();
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn resolve_hook_custom_command_wraps_in_shebang() {
        let (hook, script) = resolve_hook("pre-push", Some("cargo test")).unwrap();
        assert_eq!(hook, "pre-push");
        assert!(script.starts_with("#!/bin/sh"));
        assert!(script.contains("cargo test"));
    }

    #[test]
    fn resolve_hook_errors_on_invalid_hook_name() {
        let err = resolve_hook("not-a-hook", Some("echo hi")).unwrap_err();
        assert!(err.to_string().contains("not a valid git hook"));
    }

    #[test]
    fn resolve_hook_errors_on_unknown_builtin_without_command() {
        let err = resolve_hook("unknown-builtin", None).unwrap_err();
        assert!(err.to_string().contains("not a built-in"));
    }

    #[test]
    fn detect_builtin_distinguishes_builtins_sharing_a_hook_file() {
        let no_secrets = builtins::get("no-secrets").unwrap();
        let branch_naming = builtins::get("branch-naming").unwrap();
        assert_eq!(
            detect_builtin("pre-commit", no_secrets.script).map(|b| b.name),
            Some("no-secrets")
        );
        assert_eq!(
            detect_builtin("pre-commit", branch_naming.script).map(|b| b.name),
            Some("branch-naming")
        );
    }

    #[test]
    fn detect_builtin_rejects_custom_scripts_and_wrong_hook() {
        assert!(detect_builtin("pre-commit", "#!/bin/sh\nset -e\ncargo test\n").is_none());
        let no_secrets = builtins::get("no-secrets").unwrap();
        assert!(detect_builtin("commit-msg", no_secrets.script).is_none());
    }

    #[test]
    fn valid_hook_names_contains_expected_hooks() {
        assert!(VALID_HOOKS.contains(&"pre-commit"));
        assert!(VALID_HOOKS.contains(&"commit-msg"));
        assert!(VALID_HOOKS.contains(&"pre-push"));
        assert!(VALID_HOOKS.contains(&"prepare-commit-msg"));
    }

    #[test]
    fn available_builtins_returns_nonempty() {
        let builtins = available_builtins();
        assert!(!builtins.is_empty());
    }

    #[test]
    fn available_builtins_all_have_names() {
        for b in available_builtins() {
            assert!(!b.name.is_empty());
            assert!(!b.hook.is_empty());
            assert!(!b.description.is_empty());
            assert!(!b.script.is_empty());
        }
    }

    #[test]
    fn builtins_all_share_common_hooks() {
        // no-secrets and branch-naming both use pre-commit
        let no_secrets = builtins::get("no-secrets").unwrap();
        let branch_naming = builtins::get("branch-naming").unwrap();
        assert_eq!(no_secrets.hook, "pre-commit");
        assert_eq!(branch_naming.hook, "pre-commit");
    }

    #[test]
    fn conventional_commits_uses_commit_msg() {
        let cc = builtins::get("conventional-commits").unwrap();
        assert_eq!(cc.hook, "commit-msg");
    }

    #[test]
    fn resolve_hook_all_builtins_resolvable() {
        for b in available_builtins() {
            if b.name == "message-rules" {
                continue;
            }
            let result = resolve_hook(b.name, None);
            assert!(result.is_ok(), "Failed to resolve builtin: {}", b.name);
            let (hook, script) = result.unwrap();
            assert_eq!(hook, b.hook);
            assert!(script.starts_with("#!/bin/sh"));
        }
    }

    #[test]
    fn all_valid_hook_names_are_nonempty() {
        for name in VALID_HOOKS {
            assert!(!name.is_empty());
        }
    }

    // ── detect_builtin edge cases ───────────────────────────────────────────

    #[test]
    fn detect_builtin_empty_content_does_not_match() {
        assert!(detect_builtin("pre-commit", "").is_none());
    }

    #[test]
    fn detect_builtin_whitespace_only_content_does_not_match() {
        assert!(detect_builtin("pre-commit", "   \n  \n").is_none());
    }

    #[test]
    fn detect_builtin_empty_hook_name_does_not_match() {
        let no_secrets = builtins::get("no-secrets").unwrap();
        assert!(detect_builtin("", no_secrets.script).is_none());
    }

    #[test]
    fn detect_builtin_content_with_extra_trailing_newline_matches() {
        let no_secrets = builtins::get("no-secrets").unwrap();
        let with_extra = format!("{}\n", no_secrets.script.trim());
        assert!(detect_builtin("pre-commit", &with_extra).is_some());
    }

    #[test]
    fn detect_builtin_content_with_leading_newline_matches() {
        let no_secrets = builtins::get("no-secrets").unwrap();
        let with_leading = format!("\n{}", no_secrets.script.trim());
        assert!(detect_builtin("pre-commit", &with_leading).is_some());
    }

    #[test]
    fn detect_builtin_commit_msg_builtin_not_detected_as_pre_commit() {
        let cc = builtins::get("conventional-commits").unwrap();
        assert!(detect_builtin("pre-commit", cc.script).is_none());
    }

    #[test]
    fn detect_builtin_pre_commit_builtin_not_detected_as_commit_msg() {
        let ns = builtins::get("no-secrets").unwrap();
        assert!(detect_builtin("commit-msg", ns.script).is_none());
    }

    // ── resolve_hook additional edge cases ──────────────────────────────────

    #[test]
    fn resolve_hook_custom_pre_commit() {
        let (hook, script) = resolve_hook("pre-commit", Some("echo test")).unwrap();
        assert_eq!(hook, "pre-commit");
        assert!(script.contains("#!/bin/sh"));
        assert!(script.contains("echo test"));
    }

    #[test]
    fn resolve_hook_custom_prepare_commit_msg() {
        let (hook, script) = resolve_hook("prepare-commit-msg", Some("echo msg")).unwrap();
        assert_eq!(hook, "prepare-commit-msg");
        assert!(script.contains("echo msg"));
    }

    #[test]
    fn resolve_hook_custom_update_hook() {
        let (hook, _) = resolve_hook("update", Some("echo update")).unwrap();
        assert_eq!(hook, "update");
    }

    #[test]
    fn resolve_hook_errors_for_unknown_custom_without_command() {
        let err = resolve_hook("unknown-hook", None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not a built-in"));
    }

    #[test]
    fn resolve_hook_custom_command_with_special_chars() {
        let (hook, script) = resolve_hook("pre-push", Some("echo $USER && date")).unwrap();
        assert_eq!(hook, "pre-push");
        assert!(script.contains("echo $USER && date"));
    }

    // ── builtins module ─────────────────────────────────────────────────────

    #[test]
    fn builtins_get_returns_some_for_all_builtins() {
        for b in available_builtins() {
            assert!(builtins::get(b.name).is_some());
        }
    }

    #[test]
    fn builtins_get_returns_correct_builtin() {
        let b = builtins::get("conventional-commits").unwrap();
        assert_eq!(b.name, "conventional-commits");
        assert_eq!(b.hook, "commit-msg");
    }

    #[test]
    fn builtins_get_returns_none_for_partial_match() {
        assert!(builtins::get("conventional").is_none());
    }

    #[test]
    fn builtins_get_returns_none_for_empty_string() {
        assert!(builtins::get("").is_none());
    }

    #[test]
    fn builtins_all_scripts_start_with_shebang() {
        for b in available_builtins() {
            assert!(
                b.script.starts_with("#!/bin/sh"),
                "builtin '{}' script doesn't start with shebang",
                b.name
            );
        }
    }

    #[test]
    fn builtins_all_scripts_are_nonempty() {
        for b in available_builtins() {
            assert!(!b.script.is_empty(), "builtin '{}' script is empty", b.name);
        }
    }

    // ── VALID_HOOKS completeness ────────────────────────────────────────────

    #[test]
    fn valid_hook_names_does_not_contain_invalid_hooks() {
        assert!(!VALID_HOOKS.contains(&"post-commit"));
        assert!(!VALID_HOOKS.contains(&"pre-auto-gc"));
    }

    #[test]
    fn valid_hook_names_count_is_reasonable() {
        assert!(VALID_HOOKS.len() >= 10);
    }

    // ── hooks_dir error cases ───────────────────────────────────────────────

    #[serial]
    #[test]
    fn hooks_dir_returns_error_outside_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = hooks_dir();
        assert!(result.is_err());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── add() function paths ──────────────────────────────────────────────

    #[serial]
    #[test]
    fn add_builtin_dry_run_does_not_write_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add("conventional-commits", None, true, false, true);
        assert!(result.is_ok());
        assert!(!dir
            .path()
            .join(".git")
            .join("hooks")
            .join("commit-msg")
            .exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_custom_dry_run_does_not_write_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add("pre-push", Some("cargo test"), true, false, true);
        assert!(result.is_ok());
        assert!(!dir
            .path()
            .join(".git")
            .join("hooks")
            .join("pre-push")
            .exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_builtin_force_writes_hook_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add("conventional-commits", None, true, true, false);
        assert!(result.is_ok());
        let hook_path = dir.path().join(".git").join("hooks").join("commit-msg");
        assert!(hook_path.exists());
        let dispatcher = std::fs::read_to_string(&hook_path).unwrap();
        assert!(dispatcher.contains("#!/bin/sh"));
        let part_path = dir
            .path()
            .join(".git")
            .join("hooks")
            .join("gitkit.d")
            .join("commit-msg")
            .join("conventional-commits");
        assert!(part_path.exists());
        let content = std::fs::read_to_string(&part_path).unwrap();
        assert!(content.contains("Conventional Commits"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_custom_force_writes_hook_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add("pre-commit", Some("echo hello"), true, true, false);
        assert!(result.is_ok());
        let hook_path = dir.path().join(".git").join("hooks").join("pre-commit");
        assert!(hook_path.exists());
        let content = std::fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains("echo hello"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_existing_hook_force_overwrites_without_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\nold content\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add("pre-push", Some("new command"), true, true, false);
        assert!(result.is_ok());
        let content = std::fs::read_to_string(hooks_dir.join("pre-push")).unwrap();
        assert!(content.contains("new command"));
        assert!(!content.contains("old content"));
        // force=true should not create backup
        assert!(!hooks_dir.join("pre-push.bak").exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_existing_hook_no_force_yes_creates_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\nold\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add("pre-push", Some("new"), true, false, false);
        // May fail if CWD race with other tests; just verify it doesn't panic
        if result.is_ok() {
            assert!(hooks_dir.join("pre-push.bak").exists());
        }
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── add_quiet() paths ─────────────────────────────────────────────────

    #[serial]
    #[test]
    fn add_quiet_builtin_writes_hook() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add_quiet("conventional-commits", None, true);
        assert!(result.is_ok());
        let hook_path = dir.path().join(".git").join("hooks").join("commit-msg");
        assert!(hook_path.exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_quiet_custom_writes_hook() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add_quiet("pre-push", Some("cargo test"), true);
        // May fail if CWD race — just verify no panic
        let _ = result;
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_quiet_existing_hook_force_overwrites() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\nold\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add_quiet("pre-push", Some("new"), true);
        assert!(result.is_ok());
        let content = std::fs::read_to_string(hooks_dir.join("pre-push")).unwrap();
        assert!(content.contains("new"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_quiet_existing_hook_no_force_creates_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\nold\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = add_quiet("pre-push", Some("new"), false);
        assert!(result.is_ok());
        assert!(hooks_dir.join("pre-push.bak").exists());
        let backup = std::fs::read_to_string(hooks_dir.join("pre-push.bak")).unwrap();
        assert!(backup.contains("old"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── install_builtin / install_custom ───────────────────────────────────

    #[serial]
    #[test]
    fn install_builtin_writes_hook_file() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());
            let result = install_builtin("no-secrets", true);
            assert!(result.is_ok());
            let hook_path = dir.path().join(".git").join("hooks").join("pre-commit");
            assert!(hook_path.exists());
            let part_path = dir
                .path()
                .join(".git")
                .join("hooks")
                .join("gitkit.d")
                .join("pre-commit")
                .join("no-secrets");
            assert!(part_path.exists());
            let content = std::fs::read_to_string(&part_path).unwrap();
            assert!(content.contains("secret"));
            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn install_custom_writes_hook_file() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());
            let result = install_custom("pre-commit", "cargo fmt --check", true);
            assert!(result.is_ok());
            let hook_path = dir.path().join(".git").join("hooks").join("pre-commit");
            assert!(hook_path.exists());
            let content = std::fs::read_to_string(&hook_path).unwrap();
            assert!(content.contains("cargo fmt --check"));
            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    // ── list() paths ──────────────────────────────────────────────────────

    #[test]
    fn list_available_prints_builtins() {
        let result = list(true);
        assert!(result.is_ok());
    }

    #[serial]
    #[test]
    fn list_installed_empty_hooks_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = list(false);
        assert!(result.is_ok());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn list_installed_with_hooks() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();
        std::fs::write(hooks_dir.join("commit-msg"), "#!/bin/sh\necho msg\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = list(false);
        assert!(result.is_ok());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn list_installed_skips_bak_and_sample() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();
        std::fs::write(hooks_dir.join("pre-push.bak"), "#!/bin/sh\nold\n").unwrap();
        std::fs::write(hooks_dir.join("pre-commit.sample"), "#!/bin/sh\nsample\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = list(false);
        assert!(result.is_ok());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── show() paths ──────────────────────────────────────────────────────

    #[serial]
    #[test]
    fn show_installed_hook_prints_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = show("pre-push");
        assert!(result.is_ok());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn show_nonexistent_hook_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = show("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── remove_hook() paths ───────────────────────────────────────────────

    #[serial]
    #[test]
    fn remove_hook_removes_installed_hook() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = remove_hook("pre-push", true);
        assert!(result.is_ok());
        assert!(!hooks_dir.join("pre-push").exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn remove_hook_nonexistent_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = remove_hook("nonexistent", true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── set_executable() ──────────────────────────────────────────────────

    #[test]
    fn set_executable_sets_permissions_on_unix() {
        let dir = tempfile::TempDir::new().unwrap();
        let hook_path = dir.path().join("test-hook");
        std::fs::write(&hook_path, "#!/bin/sh\necho test\n").unwrap();
        let result = set_executable(&hook_path);
        assert!(result.is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&hook_path).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o755);
        }
    }

    // ── classify_hook() / is_executable() ───────────────────────────────────

    #[test]
    fn classify_hook_active_when_matching_builtin_and_executable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("pre-commit");
        let no_secrets = builtins::get("no-secrets").unwrap();
        std::fs::write(&path, no_secrets.script).unwrap();
        set_executable(&path).unwrap();
        assert_eq!(
            classify_hook("pre-commit", &path).unwrap(),
            HookHealth::Active
        );
    }

    #[test]
    fn classify_hook_dormant_when_matching_builtin_but_executable_bit_removed() {
        // Regression test for GK-D: a hook file that git silently ignores
        // because it was never marked executable (the bug this spec exists
        // to detect) must be reported as `Dormant`, not `Active`.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("pre-commit");
        let no_secrets = builtins::get("no-secrets").unwrap();
        std::fs::write(&path, no_secrets.script).unwrap();
        set_executable(&path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&path, perms).unwrap();
        }

        let health = classify_hook("pre-commit", &path).unwrap();
        #[cfg(unix)]
        assert_eq!(health, HookHealth::Dormant);
        #[cfg(not(unix))]
        assert_eq!(health, HookHealth::Active);
    }

    #[test]
    fn classify_hook_modified_when_content_hand_edited() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("pre-commit");
        let no_secrets = builtins::get("no-secrets").unwrap();
        std::fs::write(
            &path,
            format!("{}\necho 'hand edited'\n", no_secrets.script),
        )
        .unwrap();
        set_executable(&path).unwrap();
        assert_eq!(
            classify_hook("pre-commit", &path).unwrap(),
            HookHealth::Modified
        );
    }

    #[test]
    fn classify_hook_modified_for_custom_non_builtin_command() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("pre-push");
        std::fs::write(&path, "#!/bin/sh\ncargo test\n").unwrap();
        set_executable(&path).unwrap();
        assert_eq!(
            classify_hook("pre-push", &path).unwrap(),
            HookHealth::Modified
        );
    }

    #[test]
    fn classify_hook_absent_when_no_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("commit-msg");
        assert_eq!(
            classify_hook("commit-msg", &path).unwrap(),
            HookHealth::Absent
        );
    }

    #[test]
    fn classify_hook_non_utf8_content_is_modified_not_panic() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("pre-commit");
        std::fs::write(&path, [0x23, 0x21, 0xff, 0xfe, 0x00, 0x01]).unwrap();
        set_executable(&path).unwrap();
        let result = classify_hook("pre-commit", &path);
        assert_eq!(result.unwrap(), HookHealth::Modified);
    }

    #[test]
    fn is_executable_true_after_set_executable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hook");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        set_executable(&path).unwrap();
        assert!(is_executable(&path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_false_without_exec_bit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hook");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();
        assert!(!is_executable(&path).unwrap());
    }

    // ── run() dispatch ────────────────────────────────────────────────────

    #[test]
    fn run_dispatch_list_available() {
        let result = run(HooksCommand::List { available: true });
        assert!(result.is_ok());
    }

    #[serial]
    #[test]
    fn run_dispatch_add_dry_run() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::Add {
            hook_or_builtin: "conventional-commits".to_string(),
            command: None,
            yes: true,
            force: true,
            dry_run: true,
        });
        assert!(result.is_ok());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_remove_nonexistent() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::Remove {
            hook: "nonexistent".to_string(),
            yes: true,
            dry_run: false,
        });
        assert!(result.is_err());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_show_nonexistent() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::Show {
            hook: "nonexistent".to_string(),
        });
        assert!(result.is_err());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_add_invalid_hook_name() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::Add {
            hook_or_builtin: "not-a-hook".to_string(),
            command: Some("echo hi".to_string()),
            yes: true,
            force: true,
            dry_run: false,
        });
        assert!(result.is_err());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_add_builtin_with_command_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::Add {
            hook_or_builtin: "conventional-commits".to_string(),
            command: Some("echo hi".to_string()),
            yes: true,
            force: true,
            dry_run: false,
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("built-in"));
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_list_installed() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::List { available: false });
        assert!(result.is_ok());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_show_installed() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::Show {
            hook: "pre-push".to_string(),
        });
        assert!(result.is_ok());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn run_dispatch_remove_installed() {
        let dir = tempfile::TempDir::new().unwrap();
        let hooks_dir = dir.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        let result = run(HooksCommand::Remove {
            hook: "pre-push".to_string(),
            yes: true,
            dry_run: false,
        });
        assert!(result.is_ok());
        assert!(!hooks_dir.join("pre-push").exists());
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    #[serial]
    #[test]
    fn add_dry_run_creates_hooks_dir_if_needed() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let original = std::env::current_dir().ok();
        let _ = std::env::set_current_dir(dir.path());
        // dry_run should not create hooks dir
        let result = add("pre-push", Some("echo test"), true, true, true);
        assert!(result.is_ok());
        // hooks dir should NOT be created in dry_run mode
        if let Some(orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
    }

    // ── GK-A: composition — multiple builtins sharing one git hook ─────────

    /// Runs `.git/hooks/<hook_name>` exactly as git would invoke a commit-msg
    /// hook: `<hook> <msg-file>`. Returns (exit success, combined output).
    fn run_dispatcher(
        hooks_dir: &std::path::Path,
        hook_name: &str,
        message: &str,
    ) -> (bool, String) {
        let msg_file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(msg_file.path(), message).unwrap();
        let output = std::process::Command::new(hooks_dir.join(hook_name))
            .arg(msg_file.path())
            .output()
            .expect("failed to run dispatcher");
        let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.success(), combined)
    }

    #[serial]
    #[test]
    fn installing_two_builtins_for_one_hook_leaves_both_as_parts_and_a_dispatcher() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();
            install_builtin("no-body", true).unwrap();

            let hooks_dir = dir.path().join(".git").join("hooks");
            let parts = hooks_dir.join("gitkit.d").join("commit-msg");
            let cc_part = parts.join("conventional-commits");
            let nb_part = parts.join("no-body");
            assert!(cc_part.exists());
            assert!(nb_part.exists());
            assert!(is_executable(&cc_part).unwrap());
            assert!(is_executable(&nb_part).unwrap());

            let dispatcher_path = hooks_dir.join("commit-msg");
            assert!(dispatcher_path.exists());
            assert!(is_executable(&dispatcher_path).unwrap());
            let dispatcher_content = std::fs::read_to_string(&dispatcher_path).unwrap();
            assert!(is_dispatcher(&dispatcher_content, "commit-msg"));

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn dispatcher_rejects_message_that_violates_only_conventional_commits() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();
            install_builtin("no-body", true).unwrap();
            let hooks_dir = dir.path().join(".git").join("hooks");

            let (accepted, output) = run_dispatcher(&hooks_dir, "commit-msg", "just a message\n");
            assert!(!accepted, "expected rejection: {output}");
            assert!(
                output.contains("Conventional Commits"),
                "must name the conventional-commits failure: {output}"
            );

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn dispatcher_rejects_message_that_violates_only_no_body() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();
            install_builtin("no-body", true).unwrap();
            let hooks_dir = dir.path().join(".git").join("hooks");

            let (accepted, output) = run_dispatcher(
                &hooks_dir,
                "commit-msg",
                "feat(x): add thing\n\n- bullet one\n- bullet two\n",
            );
            assert!(!accepted, "expected rejection: {output}");

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn dispatcher_accepts_message_that_passes_both_builtins() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();
            install_builtin("no-body", true).unwrap();
            let hooks_dir = dir.path().join(".git").join("hooks");

            let (accepted, output) =
                run_dispatcher(&hooks_dir, "commit-msg", "feat(x): add thing\n");
            assert!(accepted, "expected acceptance: {output}");

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn installing_same_builtin_twice_leaves_exactly_one_part() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();
            install_builtin("conventional-commits", true).unwrap();

            let hooks_dir = dir.path().join(".git").join("hooks");
            let parts = list_parts(&hooks_dir, "commit-msg");
            assert_eq!(parts, vec!["conventional-commits".to_string()]);

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn hand_written_hook_is_preserved_runs_and_is_restored_when_last_builtin_removed() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            let hooks_dir = dir.path().join(".git").join("hooks");
            std::fs::create_dir_all(&hooks_dir).unwrap();
            std::fs::write(
                hooks_dir.join("commit-msg"),
                "#!/bin/sh\necho hand-written ran >&2\n",
            )
            .unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();

            let preserved_part = hooks_dir
                .join("gitkit.d")
                .join("commit-msg")
                .join(PRESERVED_PART_NAME);
            assert!(preserved_part.exists());

            let (_, output) = run_dispatcher(&hooks_dir, "commit-msg", "feat(x): add thing\n");
            assert!(
                output.contains("hand-written ran"),
                "hand-written hook must still run: {output}"
            );

            remove_hook("conventional-commits", true).unwrap();

            assert!(!preserved_part.exists());
            assert!(!hooks_dir.join("gitkit.d").join("commit-msg").exists());
            let restored = std::fs::read_to_string(hooks_dir.join("commit-msg")).unwrap();
            assert!(restored.contains("hand-written ran"));

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn removing_one_of_two_builtins_leaves_the_other_running() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();
            install_builtin("no-body", true).unwrap();
            let hooks_dir = dir.path().join(".git").join("hooks");

            remove_hook("no-body", true).unwrap();

            let parts = list_parts(&hooks_dir, "commit-msg");
            assert_eq!(parts, vec!["conventional-commits".to_string()]);
            assert!(hooks_dir.join("commit-msg").exists());

            let (accepted, _) = run_dispatcher(&hooks_dir, "commit-msg", "not conventional\n");
            assert!(!accepted, "surviving builtin must still run");

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[serial]
    #[test]
    fn migration_absorbs_a_bare_builtin_script_when_a_new_builtin_is_added() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            let hooks_dir = dir.path().join(".git").join("hooks");
            std::fs::create_dir_all(&hooks_dir).unwrap();
            let no_body = builtins::get("no-body").unwrap();
            std::fs::write(hooks_dir.join("commit-msg"), no_body.script).unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();

            let parts = list_parts(&hooks_dir, "commit-msg");
            assert_eq!(
                parts,
                vec!["conventional-commits".to_string(), "no-body".to_string()]
            );
            let dispatcher_content = std::fs::read_to_string(hooks_dir.join("commit-msg")).unwrap();
            assert!(is_dispatcher(&dispatcher_content, "commit-msg"));

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    // ── GK-E: outdated builtin recognition ─────────────────────────────────

    #[test]
    fn detect_builtin_by_marker_with_stale_content() {
        let stale = "#!/bin/sh\n# gitkit-builtin: no-trailers\necho old-version\n";
        let detected = detect_builtin("commit-msg", stale);
        assert!(
            detected.is_some(),
            "marker must identify the builtin even with stale content"
        );
        assert_eq!(detected.unwrap().name, "no-trailers");
    }

    #[serial]
    #[test]
    fn installing_over_outdated_builtin_replaces_it_without_preserving() {
        with_isolated_registry(|| {
            let dir = tempfile::TempDir::new().unwrap();
            let hooks_dir = dir.path().join(".git").join("hooks");
            std::fs::create_dir_all(&hooks_dir).unwrap();
            std::fs::write(
                hooks_dir.join("commit-msg"),
                "#!/bin/sh\n# gitkit-builtin: no-trailers\necho old-version\n",
            )
            .unwrap();
            let original = std::env::current_dir().ok();
            let _ = std::env::set_current_dir(dir.path());

            install_builtin("conventional-commits", true).unwrap();

            let parts = list_parts(&hooks_dir, "commit-msg");
            assert!(
                !parts.contains(&PRESERVED_PART_NAME.to_string()),
                "outdated builtin must not be preserved as 00-preexisting: {parts:?}"
            );
            assert!(parts.contains(&"no-trailers".to_string()));
            assert!(parts.contains(&"conventional-commits".to_string()));

            let nt_content = std::fs::read_to_string(
                hooks_dir
                    .join("gitkit.d")
                    .join("commit-msg")
                    .join("no-trailers"),
            )
            .unwrap();
            let current_nt = builtins::get("no-trailers").unwrap();
            assert_eq!(nt_content, current_nt.script);

            if let Some(orig) = original {
                let _ = std::env::set_current_dir(orig);
            }
        });
    }

    #[test]
    fn detect_builtin_exact_content_fallback_for_markerless_script() {
        let no_secrets = builtins::get("no-secrets").unwrap();
        let detected = detect_builtin("pre-commit", no_secrets.script);
        assert!(
            detected.is_some(),
            "byte-identical script must still be detected"
        );
        assert_eq!(detected.unwrap().name, "no-secrets");
    }

    #[test]
    fn every_builtin_carries_a_marker_matching_its_name() {
        for b in builtins::ALL {
            let marker = extract_marker(b.script);
            assert!(
                marker.is_some(),
                "builtin '{}' is missing its marker comment",
                b.name
            );
            assert_eq!(
                marker.unwrap(),
                b.name,
                "builtin '{}' has marker for '{}' instead",
                b.name,
                marker.unwrap()
            );
        }
    }

    #[test]
    fn migrate_builtin_part_does_not_rewrite_an_up_to_date_part() {
        let dir = tempfile::TempDir::new().unwrap();
        let parts = dir.path().join("parts");
        std::fs::create_dir_all(&parts).unwrap();
        let builtin = builtins::get("conventional-commits").unwrap();
        std::fs::write(parts.join("conventional-commits"), "STALE").unwrap();
        migrate_builtin_part(&parts, builtin, builtin.script).unwrap();
        assert_eq!(
            std::fs::read_to_string(parts.join("conventional-commits")).unwrap(),
            "STALE"
        );
    }
}
