mod coord;
mod deployment;

use clap::Parser;
use kit_core::paths::data_root;
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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    let result = match data_root(cli.data_dir) {
        Ok(root) => coord::run(&root, cli.args).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(coord::Output::Json(value)) => println!(
            "{}",
            serde_json::to_string(&value).expect("serializable response")
        ),
        Ok(coord::Output::Text(text)) => print!("{text}"),
        Err(error) => {
            let (code, exit) = coord::error::exit_of(&error);
            eprintln!("{}", kit_core::error::json(&error, code));
            std::process::exit(exit);
        }
    }
}
