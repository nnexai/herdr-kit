use std::path::Path;

use super::role::Role;

pub const FOOTER: &str = r#"---
Reporting (fixed by the coordinator system):

- When you are done, or cannot continue, report exactly once:
  {cli} report done --summary "<one line>"
  {cli} report blocked --summary "<what blocks you>"
  {cli} report question --summary "<your question>"
  Long results go into {result}; add --result {result} to the report.
  Attach evidence files (screenshots, mocks, logs) with repeatable --attach <path> (or --attach-ref <path> for large/external references) on progress or final reports.
- Use `{cli} report progress --summary "<milestone>"` only for milestones; it never interrupts anyone.
- If the person changes the scope with you directly, record it with `{cli} note "<what changed>"`.
- Do not start other agents, create spaces or worktrees, or close anything. Do not report with `hc send`.
- After reporting, stop and wait: a follow-up may arrive in this session."#;

fn footer(cli: &str, result: &Path) -> String {
    FOOTER
        .replace("{result}", &result.to_string_lossy())
        .replace("{cli}", cli)
}

pub fn delegate_first(role: &Role, brief: &str, cli: &str, result: &Path) -> String {
    format!("{}\n\n{}", role.body, follow_up(brief, cli, result))
}

pub fn follow_up(brief: &str, cli: &str, result: &Path) -> String {
    format!("{}\n\n{}", brief, footer(cli, result))
}

/// The coordinator's product instructions; the same on every machine.
pub const COORDINATOR: &str = include_str!("coordinator.md");

/// The coordinator's first message: the product instructions, the role
/// guidance derived from this machine's role files, the coordinator role's
/// body as local notes, the command reference and the startup read.
pub fn coordinator_first(role: &Role, cli: &str, guidance: &str, startup_read: &str) -> String {
    const LIMIT: usize = 32 * 1024;
    let read = if startup_read.len() > LIMIT {
        let notice = format!("\n\nStartup read truncated; run `{cli} board` for the full board.");
        let mut end = LIMIT.saturating_sub(notice.len());
        while !startup_read.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{notice}", &startup_read[..end])
    } else {
        startup_read.to_owned()
    };
    let notes = if role.body.is_empty() {
        String::new()
    } else {
        format!("## Local coordinator notes\n\n{}\n\n", role.body)
    };
    format!(
        r#"{}

{}

{notes}## Commands
`kit` is: {cli}
Read commands: kit board | inbox | log [--ws WS] [--task TASK] | explain <ws|task> | artifacts list [--ws WS] [--task TASK] --json | roles [NAME] | ext [NAME] | stats --json [--since DURATION]
Coordinator commands (optional global --cause SEQ): kit inbox ack <SEQ> | ws open --title TEXT --target TEXT [--request TEXT] [--repo PATH] [--delivery-branch REF] [--purpose TEXT] | ws close <WS> --summary TEXT [--outcome delivered|partial|abandoned] | ws list [--all] | task start --ws WS --rung 1..5 --reason TEXT --brief FILE [--request TEXT] [--follows TASK --relation follow_up|rework|escalation] [--role NAME] [--reject N=REASON]... [--title TEXT] [--session-of TASK] [--space ID] [--branch NAME] [--base REF] [--folder PATH] | task send <TASK> (--text TEXT | --file FILE) | task close <TASK> --reason TEXT --outcome accepted|rejected|superseded|abandoned | release <WS> | decide --what TEXT --why TEXT --tier silent|digest|ask [--ws WS] [--task TASK] [--…

## Startup read
{read}"#,
        COORDINATOR.trim_end(),
        guidance.trim_end(),
    )
}

pub fn for_argv(message: String, saved_at: &Path) -> String {
    if message.len() > 96 * 1024 {
        format!(
            "Read your full brief in {} before doing anything else.",
            saved_at.display()
        )
    } else {
        message
    }
}

pub fn title_of(brief: &str) -> String {
    brief
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::role::{RoleSource, builtin, parse};

    #[test]
    fn messages_and_caps() {
        let role = parse(
            "coord-main",
            builtin("coord-main").unwrap(),
            RoleSource::Builtin,
        )
        .unwrap();
        let result = Path::new("/tmp/result.md");
        let first = delegate_first(&role, "task", "/bin/kit --data-dir /x", result);
        assert!(first.contains("/bin/kit --data-dir /x report done"));
        assert!(first.contains("/tmp/result.md"));
        assert!(follow_up("task", "kit", result).starts_with("task\n\n---"));
        assert_eq!(for_argv("x".repeat(96 * 1024), result).len(), 96 * 1024);
        assert_eq!(
            for_argv("x".repeat(96 * 1024 + 1), result),
            "Read your full brief in /tmp/result.md before doing anything else."
        );
        let guidance = "## Roles on this machine\n\n- `coord-default`: General.\n";
        let startup = coordinator_first(&role, "kit", guidance, &"é".repeat(20_000));
        assert!(startup.contains("run `kit board`"));
        assert!(!startup.contains(&"é".repeat(20_000)));
        let order = [
            "You are the coordinator",
            "## Roles on this machine",
            "## Commands",
            "## Startup read",
        ]
        .map(|h| startup.find(h).unwrap());
        assert!(order.is_sorted(), "{order:?}");
        assert!(!startup.contains("## Local coordinator notes"));
        let local = Role {
            body: "Prefer the person's own space.".into(),
            ..role
        };
        let noted = coordinator_first(&local, "kit", guidance, "read");
        assert!(noted.contains(
            "- `coord-default`: General.\n\n## Local coordinator notes\n\nPrefer the person's own space.\n\n## Commands"
        ));
        assert_eq!(title_of("\n  hello  \nmore"), "hello");
        assert_eq!(title_of(&"é".repeat(90)).chars().count(), 80);
    }
}
