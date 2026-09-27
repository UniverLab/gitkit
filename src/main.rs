use anyhow::Result;
use clap::{Parser, Subcommand};

mod attributes;
mod autoupdate;
mod builds;
mod clone;
mod config;
mod git;
mod hooks;
mod ignore;
mod init;
mod lock;
mod registry;
mod status;
mod uninstall;
mod utils;

#[derive(Parser)]
#[command(
    name = "gitkit",
    version,
    about = "Standalone CLI for configuring git repos"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Interactive wizard to configure your repo
    Init,
    /// Show current configuration status
    Status(status::StatusArgs),
    /// Clone repository and run init wizard
    Clone(clone::CloneArgs),
    /// Manage git hooks
    Hooks {
        #[command(subcommand)]
        action: hooks::HooksCommand,
    },
    /// Generate .gitignore via gitignore.io
    Ignore {
        #[command(subcommand)]
        action: ignore::IgnoreCommand,
    },
    /// Configure .gitattributes
    Attributes {
        #[command(subcommand)]
        action: attributes::AttributesCommand,
    },
    /// Apply curated git config presets
    Config {
        #[command(subcommand)]
        action: config::ConfigCommand,
    },
    /// Manage saved builds
    Build {
        #[command(subcommand)]
        action: builds::BuildCommand,
    },
    /// Block commits and/or pushes for the duration of an agent session
    Lock(lock::LockArgs),
    /// Remove an active commit/push lock
    Unlock,
    /// Remove gitkit hooks from every repository it has touched
    Uninstall(uninstall::UninstallArgs),
    /// Check for a newer stable release and install it (asks first; refuses cargo installs)
    Update {
        /// Only report whether an update exists: exit 1 when one does, 0 when current
        #[arg(long)]
        check: bool,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // The explicit command fetches and reports on its own, loudly — running
    // the silent background check too would only double the requests.
    if !matches!(&cli.command, Some(Command::Update { .. })) {
        autoupdate::check_for_update();
    }
    match cli.command {
        Some(Command::Init) | None => init::run(),
        Some(Command::Status(args)) => status::run(args),
        Some(Command::Clone(args)) => clone::run(args),
        Some(Command::Hooks { action }) => hooks::run(action),
        Some(Command::Ignore { action }) => ignore::run(action),
        Some(Command::Attributes { action }) => attributes::run(action),
        Some(Command::Config { action }) => config::run(action),
        Some(Command::Build { action }) => builds::run(action),
        Some(Command::Lock(args)) => lock::run(args),
        Some(Command::Unlock) => lock::unlock(),
        Some(Command::Uninstall(args)) => uninstall::run(args),
        Some(Command::Update { check, yes }) => {
            // Non-zero exit codes (`--check` finds an update, a missing
            // binary in the archive) and errors are reported by the command.
            let code = autoupdate::run_update(check, yes)?;
            std::process::exit(code);
        }
    }
}
