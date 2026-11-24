mod manager;
mod omp;

use anyhow::Result;
use clap::{Parser, Subcommand};
use kit_core::paths::data_root;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Optional local-source manager for Herdr kit packages"
)]
struct Cli {
    /// Independent suite data, installation receipts and release store.
    #[arg(long, env = "HERDR_KIT_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the suite from a trusted local Git repository.
    Install {
        #[arg(long)]
        app_home: PathBuf,
        #[arg(long)]
        source: PathBuf,
        /// Exact component set; omitted selects the herdr-kit bundle.
        #[arg(long = "component", value_delimiter = ',')]
        components: Vec<String>,
        /// Full committed source revision; omitted uses trusted HEAD.
        #[arg(long)]
        revision: Option<String>,
    },
    /// Read or configure the trusted local Git update source.
    Source {
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Inspect the installation and configured update source.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Update this executable and managed Herdr assets together.
    Update {
        #[command(subcommand)]
        action: manager::UpdateAction,
        /// Update only these already selected components.
        #[arg(long = "component", global = true, value_delimiter = ',')]
        components: Vec<String>,
    },
    /// Restart eligible live OMP sessions, including current images with --force.
    Omp {
        #[command(subcommand)]
        action: OmpAction,
    },
}

#[derive(Subcommand)]
enum OmpAction {
    /// Read-only: every terminal, whether its OMP runs the target executable, and what
    /// `restart` would do with it.
    Status {
        /// Only these terminals.
        terminals: Vec<String>,
        /// Target executable; default `omp` on PATH.
        #[arg(long)]
        omp: Option<PathBuf>,
    },
    /// Restart the named sessions, or every eligible one, serially through the
    /// herdr-kit OMP plugin. Sessions that are busy, hold a draft or cannot be proven
    /// safe are skipped; the run stops at the first failed restart.
    Restart {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        terminals: Vec<String>,
        #[arg(long)]
        all: bool,
        /// Also restart eligible sessions already running the target image, to load
        /// changed extensions. All safety checks still apply; a loaded bridge is required.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        omp: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string(&value).expect("serializable response")
            );
            // A restart run that stopped early or failed a session is not a success.
            if value.get("complete").and_then(serde_json::Value::as_bool) == Some(false)
                || value.pointer("/summary/failed").is_some()
            {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{}", kit_core::error::json(&error, "operation_failed"));
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<serde_json::Value> {
    let root = data_root(cli.data_dir)?;
    let mut value = match cli.command {
        Command::Install {
            app_home,
            source,
            components,
            revision,
        } => manager::install(&root, &app_home, &source, &components, revision.as_deref()),
        Command::Source { repo } => manager::source(&root, repo.as_deref()),
        Command::Status { .. } => manager::status(&root),
        Command::Update { action, components } => manager::update(&root, action, &components),
        Command::Omp { action } => return omp_command(action),
    }?;
    // Only install and update responses carry activation flags.
    let flag = |key: &str| value.get(key).and_then(serde_json::Value::as_bool) == Some(true);
    if flag("coord_changed") {
        value["coord"] = manager::after_activation(&root);
    } else if flag("activated") {
        value["coord"] =
            serde_json::json!({"state":"skipped","reason":"coordinator_not_activated"});
    }
    Ok(value)
}

fn omp_command(action: OmpAction) -> Result<serde_json::Value> {
    let host = omp::System::new()?;
    let me = std::env::var("HC_TERMINAL_ID")
        .ok()
        .filter(|s| !s.is_empty());
    match action {
        OmpAction::Status { terminals, omp } => {
            omp::status(&host, &omp::target(omp)?, &terminals, me.as_deref())
        }
        OmpAction::Restart {
            terminals,
            all,
            force,
            omp,
        } => {
            let target = omp::target(omp)?;
            let scope = if all {
                omp::Scope::All
            } else {
                omp::Scope::Only(terminals)
            };
            let _lock = host.lock()?;
            // First Ctrl+C finishes the session in flight and reports; a second one exits.
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                signal_hook::flag::register_conditional_shutdown(signal, 130, cancel.clone())?;
                signal_hook::flag::register(signal, cancel.clone())?;
            }
            omp::restart(&host, &target, &scope, me.as_deref(), &cancel, force)
        }
    }
}
