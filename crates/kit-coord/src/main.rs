mod coord;
mod deployment;
mod paths;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version = env!("KIT_COORD_VERSION"), about = "Coordinate agent work in any Herdr layout")]
struct Cli {
    /// Existing parent data root; coordinator data stays in its coord/ directory.
    #[arg(long, env = "KIT_COORD_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[command(flatten)]
    args: coord::cli::CoordArgs,
}

fn data_root(arg: Option<PathBuf>) -> Result<PathBuf> {
    let root = arg.map(Ok).unwrap_or_else(paths::data_dir)?;
    if root.is_absolute() { Ok(root) }
    else { Ok(std::env::current_dir().context("current directory unavailable")?.join(root)) }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    let result = match data_root(cli.data_dir) {
        Ok(root) => coord::run(&root, cli.args).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(coord::Output::Json(value)) => println!("{}", serde_json::to_string(&value).expect("serializable response")),
        Ok(coord::Output::Text(text)) => print!("{text}"),
        Err(error) => {
            let message: String = error.to_string().chars().filter(|c| !c.is_control()).take(512).collect();
            let (code, exit) = coord::error::exit_of(&error);
            eprintln!("{}", serde_json::json!({"error":{"code":code,"message":message}}));
            std::process::exit(exit);
        }
    }
}
