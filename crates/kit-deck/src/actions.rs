use crate::sources::{SourceConfig, run_bounded};
use anyhow::{Context, Result};
use serde_json::json;
use std::time::Duration;
use tokio::process::Command;

pub async fn focus_terminal(cfg: &SourceConfig, terminal: &str) -> Result<()> {
    let mut cmd = Command::new(&cfg.herdr_app);
    cmd.args([
        "do",
        "agent.focus",
        &json!({ "terminal": terminal }).to_string(),
    ]);
    run_bounded(cmd, Duration::from_secs(10), 65_536)
        .await
        .context("herdr-app agent.focus failed")?;
    Ok(())
}

pub(crate) fn build_start_coordinator_cmd(cfg: &SourceConfig) -> Command {
    let mut cmd = Command::new(&cfg.kit_coord);
    cmd.arg("--data-dir").arg(&cfg.data_dir).arg("up");
    cmd
}

pub async fn start_coordinator(cfg: &SourceConfig) -> Result<()> {
    run_bounded(
        build_start_coordinator_cmd(cfg),
        Duration::from_secs(20),
        65_536,
    )
    .await
    .context("kit-coord up failed")?;
    Ok(())
}

pub(crate) fn build_answer_question_cmd(cfg: &SourceConfig, qid: &str, text: &str) -> Command {
    let mut cmd = Command::new(&cfg.kit_coord);
    cmd.arg("--data-dir")
        .arg(&cfg.data_dir)
        .args(["say", "--question", qid, "--text", text]);
    cmd
}

pub async fn answer_question(cfg: &SourceConfig, qid: &str, text: &str) -> Result<()> {
    run_bounded(
        build_answer_question_cmd(cfg, qid, text),
        Duration::from_secs(20),
        65_536,
    )
    .await
    .context("kit-coord say failed")?;
    Ok(())
}

pub async fn interrupt_terminal(cfg: &SourceConfig, terminal: &str) -> Result<()> {
    let mut cmd = Command::new(&cfg.hc);
    cmd.args(["interrupt", terminal]);
    run_bounded(cmd, Duration::from_secs(10), 65_536)
        .await
        .context("hc interrupt failed")?;
    Ok(())
}
