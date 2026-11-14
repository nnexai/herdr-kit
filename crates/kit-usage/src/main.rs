mod usage;

use anyhow::Result;
use clap::{Parser, Subcommand};
use kit_core::paths::data_root;
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
        Ok(value) => println!(
            "{}",
            serde_json::to_string(&value).expect("serializable response")
        ),
        Err(error) => {
            eprintln!("{}", kit_core::error::json(&error, "operation_failed"));
            std::process::exit(1);
        }
    }
}

async fn run(cli: Cli) -> Result<usage::Snapshot> {
    let root = data_root(cli.data_dir)?;
    match cli.command {
        Command::Snapshot {
            agents_working,
            omp_executable,
            ..
        } => {
            usage::snapshot(
                &root.join("usage"),
                agents_working,
                omp_executable.as_deref(),
            )
            .await
        }
    }
}
