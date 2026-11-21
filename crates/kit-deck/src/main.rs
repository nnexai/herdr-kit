mod actions;
mod bundle;
mod model;
mod plugin;
mod render;
mod sources;
#[cfg(test)]
mod tests;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Standalone OpenDeck (OpenAction) Stream Deck extension for Herdr"
)]
struct Cli {
    /// Optional Herdr kit data directory override.
    #[arg(long, env = "KIT_DECK_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Collect a live snapshot of Herdr agents, coordinator, and limits.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Install dev.herdr.kit.sdPlugin into OpenDeck's plugins directory.
    Install {
        #[arg(long)]
        plugins_dir: Option<PathBuf>,
        /// Reference the source binary via a wrapper script instead of copying it.
        #[arg(long)]
        wrapper: bool,
    },
    /// Remove dev.herdr.kit.sdPlugin from OpenDeck's plugins directory.
    Uninstall {
        #[arg(long)]
        plugins_dir: Option<PathBuf>,
    },
    /// Render a 5x3 Stream Deck MK.2 SVG + HTML preview directory.
    Preview {
        #[arg(long)]
        out: PathBuf,
        /// Use built-in sample data instead of querying live Herdr tools.
        #[arg(long)]
        fixture: bool,
    },
}

fn is_openaction_launch(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower == "-port" || lower == "-pluginuuid" || lower == "-registerevent"
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if is_openaction_launch(&args) {
        let cfg = match sources::SourceConfig::resolve(None) {
            Ok(cfg) => cfg,
            Err(error) => {
                eprintln!("{}", kit_core::error::json(&error, "config_failed"));
                std::process::exit(1);
            }
        };
        if let Err(error) = plugin::run(cfg, args).await {
            eprintln!("{}", kit_core::error::json(&error, "plugin_failed"));
            std::process::exit(1);
        }
        return;
    }

    match run_cli(Cli::parse()).await {
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

async fn run_cli(cli: Cli) -> Result<serde_json::Value> {
    let explicit_data_dir = cli.data_dir.clone();
    match cli.command {
        Command::Status { .. } => {
            let cfg = sources::SourceConfig::resolve(explicit_data_dir)?;
            let snap = sources::collect_once(&cfg).await;
            Ok(serde_json::to_value(snap)?)
        }
        Command::Install {
            plugins_dir,
            wrapper,
        } => {
            let dir = match plugins_dir {
                Some(p) => p,
                None => bundle::default_plugins_dir()?,
            };
            let exe = std::env::current_exe()?;
            let data_dir_abs = match explicit_data_dir {
                Some(d) => Some(kit_core::paths::data_root(Some(d))?),
                None => None,
            };
            let report = bundle::install(&dir, &exe, data_dir_abs.as_deref(), wrapper)?;
            Ok(serde_json::to_value(report)?)
        }
        Command::Uninstall { plugins_dir } => {
            let dir = match plugins_dir {
                Some(p) => p,
                None => bundle::default_plugins_dir()?,
            };
            let report = bundle::uninstall(&dir)?;
            Ok(serde_json::to_value(report)?)
        }
        Command::Preview { out, fixture } => {
            let snap = if fixture {
                model::fixture_snapshot(sources::now_ms())
            } else {
                let cfg = sources::SourceConfig::resolve(explicit_data_dir)?;
                sources::collect_once(&cfg).await
            };
            std::fs::create_dir_all(&out)?;
            let mut tiles: Vec<(String, String, String)> = Vec::with_capacity(15);
            // Row 0: Overview, Inbox, Coordinator, Agent 1, Agent 2
            tiles.push((
                "Overview".into(),
                "key-00-overview.svg".into(),
                render::render_overview(&snap),
            ));
            tiles.push((
                "Inbox".into(),
                "key-01-inbox.svg".into(),
                render::render_inbox(&snap, 0),
            ));
            tiles.push((
                "Coordinator".into(),
                "key-02-coordinator.svg".into(),
                render::render_coordinator(&snap),
            ));
            tiles.push((
                "Agent 1".into(),
                "key-03-agent1.svg".into(),
                render::render_agent_slot(&snap, 1, "all", None),
            ));
            tiles.push((
                "Agent 2".into(),
                "key-04-agent2.svg".into(),
                render::render_agent_slot(&snap, 2, "all", None),
            ));
            // Row 1: Codex, Claude, Copilot, Antigravity, Antigravity 5h Bucket
            for (idx, (id, label)) in [
                ("codex", "Codex"),
                ("claude", "Claude"),
                ("copilot", "Copilot"),
                ("antigravity", "Antigravity"),
            ]
            .into_iter()
            .enumerate()
            {
                tiles.push((
                    label.into(),
                    format!("key-{:02}-{id}.svg", 5 + idx),
                    render::render_limit_provider(&snap, id, "antigravity"),
                ));
            }
            tiles.push((
                "AGY Gem 5h".into(),
                "key-09-agy-5h.svg".into(),
                render::render_limit_bucket(&snap, "antigravity", "antigravity", 1),
            ));
            // Row 2: AGY Argon, Inbox Next, Inbox Focus, Reply 1, Agent 3
            tiles.push((
                "AGY Argon".into(),
                "key-10-agy-argon.svg".into(),
                render::render_limit_bucket(&snap, "antigravity", "antigravity", 4),
            ));
            tiles.push((
                "Inbox Next".into(),
                "key-11-inbox-next.svg".into(),
                render::render_inbox_action(&snap, 0, "next", false),
            ));
            tiles.push((
                "Inbox Focus".into(),
                "key-12-inbox-focus.svg".into(),
                render::render_inbox_action(&snap, 0, "focus", false),
            ));
            tiles.push((
                "Reply Opt 1".into(),
                "key-13-reply1.svg".into(),
                render::render_inbox_action(&snap, 0, "reply_1", false),
            ));
            tiles.push((
                "Agent 3".into(),
                "key-14-agent3.svg".into(),
                render::render_agent_slot(&snap, 3, "all", None),
            ));

            let mut refs = Vec::with_capacity(tiles.len());
            for (label, file, svg) in &tiles {
                std::fs::write(out.join(file), svg)?;
                refs.push((label.as_str(), file.as_str()));
            }
            let html = render::render_preview_html(&refs);
            std::fs::write(out.join("index.html"), html)?;
            Ok(json!({
                "generated": true,
                "out": out,
                "keys": tiles.len(),
            }))
        }
    }
}
