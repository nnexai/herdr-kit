mod manager;
mod paths;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Optional local-source manager for Herdr kit packages")]
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
        command: UpdateCommand,
        /// Update only these already selected components.
        #[arg(long = "component", global = true, value_delimiter = ',')]
        components: Vec<String>,
    },
}


#[derive(Clone, Copy, Subcommand)]
enum UpdateCommand {
    Check,
    Apply,
    Maintain,
    Rollback,
}


fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(value) => println!("{}", serde_json::to_string(&value).expect("serializable response")),
        Err(error) => {
            let message: String = error.to_string().chars().filter(|c| !c.is_control()).take(512).collect();
            let (code, exit) = ("operation_failed", 1);
            eprintln!("{}", serde_json::json!({"error":{"code":code,"message":message}}));
            std::process::exit(exit);
        }
    }
}

fn data_root(arg: Option<PathBuf>) -> Result<PathBuf> {
    let root = arg.map(Ok).unwrap_or_else(paths::data_dir)?;
    if root.is_absolute() { Ok(root) }
    else { Ok(std::env::current_dir().context("current directory unavailable")?.join(root)) }
}

fn run(cli: Cli) -> Result<serde_json::Value> {
    let root = data_root(cli.data_dir)?;
    let activation_command = matches!(&cli.command, Command::Install { .. } | Command::Update { .. });
    let mut value = match cli.command {
        Command::Install { app_home, source, components, revision } => manager::install(&root, &app_home, &source, &components, revision.as_deref()),
        Command::Source { repo } => manager::source(&root, repo.as_deref()),
        Command::Status { .. } => manager::status(&root),
        Command::Update { command, components } => manager::update(&root, match command {
            UpdateCommand::Check => "check",
            UpdateCommand::Apply => "apply",
            UpdateCommand::Maintain => "maintain",
            UpdateCommand::Rollback => "rollback",
        }, &components),
    }?;
    if activation_command && value.get("coord_changed").and_then(serde_json::Value::as_bool) == Some(true) {
        value["coord"] = manager::after_activation(&root);
    } else if activation_command && value.get("activated").and_then(serde_json::Value::as_bool) == Some(true) {
        value["coord"] = serde_json::json!({"state":"skipped","reason":"coordinator_not_activated"});
    }
    Ok(value)
}
