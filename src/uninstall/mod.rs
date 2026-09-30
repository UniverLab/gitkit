use anyhow::{Context, Result};
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};

use crate::hooks;
use crate::registry;

#[derive(Args)]
pub struct UninstallArgs {
    /// Also remove local state under ~/.gitkit (builds, registry)
    #[arg(long)]
    pub data: bool,

    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub yes: bool,

    /// Print what would be done without changing anything
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(args: UninstallArgs) -> Result<()> {
    let plan = build_plan(args.data)?;

    print_plan(&plan);

    if plan.is_empty() {
        println!("\nNothing to uninstall.");
        print_binary_note();
        return Ok(());
    }

    if args.dry_run {
        println!("\n[dry-run] No changes made.");
        return Ok(());
    }

    if !args.yes && !crate::utils::confirm("\nProceed with uninstall?", false) {
        println!("Aborted.");
        return Ok(());
    }

    execute_plan(&plan)?;

    if plan.remove_local_data {
        remove_local_data()?;
    }

    println!("\nUninstall complete.");
    print_binary_note();
    Ok(())
}

fn print_binary_note() {
    println!("\nThe gitkit binary was not removed.");
    if let Some(method) = detect_install_method() {
        println!("It was installed via {method} — remove it with that tool's uninstall command.");
    } else {
        println!("Remove it manually from wherever it was installed.");
    }
}

fn detect_install_method() -> Option<&'static str> {
    let exe = std::env::current_exe().ok()?;
    let exe_str = exe.to_string_lossy();
    if exe_str.contains(".cargo/bin") {
        Some("cargo")
    } else if exe_str.contains(".local/bin") || exe_str.contains("/usr/local/bin") {
        Some("the install script")
    } else if exe_str.contains("Homebrew") || exe_str.contains("homebrew") {
        Some("Homebrew")
    } else {
        None
    }
}

// ── Plan ─────────────────────────────────────────────────────────────────────

struct UninstallPlan {
    repos: Vec<RepoPlan>,
    remove_local_data: bool,
}

impl UninstallPlan {
    fn is_empty(&self) -> bool {
        self.repos.iter().all(|r| !r.exists || r.hooks.is_empty()) && !self.remove_local_data
    }
}

struct RepoPlan {
    path: String,
    exists: bool,
    hooks: Vec<HookPlan>,
}

struct HookPlan {
    hook_name: String,
    has_dispatcher: bool,
    parts: Vec<String>,
    has_preexisting: bool,
}

fn build_plan(include_data: bool) -> Result<UninstallPlan> {
    let reg = registry::load();

    let repos = reg
        .repos
        .iter()
        .map(|(key, entry)| {
            let repo_path = PathBuf::from(&entry.path);
            RepoPlan {
                path: key.clone(),
                exists: repo_path.exists(),
                hooks: plan_hooks_for_repo(&repo_path),
            }
        })
        .collect();

    Ok(UninstallPlan {
        repos,
        remove_local_data: include_data,
    })
}

/// The hooks worth uninstalling in one registered repository: nothing when
/// the repository or its `.git/hooks` directory is gone.
fn plan_hooks_for_repo(repo_path: &Path) -> Vec<HookPlan> {
    let hooks_dir = repo_path.join(".git").join("hooks");
    if !hooks_dir.exists() {
        return Vec::new();
    }

    hooks::valid_hook_names()
        .iter()
        .filter_map(|hook_name| plan_hook(&hooks_dir, hook_name))
        .collect()
}

/// One hook of one registered repository, or `None` when there is nothing
/// gitkit-owned to remove: no file, or a hook someone replaced by hand.
fn plan_hook(hooks_dir: &Path, hook_name: &str) -> Option<HookPlan> {
    let dispatcher_path = hooks_dir.join(hook_name);
    if !dispatcher_path.exists() {
        return None;
    }

    let content = fs::read_to_string(&dispatcher_path).unwrap_or_default();
    if !hooks::is_dispatcher(&content, hook_name) {
        return None;
    }

    let parts = hooks::list_parts(hooks_dir, hook_name);
    let has_preexisting = parts.iter().any(|p| p == hooks::PRESERVED_PART_NAME);

    Some(HookPlan {
        hook_name: hook_name.to_string(),
        has_dispatcher: true,
        parts,
        has_preexisting,
    })
}

fn print_plan(plan: &UninstallPlan) {
    println!("gitkit uninstall will:");

    let mut any_repo = false;
    for repo in &plan.repos {
        if !repo.exists {
            println!("\n  {} — repository no longer exists, skipping", repo.path);
            continue;
        }
        if repo.hooks.is_empty() {
            println!("\n  {} — no gitkit hooks found, skipping", repo.path);
            continue;
        }

        any_repo = true;
        println!("\n  {}", repo.path);
        for hook in &repo.hooks {
            if hook.has_preexisting {
                println!(
                    "    restore .git/hooks/{} from absorbed hand-written hook",
                    hook.hook_name
                );
            }
            if hook.has_dispatcher {
                println!(
                    "    remove .git/hooks/{} (gitkit dispatcher)",
                    hook.hook_name
                );
            }
            if !hook.parts.is_empty() {
                println!(
                    "    remove .git/hooks/gitkit.d/{}/ ({} part{})",
                    hook.hook_name,
                    hook.parts.len(),
                    if hook.parts.len() == 1 { "" } else { "s" }
                );
            }
        }
    }

    if !any_repo && plan.repos.is_empty() {
        println!("  (no repositories in registry)");
    }

    if plan.remove_local_data {
        println!("\n  remove ~/.gitkit/ (local state: builds, registry)");
    } else {
        println!("\n  keep ~/.gitkit/ (use --data to also remove local state)");
    }
}

// ── Execute ──────────────────────────────────────────────────────────────────

fn execute_plan(plan: &UninstallPlan) -> Result<()> {
    for repo in &plan.repos {
        if !repo.exists {
            continue;
        }
        if repo.hooks.is_empty() {
            continue;
        }

        let repo_path = PathBuf::from(&repo.path);
        if let Err(e) = execute_repo_plan(&repo_path, &repo.hooks) {
            eprintln!("Warning: failed to clean {}: {e}", repo.path);
        }
    }

    remove_registry_entries(plan)?;

    Ok(())
}

fn execute_repo_plan(repo_path: &Path, hooks: &[HookPlan]) -> Result<()> {
    let hooks_dir = repo_path.join(".git").join("hooks");

    for hook in hooks {
        if let Err(e) = execute_hook_plan(&hooks_dir, hook) {
            eprintln!(
                "Warning: failed to clean hook {} in {}: {e}",
                hook.hook_name,
                repo_path.display()
            );
        }
    }

    Ok(())
}

fn execute_hook_plan(hooks_dir: &Path, hook: &HookPlan) -> Result<()> {
    let dispatcher_path = hooks_dir.join(&hook.hook_name);
    let parts_dir = hooks::parts_dir(hooks_dir, &hook.hook_name);

    if hook.has_preexisting {
        let preexisting_path = parts_dir.join(hooks::PRESERVED_PART_NAME);
        if preexisting_path.exists() {
            let content = fs::read(&preexisting_path)
                .with_context(|| format!("Failed to read preserved hook for {}", hook.hook_name))?;
            fs::write(&dispatcher_path, &content)
                .with_context(|| format!("Failed to restore original {} hook", hook.hook_name))?;
            hooks::set_executable(&dispatcher_path)?;
        }
    }

    if parts_dir.exists() {
        let _ = fs::remove_dir_all(&parts_dir);
    }

    if hook.has_dispatcher && dispatcher_path.exists() {
        let content = fs::read_to_string(&dispatcher_path).unwrap_or_default();
        if hooks::is_dispatcher(&content, &hook.hook_name) {
            fs::remove_file(&dispatcher_path)
                .with_context(|| format!("Failed to remove dispatcher for {}", hook.hook_name))?;
        }
    }

    Ok(())
}

fn remove_registry_entries(plan: &UninstallPlan) -> Result<()> {
    let mut reg = registry::load();
    for repo in &plan.repos {
        reg.repos.remove(&repo.path);
    }
    registry::save(&reg)
}

fn remove_local_data() -> Result<()> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("Neither HOME nor USERPROFILE environment variable is set")?;
    let gitkit_dir = PathBuf::from(home).join(".gitkit");
    if gitkit_dir.exists() {
        fs::remove_dir_all(&gitkit_dir).context("Failed to remove ~/.gitkit")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn with_temp_home<F: FnOnce(&Path)>(f: F) {
        let dir = tempfile::TempDir::new().unwrap();
        let original = std::env::var("HOME").ok();
        let orig_gitkit_home = std::env::var("GITKIT_HOME").ok();
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("GITKIT_HOME", dir.path().join(".gitkit"));
        }
        f(dir.path());
        unsafe {
            match &original {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            match &orig_gitkit_home {
                Some(h) => std::env::set_var("GITKIT_HOME", h),
                None => std::env::remove_var("GITKIT_HOME"),
            }
        }
    }

    fn init_repo(path: &Path) {
        fs::create_dir_all(path.join(".git").join("hooks")).unwrap();
    }

    fn install_dispatcher(hooks_dir: &Path, hook_name: &str) {
        let script = hooks::dispatcher_script(hook_name);
        let hook_path = hooks_dir.join(hook_name);
        fs::write(&hook_path, &script).unwrap();
        hooks::set_executable(&hook_path).unwrap();
    }

    fn install_part(hooks_dir: &Path, hook_name: &str, part_name: &str, content: &str) {
        let parts_dir = hooks::parts_dir(hooks_dir, hook_name);
        fs::create_dir_all(&parts_dir).unwrap();
        let part_path = parts_dir.join(part_name);
        fs::write(&part_path, content).unwrap();
        hooks::set_executable(&part_path).unwrap();
    }

    #[serial]
    #[test]
    fn uninstall_removes_dispatcher_and_parts() {
        with_temp_home(|_home| {
            let repo = tempfile::TempDir::new().unwrap();
            init_repo(repo.path());
            let hooks_dir = repo.path().join(".git").join("hooks");
            install_dispatcher(&hooks_dir, "pre-commit");
            let builtin = hooks::builtins::get("no-secrets").unwrap();
            install_part(&hooks_dir, "pre-commit", "no-secrets", builtin.script);

            registry::record(repo.path(), &["hook:no-secrets".to_string()]).unwrap();

            let plan = build_plan(false).unwrap();
            assert_eq!(plan.repos.len(), 1);
            assert_eq!(plan.repos[0].hooks.len(), 1);
            assert!(plan.repos[0].hooks[0].has_dispatcher);

            execute_plan(&plan).unwrap();

            assert!(!hooks_dir.join("pre-commit").exists());
            assert!(!hooks::parts_dir(&hooks_dir, "pre-commit").exists());
        });
    }

    #[serial]
    #[test]
    fn uninstall_restores_preexisting_hook() {
        with_temp_home(|_home| {
            let repo = tempfile::TempDir::new().unwrap();
            init_repo(repo.path());
            let hooks_dir = repo.path().join(".git").join("hooks");

            let original_hook = "#!/bin/sh\necho my custom hook\n";
            install_dispatcher(&hooks_dir, "pre-commit");
            install_part(
                &hooks_dir,
                "pre-commit",
                hooks::PRESERVED_PART_NAME,
                original_hook,
            );
            let builtin = hooks::builtins::get("no-secrets").unwrap();
            install_part(&hooks_dir, "pre-commit", "no-secrets", builtin.script);

            registry::record(repo.path(), &["hook:no-secrets".to_string()]).unwrap();

            let plan = build_plan(false).unwrap();
            assert!(plan.repos[0].hooks[0].has_preexisting);

            execute_plan(&plan).unwrap();

            let restored = hooks_dir.join("pre-commit");
            assert!(restored.exists());
            let content = fs::read_to_string(&restored).unwrap();
            assert_eq!(content, original_hook);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::metadata(&restored).unwrap().permissions();
                assert!(
                    perms.mode() & 0o111 != 0,
                    "restored hook should be executable"
                );
            }

            assert!(!hooks::parts_dir(&hooks_dir, "pre-commit").exists());
        });
    }

    #[serial]
    #[test]
    fn uninstall_skips_missing_repo() {
        with_temp_home(|_home| {
            let fake_path = "/tmp/nonexistent-repo-gitkit-test-12345";
            let mut reg = registry::load();
            reg.repos.insert(
                fake_path.to_string(),
                registry::RegistryEntry {
                    path: fake_path.to_string(),
                    applied_at: "2026-01-01T00:00:00Z".to_string(),
                    applied: vec!["hook:no-secrets".to_string()],
                },
            );
            registry::save(&reg).unwrap();

            let plan = build_plan(false).unwrap();
            assert_eq!(plan.repos.len(), 1);
            assert!(!plan.repos[0].exists);

            execute_plan(&plan).unwrap();

            let reg_after = registry::load();
            assert!(!reg_after.repos.contains_key(fake_path));
        });
    }

    #[serial]
    #[test]
    fn dry_run_does_not_change_disk() {
        with_temp_home(|_home| {
            let repo = tempfile::TempDir::new().unwrap();
            init_repo(repo.path());
            let hooks_dir = repo.path().join(".git").join("hooks");
            install_dispatcher(&hooks_dir, "pre-commit");
            let builtin = hooks::builtins::get("no-secrets").unwrap();
            install_part(&hooks_dir, "pre-commit", "no-secrets", builtin.script);

            registry::record(repo.path(), &["hook:no-secrets".to_string()]).unwrap();

            let args = UninstallArgs {
                data: false,
                yes: false,
                dry_run: true,
            };
            run(args).unwrap();

            assert!(hooks_dir.join("pre-commit").exists());
            assert!(hooks::parts_dir(&hooks_dir, "pre-commit").exists());
        });
    }

    #[serial]
    #[test]
    fn without_data_flag_gitkit_dir_survives() {
        with_temp_home(|home| {
            let gitkit_dir = home.join(".gitkit");
            fs::create_dir_all(&gitkit_dir).unwrap();
            fs::write(gitkit_dir.join("registry.toml"), "").unwrap();

            let repo = tempfile::TempDir::new().unwrap();
            init_repo(repo.path());
            let hooks_dir = repo.path().join(".git").join("hooks");
            install_dispatcher(&hooks_dir, "pre-commit");
            let builtin = hooks::builtins::get("no-secrets").unwrap();
            install_part(&hooks_dir, "pre-commit", "no-secrets", builtin.script);

            registry::record(repo.path(), &["hook:no-secrets".to_string()]).unwrap();

            let args = UninstallArgs {
                data: false,
                yes: true,
                dry_run: false,
            };
            run(args).unwrap();

            assert!(gitkit_dir.exists());
        });
    }

    #[serial]
    #[test]
    fn with_data_flag_removes_gitkit_dir() {
        with_temp_home(|home| {
            let gitkit_dir = home.join(".gitkit");
            fs::create_dir_all(&gitkit_dir).unwrap();
            fs::write(gitkit_dir.join("registry.toml"), "").unwrap();

            let repo = tempfile::TempDir::new().unwrap();
            init_repo(repo.path());
            let hooks_dir = repo.path().join(".git").join("hooks");
            install_dispatcher(&hooks_dir, "pre-commit");
            let builtin = hooks::builtins::get("no-secrets").unwrap();
            install_part(&hooks_dir, "pre-commit", "no-secrets", builtin.script);

            registry::record(repo.path(), &["hook:no-secrets".to_string()]).unwrap();

            let args = UninstallArgs {
                data: true,
                yes: true,
                dry_run: false,
            };
            run(args).unwrap();

            assert!(!gitkit_dir.exists());
        });
    }

    // ── plan_hook / plan_hooks_for_repo ─────────────────────────────────────

    #[test]
    fn plan_hook_skips_missing_dispatcher_file() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(plan_hook(dir.path(), "pre-commit").is_none());
    }

    #[test]
    fn plan_hook_skips_hook_replaced_by_hand() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("pre-commit"), "#!/bin/sh\nmine\n").unwrap();
        assert!(plan_hook(dir.path(), "pre-commit").is_none());
    }

    #[test]
    fn plan_hook_plans_dispatcher_with_parts() {
        let dir = tempfile::TempDir::new().unwrap();
        install_dispatcher(dir.path(), "pre-commit");
        install_part(dir.path(), "pre-commit", "no-secrets", "#!/bin/sh\np\n");

        let plan = plan_hook(dir.path(), "pre-commit").unwrap();
        assert_eq!(plan.hook_name, "pre-commit");
        assert!(plan.has_dispatcher);
        assert_eq!(plan.parts, vec!["no-secrets".to_string()]);
        assert!(!plan.has_preexisting);
    }

    #[test]
    fn plan_hook_flags_preserved_preexisting_part() {
        let dir = tempfile::TempDir::new().unwrap();
        install_dispatcher(dir.path(), "pre-commit");
        install_part(
            dir.path(),
            "pre-commit",
            hooks::PRESERVED_PART_NAME,
            "#!/bin/sh\nold\n",
        );

        let plan = plan_hook(dir.path(), "pre-commit").unwrap();
        assert!(plan.has_preexisting);
        assert_eq!(plan.parts, vec![hooks::PRESERVED_PART_NAME.to_string()]);
    }

    #[test]
    fn plan_hooks_for_repo_empty_without_hooks_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(plan_hooks_for_repo(dir.path()).is_empty());
    }

    #[test]
    fn plan_hooks_for_repo_finds_installed_dispatcher() {
        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let hooks_dir = dir.path().join(".git").join("hooks");
        install_dispatcher(&hooks_dir, "pre-commit");

        let plans = plan_hooks_for_repo(dir.path());
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].hook_name, "pre-commit");
    }
}
