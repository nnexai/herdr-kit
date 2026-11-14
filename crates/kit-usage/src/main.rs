mod paths;
mod usage;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Anonymous OMP subscription usage")]
struct Cli {
    #[arg(long, env = "KIT_USAGE_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Snapshot {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        agents_working: bool,
        #[arg(long, env = "KIT_USAGE_OMP_EXECUTABLE")]
        omp_executable: Option<PathBuf>,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    match run(Cli::parse()).await {
        Ok(value) => println!("{}", serde_json::to_string(&value).expect("serializable response")),
        Err(error) => {
            let message: String = error.to_string().chars().filter(|c| !c.is_control()).take(512).collect();
            eprintln!("{}", serde_json::json!({"error":{"code":"operation_failed","message":message}}));
            std::process::exit(1);
        }
    }
}

async fn run(cli: Cli) -> Result<usage::Snapshot> {
    let root = cli.data_dir.map(Ok).unwrap_or_else(paths::data_dir)?;
    let root = if root.is_absolute() { root } else {
        std::env::current_dir().context("current directory unavailable")?.join(root)
    };
    match cli.command {
        Command::Snapshot { agents_working, omp_executable, .. } =>
            usage::snapshot(&root.join("usage"), agents_working, omp_executable.as_deref()).await,
    }
}
