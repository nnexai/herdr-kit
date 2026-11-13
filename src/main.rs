mod coord;
mod manager;
mod paths;
mod usage;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Local toolkit and native Herdr layout/mod suite")]
struct Cli {
    /// Independent suite data, installation receipts and release store.
    #[arg(long, env = "HERDR_KIT_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Coordinate parallel agent work.
    Coord(coord::cli::CoordArgs),
    /// Read anonymous OMP subscription usage.
    Usage {
        #[command(subcommand)]
        command: UsageCommand,
    },
    /// Install the suite from a trusted local Git repository.
    Install {
        #[arg(long)]
        app_home: PathBuf,
        #[arg(long)]
        source: PathBuf,
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
    },
}

#[derive(Subcommand)]
enum UsageCommand {
    Snapshot {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        agents_working: bool,
        #[arg(long, env = "HERDR_KIT_OMP_EXECUTABLE")]
        omp_executable: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Subcommand)]
enum UpdateCommand {
    Check,
    Apply,
    Maintain,
    Rollback,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    let is_coord = matches!(&cli.command, Command::Coord(_));
    match run(cli).await {
        Ok(coord::Output::Json(value)) => println!("{}", serde_json::to_string(&value).expect("serializable response")),
        Ok(coord::Output::Text(text)) => print!("{text}"),
        Err(error) => {
            let message: String = error.to_string().chars().filter(|c| !c.is_control()).take(512).collect();
            let (code, exit) = if is_coord { coord::error::exit_of(&error) } else { ("operation_failed", 1) };
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

async fn run(cli: Cli) -> Result<coord::Output> {
    let root = data_root(cli.data_dir)?;
    let activation_command = matches!(&cli.command, Command::Install { .. } | Command::Update { .. });
    let mut value = match cli.command {
        Command::Coord(args) => return coord::run(&root, args).await,
        Command::Usage { command: UsageCommand::Snapshot { agents_working, omp_executable, .. } } => {
            serde_json::to_value(usage::snapshot(&root.join("usage"), agents_working, omp_executable.as_deref()).await?)
                .context("usage response could not be encoded")
        }
        Command::Install { app_home, source } => manager::install(&root, &app_home, &source),
        Command::Source { repo } => manager::source(&root, repo.as_deref()),
        Command::Status { .. } => manager::status(&root),
        Command::Update { command } => manager::update(&root, match command {
            UpdateCommand::Check => "check",
            UpdateCommand::Apply => "apply",
            UpdateCommand::Maintain => "maintain",
            UpdateCommand::Rollback => "rollback",
        }),
    }?;
    if activation_command && value.get("activated").and_then(serde_json::Value::as_bool) == Some(true) {
        value["coord"] = coord::lifecycle::after_activation(&root);
    }
    Ok(coord::Output::Json(value))
}
