use std::process::Command;
use tempfile::TempDir;

fn gitkit_binary() -> std::path::PathBuf {
    // Build the binary first, then return its path
    let output = Command::new("cargo")
        .args(["build", "--message-format=json"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to build");
    assert!(output.status.success(), "Failed to build gitkit binary");

    // Find the binary in target/debug
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = manifest_dir.join("target/debug/gitkit");
    assert!(binary.exists(), "Binary not found at {binary:?}");
    binary
}

fn run_gitkit(args: &[&str]) -> (bool, String) {
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to run gitkit");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.success(), format!("{stdout}{stderr}"))
}

// ═══════════════════════════════════════════════════════════════════════════
// CLI integration tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn cli_no_args_shows_banner() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let (success, output) = run_gitkit(&["--help"]);
    assert!(success, "gitkit --help should succeed");
    assert!(output.contains("gitkit"));
}

#[test]
fn cli_version_flag() {
    let (success, output) = run_gitkit(&["--version"]);
    assert!(success);
    assert!(output.contains("gitkit"));
}

#[test]
fn cli_help_flag() {
    let (success, output) = run_gitkit(&["--help"]);
    assert!(success);
    assert!(output.contains("init"));
    assert!(output.contains("status"));
    assert!(output.contains("clone"));
    assert!(output.contains("hooks"));
    assert!(output.contains("ignore"));
    assert!(output.contains("attributes"));
    assert!(output.contains("config"));
    assert!(output.contains("build"));
}

#[test]
fn cli_hooks_help() {
    let (success, output) = run_gitkit(&["hooks", "--help"]);
    assert!(success);
    assert!(output.contains("add"));
    assert!(output.contains("list"));
    assert!(output.contains("remove"));
    assert!(output.contains("show"));
}

#[test]
fn cli_ignore_help() {
    let (success, output) = run_gitkit(&["ignore", "--help"]);
    assert!(success);
    assert!(output.contains("add"));
    assert!(output.contains("list"));
}

#[test]
fn cli_attributes_help() {
    let (success, output) = run_gitkit(&["attributes", "--help"]);
    assert!(success);
    assert!(output.contains("init"));
}

#[test]
fn cli_config_help() {
    let (success, output) = run_gitkit(&["config", "--help"]);
    assert!(success);
    assert!(output.contains("apply"));
    assert!(output.contains("show"));
}

#[test]
fn cli_build_help() {
    let (success, output) = run_gitkit(&["build", "--help"]);
    assert!(success);
    assert!(output.contains("list"));
    assert!(output.contains("apply"));
    assert!(output.contains("save"));
    assert!(output.contains("delete"));
}

#[test]
fn update_help_documents_exit_codes() {
    let (success, output) = run_gitkit(&["update", "--help"]);
    assert!(success, "gitkit update --help should succeed");
    // clap wraps help lines, so compare against one flattened line.
    let flat = output.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("0 = up to date or nothing installed"),
        "exit 0 undocumented: {flat}"
    );
    assert!(
        flat.contains("1 = update available"),
        "exit 1 undocumented: {flat}"
    );
    assert!(
        flat.contains("2 = the update check could not complete"),
        "exit 2 undocumented: {flat}"
    );
    assert!(flat.contains("exit 0"), "--check help: {flat}");
    assert!(flat.contains("exit 1"), "--check help: {flat}");
    assert!(flat.contains("exit 2"), "--check help: {flat}");
}

/// The spec's guideline command: behind a dead proxy the check must fail —
/// not hang, not pretend an update exists — with exit 2 and one stderr line
/// naming the cause. Loopback only, so it is deterministic offline.
#[test]
fn update_check_through_dead_proxy_exits_2() {
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "")
        .args(["update", "--check"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to run gitkit");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2, stderr: {stderr}"
    );
    assert!(
        stderr.starts_with("update check failed:"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn cli_status_outside_repo() {
    let dir = TempDir::new().unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["status"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    // status outside a repo should not panic (just print global config)
    assert!(output.status.success());
}

#[test]
fn cli_hooks_list_outside_repo() {
    let dir = TempDir::new().unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "list"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    // hooks list outside repo should fail gracefully (no hooks dir)
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should indicate error about not being in a repo
    assert!(!output.status.success() || stderr.contains("error") || !output.status.success());
}

#[test]
fn cli_build_list_empty() {
    let dir = TempDir::new().unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["build", "list"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No builds") || stdout.contains("Saved builds") || !output.status.success(),
        "Should show 'No builds', 'Saved builds', or fail gracefully"
    );
}

#[test]
fn cli_hooks_add_invalid_builtin() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "--yes", "nonexistent-builtin"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should fail — not a builtin and no command provided
    assert!(
        !output.status.success()
            || stderr.contains("not a built-in")
            || stdout.contains("not a built-in"),
        "Should reject unknown builtin without command"
    );
}

#[test]
fn cli_hooks_add_custom_hook() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "--yes", "pre-push", "echo test"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(
        output.status.success(),
        "Adding custom hook should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Verify the hook file was created
    let hook_path = dir.path().join(".git").join("hooks").join("pre-push");
    assert!(hook_path.exists());
    let content = std::fs::read_to_string(&hook_path).unwrap();
    assert!(content.contains("#!/bin/sh"));
    assert!(content.contains("echo test"));
}

#[test]
fn cli_hooks_add_builtin_conventional_commits() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "--yes", "conventional-commits"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(
        output.status.success(),
        "Installing builtin should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hook_path = dir.path().join(".git").join("hooks").join("commit-msg");
    assert!(hook_path.exists());
}

#[test]
fn cli_hooks_remove_installed_hook() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    // Create a dummy hook
    std::fs::write(hooks_dir.join("pre-push"), "#!/bin/sh\necho test\n").unwrap();

    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "remove", "--yes", "pre-push"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(
        output.status.success(),
        "Removing hook should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!hooks_dir.join("pre-push").exists());
}

#[test]
fn cli_hooks_remove_nonexistent_hook() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();

    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "remove", "--yes", "nonexistent-hook"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(
        !output.status.success(),
        "Removing nonexistent hook should fail"
    );
}

#[test]
fn cli_hooks_show_installed_hook() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hook_content = "#!/bin/sh\necho hello\n";
    std::fs::write(hooks_dir.join("pre-push"), hook_content).unwrap();

    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "show", "pre-push"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("echo hello"));
}

#[test]
fn cli_hooks_show_nonexistent_hook() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();

    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "show", "nonexistent"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(!output.status.success());
}

#[test]
fn cli_hooks_add_invalid_hook_name() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "--yes", "not-a-real-hook", "echo hi"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() || stderr.contains("not a valid git hook"),
        "Should reject invalid hook name"
    );
}

#[test]
fn cli_hooks_add_custom_hook_creates_executable() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "--yes", "pre-commit", "echo hello"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(output.status.success());
    let hook_path = dir.path().join(".git").join("hooks").join("pre-commit");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&hook_path).unwrap().permissions();
        assert!(perms.mode() & 0o111 != 0, "Hook should be executable");
    }
}

#[test]
fn cli_hooks_add_with_dry_run() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args([
            "hooks",
            "add",
            "--yes",
            "--dry-run",
            "pre-push",
            "echo test",
        ])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[dry-run]"));
    // Hook file should NOT exist
    assert!(!dir
        .path()
        .join(".git")
        .join("hooks")
        .join("pre-push")
        .exists());
}

#[test]
fn cli_ignore_add_dry_run() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["ignore", "add", "--yes", "--dry-run", "rust"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[dry-run]"));
}

#[test]
fn cli_attributes_init_dry_run() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let binary = gitkit_binary();
    let output = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["attributes", "init", "--yes", "--dry-run"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[dry-run]"));
    assert!(!dir.path().join(".gitattributes").exists());
}

#[test]
fn cli_config_show_does_not_panic() {
    let (success, _) = run_gitkit(&["config", "show"]);
    assert!(success);
}

#[test]
fn cli_config_apply_dry_run() {
    let (success, output) = run_gitkit(&["config", "apply", "defaults", "--dry-run"]);
    assert!(success);
    assert!(output.contains("[dry-run]") || output.contains("already set"));
}

// ═══════════════════════════════════════════════════════════════════════════
// Lock / commit-blocking integration tests (real git repo, real git commit)
// ═══════════════════════════════════════════════════════════════════════════

fn init_git_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("Failed to run git");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test User"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(&["add", "README.md"]);
}

fn git_commit_allow_empty(dir: &std::path::Path, msg: &str) -> (bool, String) {
    let output = Command::new("git")
        .args(["commit", "--allow-empty", "-m", msg])
        .current_dir(dir)
        .output()
        .expect("Failed to run git commit");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (output.status.success(), format!("{stdout}{stderr}"))
}

#[test]
fn lock_fixture_commit_succeeds_unlocked_fails_locked_succeeds_after_unlock() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    // Unlocked: commit succeeds.
    let (ok, _) = git_commit_allow_empty(dir.path(), "initial commit");
    assert!(ok, "commit should succeed with no lock active");

    // Lock: commit fails and names the reason + how to unlock.
    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--reason", "Agent session active"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    let (ok, msg) = git_commit_allow_empty(dir.path(), "blocked commit");
    assert!(!ok, "commit should fail while locked");
    assert!(msg.contains("Agent session active"), "message was: {msg}");
    assert!(msg.contains("gitkit unlock"), "message was: {msg}");

    // Unlock: commit succeeds again.
    let unlock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["unlock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit unlock");
    assert!(unlock_out.status.success());

    let (ok, _) = git_commit_allow_empty(dir.path(), "commit after unlock");
    assert!(ok, "commit should succeed after unlock");
}

#[test]
fn lock_fixture_expired_lock_does_not_block_commit() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--timeout", "30m"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    // Confirm it blocks before expiry.
    let (ok, _) = git_commit_allow_empty(dir.path(), "blocked commit");
    assert!(!ok, "commit should fail while lock has not expired");

    // Rewrite the lock file with an expiry far in the past.
    let lock_path = dir.path().join(".git").join("gitkit.lock");
    std::fs::write(
        &lock_path,
        r#"{"locked_at":"2000-01-01T00:00:00Z","expires_at":"2000-01-01T00:01:00Z","reason":"stale","operations":["commit"]}"#,
    )
    .unwrap();

    let (ok, _) = git_commit_allow_empty(dir.path(), "commit after expiry");
    assert!(ok, "commit should succeed once the lock has expired");

    // status should report the lock as expired.
    let status_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock status");
    let status_msg = String::from_utf8_lossy(&status_out.stdout);
    assert!(status_msg.contains("expired"), "status was: {status_msg}");
}

#[test]
fn lock_fixture_malformed_lock_file_fails_open() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    // Install the hook via a real lock, then corrupt the lock file.
    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    let lock_path = dir.path().join(".git").join("gitkit.lock");
    std::fs::write(&lock_path, "not json at all {{{").unwrap();

    let (ok, _) = git_commit_allow_empty(dir.path(), "commit with malformed lock");
    assert!(ok, "a malformed lock file must never block a commit");
}

#[test]
fn lock_fixture_locking_twice_is_idempotent() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let run_lock = |reason: &str| {
        let out = Command::new(&binary)
            // Never let a test hit the network via the update check.
            .env("GITKIT_NO_UPDATE_CHECK", "1")
            .env("HOME", dir.path())
            .args(["lock", "--reason", reason])
            .current_dir(dir.path())
            .output()
            .expect("Failed to run gitkit lock");
        assert!(out.status.success());
    };
    run_lock("first reason");
    run_lock("second reason");

    let (ok, msg) = git_commit_allow_empty(dir.path(), "blocked commit");
    assert!(!ok);
    assert!(msg.contains("second reason"), "message was: {msg}");
    assert!(!msg.contains("first reason"), "message was: {msg}");

    let hooks_dir = dir.path().join(".git").join("hooks");
    assert!(
        !hooks_dir.join("pre-commit.gitkit-orig").exists(),
        "locking twice with no prior user hook must not fabricate a backup"
    );
}

#[test]
fn lock_fixture_preserves_existing_user_pre_commit_hook() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let user_hook = "#!/bin/sh\necho user-hook-ran >&2\nexit 0\n";
    std::fs::write(hooks_dir.join("pre-commit"), user_hook).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(hooks_dir.join("pre-commit"))
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(hooks_dir.join("pre-commit"), perms).unwrap();
    }
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());
    assert!(hooks_dir.join("pre-commit.gitkit-orig").exists());

    let unlock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["unlock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit unlock");
    assert!(unlock_out.status.success());

    let restored = std::fs::read_to_string(hooks_dir.join("pre-commit")).unwrap();
    assert_eq!(restored, user_hook);
    assert!(!hooks_dir.join("pre-commit.gitkit-orig").exists());

    // The restored user hook still runs on commit.
    let (ok, msg) = git_commit_allow_empty(dir.path(), "commit runs user hook");
    assert!(ok);
    assert!(msg.contains("user-hook-ran"), "message was: {msg}");
}

// ═══════════════════════════════════════════════════════════════════════════
// Lock / push-blocking integration tests (real git repo, real git push)
// ═══════════════════════════════════════════════════════════════════════════

/// Sets up `dir` as a git repo with an initial commit and a local bare
/// remote named `origin`, so `git push` can be exercised without touching
/// the network. Returns the bare remote's `TempDir` - keep it alive for the
/// duration of the test.
fn init_git_repo_with_remote(dir: &std::path::Path) -> TempDir {
    let remote_dir = TempDir::new().unwrap();
    let status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(remote_dir.path())
        .status()
        .expect("Failed to init bare remote");
    assert!(status.success());

    init_git_repo(dir);
    let (ok, _) = git_commit_allow_empty(dir, "initial commit");
    assert!(ok, "initial commit should succeed");

    let status = Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            remote_dir.path().to_str().unwrap(),
        ])
        .current_dir(dir)
        .status()
        .expect("Failed to add remote");
    assert!(status.success());

    remote_dir
}

fn git_push_head(dir: &std::path::Path) -> (bool, String) {
    let output = Command::new("git")
        .args(["push", "origin", "HEAD:refs/heads/main"])
        .current_dir(dir)
        .output()
        .expect("Failed to run git push");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (output.status.success(), format!("{stdout}{stderr}"))
}

#[test]
fn push_fixture_push_succeeds_unlocked_fails_locked_succeeds_after_unlock() {
    let dir = TempDir::new().unwrap();
    let _remote = init_git_repo_with_remote(dir.path());
    let binary = gitkit_binary();

    // Unlocked: push succeeds.
    let (ok, msg) = git_push_head(dir.path());
    assert!(ok, "push should succeed with no lock active: {msg}");

    // Lock --push: push fails and names the reason + how to unlock.
    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--push", "--reason", "Agent session active"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --push");
    assert!(lock_out.status.success());

    let (ok, _) = git_commit_allow_empty(dir.path(), "another commit");
    assert!(ok, "commit should still succeed - only push is locked");

    let (ok, msg) = git_push_head(dir.path());
    assert!(!ok, "push should fail while push-locked");
    assert!(msg.contains("Agent session active"), "message was: {msg}");
    assert!(msg.contains("gitkit unlock"), "message was: {msg}");

    // Unlock: push succeeds again.
    let unlock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["unlock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit unlock");
    assert!(unlock_out.status.success());

    let (ok, _) = git_push_head(dir.path());
    assert!(ok, "push should succeed after unlock");
}

#[test]
fn push_fixture_expired_lock_does_not_block_push() {
    let dir = TempDir::new().unwrap();
    let _remote = init_git_repo_with_remote(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--push", "--timeout", "30m"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --push");
    assert!(lock_out.status.success());

    // Confirm it blocks before expiry.
    let (ok, _) = git_push_head(dir.path());
    assert!(!ok, "push should fail while lock has not expired");

    // Rewrite the lock file with an expiry far in the past.
    let lock_path = dir.path().join(".git").join("gitkit.lock");
    std::fs::write(
        &lock_path,
        r#"{"locked_at":"2000-01-01T00:00:00Z","expires_at":"2000-01-01T00:01:00Z","reason":"stale","operations":["push"]}"#,
    )
    .unwrap();

    let (ok, _) = git_push_head(dir.path());
    assert!(ok, "push should succeed once the lock has expired");

    let status_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock status");
    let status_msg = String::from_utf8_lossy(&status_out.stdout);
    assert!(status_msg.contains("expired"), "status was: {status_msg}");
}

#[test]
fn push_fixture_malformed_lock_file_fails_open() {
    let dir = TempDir::new().unwrap();
    let _remote = init_git_repo_with_remote(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--push"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --push");
    assert!(lock_out.status.success());

    let lock_path = dir.path().join(".git").join("gitkit.lock");
    std::fs::write(&lock_path, "not json at all {{{").unwrap();

    let (ok, _) = git_push_head(dir.path());
    assert!(ok, "a malformed lock file must never block a push");
}

#[test]
fn push_fixture_commit_lock_alone_does_not_block_push() {
    let dir = TempDir::new().unwrap();
    let _remote = init_git_repo_with_remote(dir.path());
    let binary = gitkit_binary();

    // Only the commit lock is active - push must remain unblocked.
    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    let (ok, msg) = git_push_head(dir.path());
    assert!(ok, "push should succeed while only commit is locked: {msg}");

    let (ok, _) = git_commit_allow_empty(dir.path(), "blocked commit");
    assert!(!ok, "commit should still fail while commit-locked");
}

#[test]
fn push_fixture_all_locks_both_commit_and_push() {
    let dir = TempDir::new().unwrap();
    let _remote = init_git_repo_with_remote(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--all", "--reason", "full lockdown"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --all");
    assert!(lock_out.status.success());

    let (ok, _) = git_commit_allow_empty(dir.path(), "blocked commit");
    assert!(!ok, "commit should fail under --all");

    let (ok, msg) = git_push_head(dir.path());
    assert!(!ok, "push should fail under --all: {msg}");
    assert!(msg.contains("full lockdown"), "message was: {msg}");
}

#[test]
fn push_fixture_adding_push_lock_preserves_commit_lock_reason() {
    let dir = TempDir::new().unwrap();
    let _remote = init_git_repo_with_remote(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--reason", "original session"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    // Extend to push without a new --reason.
    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--push"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --push");
    assert!(lock_out.status.success());

    let (ok, msg) = git_push_head(dir.path());
    assert!(!ok, "push should fail once push is locked");
    assert!(msg.contains("original session"), "message was: {msg}");

    let (ok, msg) = git_commit_allow_empty(dir.path(), "blocked commit");
    assert!(!ok, "commit should still fail too");
    assert!(msg.contains("original session"), "message was: {msg}");
}

#[test]
fn push_fixture_preserves_existing_user_pre_push_hook() {
    let dir = TempDir::new().unwrap();
    let _remote = init_git_repo_with_remote(dir.path());
    let hooks_dir = dir.path().join(".git").join("hooks");
    let user_hook = "#!/bin/sh\ncat >/dev/null\necho user-push-hook-ran >&2\nexit 0\n";
    std::fs::write(hooks_dir.join("pre-push"), user_hook).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(hooks_dir.join("pre-push"))
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(hooks_dir.join("pre-push"), perms).unwrap();
    }
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--push"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --push");
    assert!(lock_out.status.success());
    assert!(hooks_dir.join("pre-push.gitkit-orig").exists());

    let unlock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["unlock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit unlock");
    assert!(unlock_out.status.success());

    let restored = std::fs::read_to_string(hooks_dir.join("pre-push")).unwrap();
    assert_eq!(restored, user_hook);
    assert!(!hooks_dir.join("pre-push.gitkit-orig").exists());

    // The restored user hook still runs on push.
    let (ok, msg) = git_push_head(dir.path());
    assert!(ok);
    assert!(msg.contains("user-push-hook-ran"), "message was: {msg}");
}

// ═══════════════════════════════════════════════════════════════════════════
// `lock status --json` integration tests
// ═══════════════════════════════════════════════════════════════════════════
//
// Exit-code assertions live here (real subprocess) rather than in the crate's
// unit tests, since `lock status --json` calls `process::exit` and running
// that in-process would kill the test binary.

#[test]
fn lock_status_json_exit_code_zero_when_unlocked() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock status --json");

    assert!(
        out.status.success(),
        "expected exit 0 when no lock is active"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"active\":false"), "stdout was: {stdout}");
    assert!(stdout.contains("\"expired\":false"), "stdout was: {stdout}");
    assert!(stdout.contains("\"operations\":[]"), "stdout was: {stdout}");
}

#[test]
fn lock_status_json_exit_code_nonzero_when_locked() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--reason", "agent session"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    let out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock status --json");

    assert!(
        !out.status.success(),
        "expected non-zero exit when a lock is active"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"active\":true"), "stdout was: {stdout}");
    assert!(
        stdout.contains("\"reason\":\"agent session\""),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("\"operations\":[\"commit\",\"rebase\"]"),
        "stdout was: {stdout}"
    );
}

#[test]
fn lock_status_json_exit_code_zero_when_expired() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let lock_path = dir.path().join(".git").join("gitkit.lock");
    std::fs::write(
        &lock_path,
        r#"{"locked_at":"2000-01-01T00:00:00Z","expires_at":"2000-01-01T00:01:00Z","reason":"stale","operations":["commit"]}"#,
    )
    .unwrap();

    let out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock status --json");

    assert!(
        out.status.success(),
        "expected exit 0 when the lock has expired"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"active\":false"), "stdout was: {stdout}");
    assert!(stdout.contains("\"expired\":true"), "stdout was: {stdout}");
    assert!(
        stdout.contains("\"operations\":[\"commit\"]"),
        "stdout was: {stdout}"
    );
}

#[test]
fn lock_status_json_exit_code_zero_when_malformed() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let lock_path = dir.path().join(".git").join("gitkit.lock");
    std::fs::write(&lock_path, "not json at all {{{").unwrap();

    let out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock status --json");

    assert!(
        out.status.success(),
        "a malformed lock file must report unlocked, not fail the caller"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"active\":false"), "stdout was: {stdout}");
}

#[test]
fn lock_status_json_works_from_a_subdirectory() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    let subdir = dir.path().join("nested").join("deeper");
    std::fs::create_dir_all(&subdir).unwrap();

    let out = Command::new(&binary)
        // Never let a test hit the network via the update check.
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status", "--json"])
        .current_dir(&subdir)
        .output()
        .expect("Failed to run gitkit lock status --json from a subdirectory");

    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"active\":true"), "stdout was: {stdout}");
}

// ═══════════════════════════════════════════════════════════════════════════
// no-invisibles integration tests (real git repo, real git commit, hook
// script execs back into the built `gitkit` binary — unlike the other
// builtins, which are pure `sh`)
// ═══════════════════════════════════════════════════════════════════════════

/// Prepends the directory holding the built `gitkit` binary to `PATH`, so
/// the installed `no-invisibles` hook's `exec gitkit hooks scan-invisibles`
/// can find it when git runs the hook as a subprocess.
fn path_with_gitkit_dir(binary: &std::path::Path) -> std::ffi::OsString {
    let bin_dir = binary.parent().unwrap();
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    std::env::join_paths(paths).unwrap()
}

fn git_commit_with_path(
    dir: &std::path::Path,
    msg: &str,
    path: &std::ffi::OsStr,
) -> (bool, String) {
    let output = Command::new("git")
        .args(["commit", "-m", msg])
        .env("PATH", path)
        .current_dir(dir)
        .output()
        .expect("Failed to run git commit");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.success(), format!("{stdout}{stderr}"))
}

// ═══════════════════════════════════════════════════════════════════════════
// `status --strict` / `status --repair` integration tests (real subprocess)
// ═══════════════════════════════════════════════════════════════════════════
//
// Exit-code assertions live here rather than in the crate's unit tests, since
// `status --strict` calls `process::exit` when a dormant hook is found, and
// running that in-process would kill the test binary (same rationale as the
// `lock status --json` tests above).

#[test]
fn status_strict_exits_nonzero_when_a_hook_is_dormant() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();

    let binary = gitkit_binary();
    let install_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "no-secrets", "--yes", "--force"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit hooks add");
    assert!(install_out.status.success());

    // Simulate the exact regression this spec exists to catch: a hook file
    // that is present and correct but was never marked executable.
    let hook_path = hooks_dir.join("pre-commit");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&hook_path, perms).unwrap();
    }

    let out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["status", "--strict"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit status --strict");

    #[cfg(unix)]
    assert!(
        !out.status.success(),
        "expected non-zero exit when a hook is dormant"
    );
    #[cfg(not(unix))]
    assert!(
        out.status.success(),
        "the executable bit does not apply on non-Unix targets"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dormant"), "stdout was: {stdout}");
}

#[test]
fn status_strict_exits_zero_with_no_dormant_hooks() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();

    let out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["status", "--strict"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit status --strict");

    assert!(
        out.status.success(),
        "expected exit 0 when nothing is dormant"
    );
}

#[test]
fn status_repair_sets_executable_bit_and_strict_then_passes() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let binary = gitkit_binary();

    let install_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "no-secrets", "--yes", "--force"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit hooks add");
    assert!(install_out.status.success());

    let hook_path = hooks_dir.join("pre-commit");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&hook_path, perms).unwrap();
    }

    let repair_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["status", "--repair"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit status --repair");
    assert!(repair_out.status.success());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&hook_path).unwrap().permissions();
        assert!(
            perms.mode() & 0o111 != 0,
            "repair should have set the executable bit"
        );
    }

    let strict_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["status", "--strict"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit status --strict after repair");
    assert!(
        strict_out.status.success(),
        "repaired hook must no longer be dormant"
    );
}

#[test]
fn no_invisibles_fixture_blocks_zero_width_space_then_accepts_clean_commit() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();
    let path = path_with_gitkit_dir(&binary);

    // Baseline commit (README.md staged by init_git_repo) succeeds with no
    // hook installed yet.
    let (ok, _) = git_commit_with_path(dir.path(), "initial commit", &path);
    assert!(ok, "baseline commit should succeed");

    let install_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["hooks", "add", "no-invisibles", "--yes", "--force"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit hooks add no-invisibles");
    assert!(
        install_out.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&install_out.stderr)
    );

    // Stage a line carrying a zero-width space: the commit must be refused,
    // naming file, line, column and codepoint.
    std::fs::write(dir.path().join("README.md"), "hello\nhidden\u{200B}space\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    let (ok, msg) = git_commit_with_path(dir.path(), "smuggled watermark", &path);
    assert!(!ok, "commit carrying a ZWSP should be refused: {msg}");
    assert!(msg.contains("README.md:2:7"), "message was: {msg}");
    assert!(msg.contains("U+200B"), "message was: {msg}");
    assert!(msg.contains("ZERO WIDTH SPACE"), "message was: {msg}");

    // Remove the character: the commit now succeeds.
    std::fs::write(dir.path().join("README.md"), "hello\nclean space\n").unwrap();
    Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    let (ok, msg) = git_commit_with_path(dir.path(), "cleaned up", &path);
    assert!(ok, "clean commit should pass silently: {msg}");
}

// ── GK-A: composition — `gitkit status` lists every composed builtin ───────

#[test]
fn status_lists_both_builtins_installed_for_one_git_hook() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir_all(dir.path().join(".git").join("hooks")).unwrap();
    let binary = gitkit_binary();

    for builtin in ["conventional-commits", "no-body"] {
        let install_out = Command::new(&binary)
            .env("GITKIT_NO_UPDATE_CHECK", "1")
            .env("HOME", dir.path())
            .args(["hooks", "add", builtin, "--yes", "--force"])
            .current_dir(dir.path())
            .output()
            .unwrap_or_else(|_| panic!("Failed to run gitkit hooks add {builtin}"));
        assert!(install_out.status.success());
    }

    let out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["status"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit status");
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("conventional-commits"),
        "status output was:\n{stdout}"
    );
    assert!(stdout.contains("no-body"), "status output was:\n{stdout}");
}

// ═══════════════════════════════════════════════════════════════════════════
// Lock / pre-rebase integration tests
// ═══════════════════════════════════════════════════════════════════════════

fn git_rebase(dir: &std::path::Path, onto: &str) -> (bool, String) {
    let output = Command::new("git")
        .args(["rebase", onto])
        .current_dir(dir)
        .output()
        .expect("Failed to run git rebase");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (output.status.success(), format!("{stdout}{stderr}"))
}

#[test]
fn lock_fixture_pre_rebase_blocks_and_unlock_releases() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let (ok, _) = git_commit_allow_empty(dir.path(), "initial commit");
    assert!(ok);

    let status = Command::new("git")
        .args(["checkout", "-b", "feature"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    let (ok, _) = git_commit_allow_empty(dir.path(), "feature commit");
    assert!(ok);

    let status = Command::new("git")
        .args(["checkout", "master"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    let (ok, _) = git_commit_allow_empty(dir.path(), "master commit");
    assert!(ok);

    let status = Command::new("git")
        .args(["checkout", "feature"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());

    // `gitkit lock` installs pre-rebase unconditionally alongside commit.
    let lock_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--reason", "Agent session active"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    let hooks_dir = dir.path().join(".git").join("hooks");
    assert!(
        hooks_dir.join("pre-rebase").exists(),
        "pre-rebase hook should be installed by `gitkit lock`"
    );

    let (ok, msg) = git_rebase(dir.path(), "master");
    assert!(!ok, "rebase should fail while locked");
    assert!(msg.contains("rebase blocked"), "message was: {msg}");
    assert!(msg.contains("gitkit unlock"), "message was: {msg}");

    // Unlock: rebase succeeds.
    let unlock_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["unlock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit unlock");
    assert!(unlock_out.status.success());

    let (ok, _) = git_rebase(dir.path(), "master");
    assert!(ok, "rebase should succeed after unlock");
}

// ═══════════════════════════════════════════════════════════════════════════
// Lock / reference-transaction integration tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn lock_fixture_reference_transaction_rejects_head_update() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let (ok, _) = git_commit_allow_empty(dir.path(), "initial commit");
    assert!(ok);
    let (ok, _) = git_commit_allow_empty(dir.path(), "second commit");
    assert!(ok);

    // Install the reference-transaction hook via --refs.
    let lock_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--refs", "--reason", "Agent session active"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --refs");
    assert!(lock_out.status.success());

    let hooks_dir = dir.path().join(".git").join("hooks");
    assert!(
        hooks_dir.join("reference-transaction").exists(),
        "reference-transaction hook should be installed"
    );

    // Attempt to update HEAD to the previous commit (simulates a ref update).
    let output = Command::new("git")
        .args(["update-ref", "HEAD", "HEAD~1"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run git update-ref");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "HEAD update should be rejected by reference-transaction hook"
    );
    assert!(stderr.contains("gitkit unlock"), "message was: {stderr}");
}

#[test]
fn lock_fixture_reference_transaction_permits_refs_remotes() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let (ok, _) = git_commit_allow_empty(dir.path(), "initial commit");
    assert!(ok);

    // Install the reference-transaction hook via --refs.
    let lock_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--refs", "--reason", "Agent session active"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --refs");
    assert!(lock_out.status.success());

    // Updating a refs/remotes/* ref should succeed.
    let output = Command::new("git")
        .args(["update-ref", "refs/remotes/origin/main", "HEAD"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run git update-ref");
    assert!(
        output.status.success(),
        "refs/remotes update should be permitted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lock_fixture_refs_not_in_default_lock() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock");
    assert!(lock_out.status.success());

    let hooks_dir = dir.path().join(".git").join("hooks");
    assert!(
        !hooks_dir.join("reference-transaction").exists(),
        "reference-transaction hook should NOT be installed by default"
    );

    let lock_path = dir.path().join(".git").join("gitkit.lock");
    let content = std::fs::read_to_string(&lock_path).unwrap();
    assert!(
        !content.contains("\"refs\""),
        "refs should not be in default lock operations: {content}"
    );
}

#[test]
fn lock_fixture_status_reports_axes() {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());
    let binary = gitkit_binary();

    let lock_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "--refs", "--reason", "Agent session active"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock --refs");
    assert!(lock_out.status.success());

    let status_out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", dir.path())
        .args(["lock", "status"])
        .current_dir(dir.path())
        .output()
        .expect("Failed to run gitkit lock status");
    assert!(status_out.status.success());

    let stdout = String::from_utf8_lossy(&status_out.stdout);
    assert!(stdout.contains("Commit:"), "status was: {stdout}");
    assert!(stdout.contains("Push:"), "status was: {stdout}");
    assert!(stdout.contains("Rebase:"), "status was: {stdout}");
    assert!(stdout.contains("Refs:"), "status was: {stdout}");
}

// ═══════════════════════════════════════════════════════════════════════════
// GK-I: `status --global` summarises gone entries instead of listing each
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn status_global_summarizes_gone_entries_in_one_line() {
    let home = TempDir::new().unwrap();
    let gitkit_dir = home.path().join(".gitkit");
    std::fs::create_dir_all(&gitkit_dir).unwrap();

    let mut registry_content = String::from("[repos]\n");
    for i in 0..5 {
        let fake_path = format!("/tmp/gone-repo-{i}");
        registry_content.push_str(&format!(
            "[repos.\"{fake_path}\"]\npath = \"{fake_path}\"\napplied_at = \"2026-01-01T00:00:00Z\"\napplied = [\"hook:no-secrets\"]\n"
        ));
    }
    std::fs::write(gitkit_dir.join("registry.toml"), &registry_content).unwrap();

    let binary = gitkit_binary();
    let out = Command::new(&binary)
        .env("GITKIT_NO_UPDATE_CHECK", "1")
        .env("HOME", home.path())
        .args(["status", "--global"])
        .current_dir(home.path())
        .output()
        .expect("Failed to run gitkit status --global");
    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("5 repositories are gone"),
        "expected a single summary line for 5 gone entries, stdout was:\n{stdout}"
    );
    for i in 0..5 {
        let fake_path = format!("/tmp/gone-repo-{i}");
        assert!(
            !stdout.contains(&fake_path),
            "stdout must NOT list individual gone entry {fake_path}, stdout was:\n{stdout}"
        );
    }
}
