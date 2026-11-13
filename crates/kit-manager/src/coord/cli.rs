use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

/// Arguments shared by coordinator commands.
#[derive(Debug, Args)]
pub struct CoordArgs {
    /// Attribute this command's journal entries to a journal sequence number.
    #[arg(long, global = true)]
    pub cause: Option<u64>,
    /// Coordinator operation to run.
    #[command(subcommand)]
    pub command: CoordCommand,
}

/// Coordinate parallel agent work.
#[derive(Debug, Subcommand)]
pub enum CoordCommand {
    /// Start the watcher and coordinator if they are not already running.
    Up {
        /// Start only when coordinator autostart is enabled.
        #[arg(long)]
        if_enabled: bool,
    },
    /// Stop coordinator and watcher; disable autostart. Delegates keep running.
    Down,
    /// Show coordinator and watcher status.
    Status {
        /// Print status as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Replace the coordinator session with a fresh one when safe.
    Restart {
        /// Restart even when the coordinator is not idle.
        #[arg(long)]
        force: bool,
    },
    /// Request a fresh coordinator session after its current work is handed off.
    Handoff {
        /// Optional context to include with the handoff.
        #[arg(long)]
        note: Option<String>,
    },
    /// Show workstreams, tasks, and coordinator context.
    Board {
        /// Print the board as JSON.
        #[arg(long)]
        json: bool,
        /// Include closed workstreams and tasks.
        #[arg(long)]
        all: bool,
    },
    /// Read journal entries, optionally filtered by time or workstream/task.
    Log {
        /// Print entries as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum entries to return (1–500).
        #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u16).range(1..=500))]
        limit: u16,
        /// Return entries before this journal sequence number.
        #[arg(long)]
        before_seq: Option<u64>,
        /// Return entries after this journal sequence number.
        #[arg(long)]
        after_seq: Option<u64>,
        /// Return entries since this time.
        #[arg(long)]
        since: Option<String>,
        /// Filter entries to this workstream.
        #[arg(long)]
        ws: Option<String>,
        /// Filter entries to this task.
        #[arg(long)]
        task: Option<String>,
    },
    /// Explain a workstream or task by identifier.
    Explain {
        /// Workstream or task identifier to explain.
        id: String,
        /// Print the explanation as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read pending coordinator inbox events, or acknowledge an event.
    Inbox {
        /// Print inbox events as JSON.
        #[arg(long)]
        json: bool,
        /// Inbox action, such as acknowledging an event.
        #[command(subcommand)]
        command: Option<InboxCommand>,
    },
    /// List available coordinator roles.
    Roles {
        /// Print built-in role defaults.
        #[arg(long)]
        defaults: bool,
        /// Show details for one role.
        name: Option<String>,
    },
    /// Manage workstreams.
    Ws {
        /// Workstream operation.
        #[command(subcommand)]
        command: WsCommand,
    },
    /// Manage tasks within workstreams.
    Task {
        /// Task operation.
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Check and release resources owned by a workstream.
    Release {
        /// Workstream identifier.
        ws: String,
    },
    /// Record a coordinator decision and its rationale.
    Decide(Decide),
    /// Ask the person a question or record their answer.
    Ask(Ask),
    /// Set or remove a standing coordinator instruction.
    Instruct(Instruct),
    /// Open or close a coordinator discussion thread.
    Thread {
        /// Thread operation.
        #[command(subcommand)]
        command: ThreadCmd,
    },
    /// Record a correction to a placement, model choice, or cleanup.
    Correct(Correct),
    /// Record the result of a read-only placement investigation.
    Scout(Scout),
    /// Report a task outcome or progress update.
    Report {
        /// Report outcome: done, blocked, question, or progress.
        outcome: ReportKind,
        /// Short report summary.
        #[arg(long)]
        summary: String,
        /// Optional path to a longer result file.
        #[arg(long)]
        result: Option<PathBuf>,
    },
    /// Add a note to the journal.
    Note {
        /// Note text.
        text: String,
    },
    /// Show coordinator activity statistics.
    Stats {
        /// Print statistics as JSON.
        #[arg(long)]
        json: bool,
        /// Include activity since this time.
        #[arg(long)]
        since: Option<String>,
    },
    /// Rate a task or workstream outcome.
    Rate {
        /// Task or workstream identifier.
        target: String,
        /// Rating: good or bad.
        value: RatingKind,
        /// Optional rating note.
        #[arg(long)]
        note: Option<String>,
    },
    /// Change coordinator configuration.
    Config {
        /// Configuration operation.
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Run the coordinator watcher in the foreground.
    Watch,
}

#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    /// Mark inbox events through this sequence number as handled.
    Ack {
        /// Journal sequence number to acknowledge.
        seq: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Set a coordinator configuration value, including watcher durations in seconds.
    Set {
        /// Configuration key.
        key: String,
        /// Value to assign to the key.
        value: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum WsCommand {
    /// Open a workstream and optionally create its space or worktree.
    Open(WsOpen),
    /// Close a workstream and record its delivery outcome.
    Close {
        /// Workstream identifier.
        ws: String,
        /// Summary of the workstream's result.
        #[arg(long)]
        summary: String,
        /// Delivery outcome for the workstream.
        #[arg(long, value_enum, default_value = "delivered")]
        outcome: WsOutcomeArg,
    },
    /// List workstreams.
    List {
        /// Include closed workstreams.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// Place and start a task at a chosen escalation rung.
    Start(TaskStart),
    /// Send a message to a task's agent.
    Send {
        /// Task identifier.
        task: String,
        /// Text to send (mutually exclusive with --file).
        #[arg(long, required_unless_present = "file", conflicts_with = "file")]
        text: Option<String>,
        /// Read the message to send from a file.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Close a task with a reason and optional outcome.
    Close {
        /// Task identifier.
        task: String,
        /// Reason for closing the task.
        #[arg(long)]
        reason: String,
        /// Task outcome, if known.
        #[arg(long, value_enum)]
        outcome: Option<TaskOutcomeArg>,
    },
}

#[derive(Debug, Args)]
pub struct WsOpen {
    /// Workstream title.
    #[arg(long)]
    pub title: String,
    /// Delivery target or agent for the workstream.
    #[arg(long)]
    pub target: String,
    /// Repository associated with this workstream.
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// Branch intended to receive delivered work.
    #[arg(long)]
    pub delivery_branch: Option<String>,
    /// One-line purpose of the workstream.
    #[arg(long)]
    pub purpose: Option<String>,
    /// Initial request for the workstream.
    #[arg(long)]
    pub request: Option<String>,
}

#[derive(Debug, Args)]
pub struct TaskStart {
    /// Workstream identifier for this task.
    #[arg(long)]
    pub ws: String,
    /// Placement rung: 1 same session, 2 same space/new tab, 3 existing space,
    /// 4 new worktree, 5 new project. Give each skipped lower rung a rejection.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=5))]
    pub rung: u8,
    /// Why this placement rung is appropriate.
    #[arg(long)]
    pub reason: String,
    /// File containing the task brief.
    #[arg(long)]
    pub brief: PathBuf,
    /// Role to use; required for rungs 2–5.
    #[arg(long, required_if_eq_any([("rung", "2"), ("rung", "3"), ("rung", "4"), ("rung", "5")]))]
    pub role: Option<String>,
    /// Rejected lower rung and reason, in the form N=reason (repeatable).
    #[arg(long = "reject")]
    pub rejected: Vec<String>,
    /// Optional task title.
    #[arg(long)]
    pub title: Option<String>,
    /// Existing task/session this task resumes or continues.
    #[arg(long)]
    pub session_of: Option<String>,
    /// Existing space to use for placement.
    #[arg(long)]
    pub space: Option<String>,
    /// Branch to use for placement.
    #[arg(long)]
    pub branch: Option<String>,
    /// Base branch or revision for a new worktree.
    #[arg(long)]
    pub base: Option<String>,
    /// Folder to use as the task's working directory.
    #[arg(long)]
    pub folder: Option<PathBuf>,
    /// Request or additional instruction for the task.
    #[arg(long)]
    pub request: Option<String>,
    /// Identifier of the task this task follows.
    #[arg(long, requires = "relation")]
    pub follows: Option<String>,
    /// Relationship to the task named by --follows.
    #[arg(long, requires = "follows")]
    pub relation: Option<RelationArg>,
}

#[derive(Debug, Args)]
pub struct Decide {
    /// Decision being made.
    #[arg(long)]
    pub what: String,
    /// Reason for the decision.
    #[arg(long)]
    pub why: String,
    /// How to surface the decision: silent, digest, or ask.
    #[arg(long)]
    pub tier: DecisionTier,
    /// Related workstream identifier.
    #[arg(long)]
    pub ws: Option<String>,
    /// Related task identifier.
    #[arg(long)]
    pub task: Option<String>,
    /// Candidate option (repeat to record multiple options).
    #[arg(long = "option")]
    pub options: Vec<String>,
}

#[derive(Debug, Args)]
pub struct Ask {
    /// Record the person's answer instead of asking a question.
    #[arg(long, conflicts_with_all = ["ws", "task", "options"])]
    pub answer: Option<String>,
    /// Related workstream identifier.
    #[arg(long)]
    pub ws: Option<String>,
    /// Related task identifier.
    #[arg(long)]
    pub task: Option<String>,
    /// Candidate answer option (repeat to record multiple options).
    #[arg(long = "option")]
    pub options: Vec<String>,
    /// Question text when asking, or context when recording an answer.
    pub text: String,
}

#[derive(Debug, Args)]
pub struct Correct {
    /// Area being corrected: placement, model, cleanup, or other.
    #[arg(long)]
    pub what: CorrectionKind,
    /// Identifier of the item or decision being corrected.
    #[arg(long = "ref")]
    pub reference: String,
    /// Reason for the correction.
    #[arg(long)]
    pub why: String,
}

#[derive(Debug, Args)]
pub struct Scout {
    /// Question the read-only scout should investigate.
    #[arg(long)]
    pub question: String,
    /// Summary of the scout's findings.
    #[arg(long)]
    pub summary: String,
    /// Related workstream identifier.
    #[arg(long)]
    pub ws: Option<String>,
}

#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("instruction").required(true).args(["text", "remove"])))]
pub struct Instruct {
    /// Replace the instruction identified by this text.
    #[arg(long, requires = "text")]
    pub replace: Option<String>,
    /// Remove the instruction identified by this text.
    #[arg(long, conflicts_with_all = ["text", "replace"])]
    pub remove: Option<String>,
    /// Instruction text to set.
    pub text: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum ThreadCmd {
    /// Open a discussion thread.
    Open {
        /// Thread title.
        title: String,
        /// Optional note about the thread.
        #[arg(long)]
        note: Option<String>,
    },
    /// Close a discussion thread.
    Close {
        /// Thread identifier.
        id: String,
        /// Optional closing note.
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DecisionTier {
    /// Keep the decision in the journal without surfacing it.
    Silent,
    /// Include the decision in the coordinator's digest.
    Digest,
    /// Ask the person to review or answer.
    Ask,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CorrectionKind {
    /// Correct a work placement.
    Placement,
    /// Correct a model or role choice.
    Model,
    /// Correct a cleanup decision.
    Cleanup,
    /// Correct another kind of decision.
    Other,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ReportKind {
    /// Task is complete.
    Done,
    /// Task is blocked.
    Blocked,
    /// Task needs an answer or decision.
    Question,
    /// Task is still in progress.
    Progress,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum WsOutcomeArg {
    /// Workstream delivered successfully.
    Delivered,
    /// Workstream delivered partially.
    Partial,
    /// Workstream was abandoned.
    Abandoned,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TaskOutcomeArg {
    /// Task was accepted.
    Accepted,
    /// Task was rejected.
    Rejected,
    /// Task was superseded by other work.
    Superseded,
    /// Task was abandoned.
    Abandoned,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RatingKind {
    /// Positive rating.
    Good,
    /// Negative rating.
    Bad,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RelationArg {
    /// This task is a follow-up to the referenced task.
    #[value(name = "follow_up")]
    FollowUp,
    /// This task reworks the referenced task.
    Rework,
    /// This task escalates the referenced task.
    Escalation,
}

impl From<WsOutcomeArg> for super::kinds::WsOutcome {
    fn from(value: WsOutcomeArg) -> Self {
        match value {
            WsOutcomeArg::Delivered => Self::Delivered,
            WsOutcomeArg::Partial => Self::Partial,
            WsOutcomeArg::Abandoned => Self::Abandoned,
        }
    }
}
impl From<TaskOutcomeArg> for super::kinds::TaskOutcome {
    fn from(value: TaskOutcomeArg) -> Self {
        match value {
            TaskOutcomeArg::Accepted => Self::Accepted,
            TaskOutcomeArg::Rejected => Self::Rejected,
            TaskOutcomeArg::Superseded => Self::Superseded,
            TaskOutcomeArg::Abandoned => Self::Abandoned,
        }
    }
}
impl From<RatingKind> for super::kinds::RatingValue {
    fn from(value: RatingKind) -> Self {
        match value {
            RatingKind::Good => Self::Good,
            RatingKind::Bad => Self::Bad,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(subcommand)]
        command: CoordCommand,
    }

    #[derive(Debug, Parser)]
    struct ScopedCli {
        #[command(flatten)]
        coord: CoordArgs,
    }
    #[test]
    fn insight_flags_parse_through_nested_commands() {
        let parsed = ScopedCli::try_parse_from([
            "coord",
            "task",
            "start",
            "--cause",
            "42",
            "--ws",
            "ws-abcdef",
            "--rung",
            "1",
            "--reason",
            "reuse",
            "--brief",
            "brief",
            "--request",
            "please",
            "--follows",
            "ws-abcdef-t1",
            "--relation",
            "follow_up",
        ])
        .unwrap();
        assert_eq!(parsed.coord.cause, Some(42));
        assert!(matches!(
            parsed.coord.command,
            CoordCommand::Task {
                command: TaskCommand::Start(TaskStart {
                    request: Some(_),
                    follows: Some(_),
                    relation: Some(RelationArg::FollowUp),
                    ..
                })
            }
        ));
        assert!(ScopedCli::try_parse_from(["coord", "stats", "--json", "--since", "30d"]).is_ok());
        assert!(
            ScopedCli::try_parse_from(["coord", "rate", "ws-abcdef", "good", "--note", "great"])
                .is_ok()
        );
        assert!(ScopedCli::try_parse_from(["coord", "config", "set", "stall_secs", "30"]).is_ok());
    }

    fn parses(args: &[&str]) -> bool {
        Cli::try_parse_from(std::iter::once("coord").chain(args.iter().copied())).is_ok()
    }

    #[test]
    fn command_tree_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn placement_role_and_ranges() {
        let start = [
            "task", "start", "--ws", "ws-test", "--rung", "1", "--reason", "reuse", "--brief", "-",
        ];
        assert!(parses(&start));
        assert!(parses(&[
            "task",
            "start",
            "--ws",
            "ws-test",
            "--rung",
            "2",
            "--reason",
            "new",
            "--brief",
            "brief.md",
            "--role",
            "coord-default"
        ]));
        assert!(!parses(&[
            "task", "start", "--ws", "ws-test", "--rung", "2", "--reason", "new", "--brief", "-"
        ]));
        assert!(!parses(&[
            "task",
            "start",
            "--ws",
            "ws-test",
            "--rung",
            "6",
            "--reason",
            "new",
            "--brief",
            "-",
            "--role",
            "coord-default"
        ]));
        assert!(parses(&["log", "--limit", "1"]));
        assert!(parses(&["log", "--limit", "500"]));
        assert!(!parses(&["log", "--limit", "0"]));
        assert!(!parses(&["log", "--limit", "501"]));
    }

    #[test]
    fn send_requires_exactly_one_source() {
        assert!(parses(&["task", "send", "task-id", "--text", "hello"]));
        assert!(parses(&["task", "send", "task-id", "--file", "-"]));
        assert!(!parses(&["task", "send", "task-id"]));
        assert!(!parses(&[
            "task", "send", "task-id", "--text", "hello", "--file", "x"
        ]));
    }

    #[test]
    fn instruction_forms_are_exclusive() {
        assert!(parses(&["instruct", "remember"]));
        assert!(parses(&["instruct", "--replace", "id", "remember"]));
        assert!(parses(&["instruct", "--remove", "id"]));
        assert!(!parses(&["instruct"]));
        assert!(!parses(&["instruct", "--replace", "id"]));
        assert!(!parses(&["instruct", "--remove", "id", "remember"]));
        assert!(!parses(&[
            "instruct",
            "--remove",
            "id",
            "--replace",
            "other"
        ]));
    }

    #[test]
    fn answer_disallows_question_context() {
        assert!(parses(&["ask", "--answer", "qid", "yes"]));
        assert!(parses(&[
            "ask", "--ws", "ws", "--task", "task", "--option", "one", "--option", "two", "question"
        ]));
        assert!(!parses(&["ask", "--answer", "qid"]));
        for option in ["--ws", "--task", "--option"] {
            assert!(!parses(&[
                "ask", "--answer", "qid", option, "value", "answer"
            ]));
        }
    }

    #[test]
    fn nested_and_value_enum_commands() {
        assert!(parses(&["inbox"]));
        assert!(parses(&["inbox", "--json"]));
        assert!(parses(&["inbox", "ack", "42"]));
        assert!(parses(&[
            "ws", "open", "--title", "title", "--target", "target"
        ]));
        assert!(parses(&["thread", "open", "topic", "--note", "note"]));
        assert!(parses(&[
            "decide", "--what", "a", "--why", "b", "--tier", "digest"
        ]));
        assert!(!parses(&[
            "decide", "--what", "a", "--why", "b", "--tier", "unknown"
        ]));
        assert!(parses(&[
            "correct",
            "--what",
            "placement",
            "--ref",
            "ws",
            "--why",
            "reason"
        ]));
        assert!(parses(&["report", "progress", "--summary", "milestone"]));
        assert!(!parses(&["report", "unknown", "--summary", "milestone"]));
    }
}
