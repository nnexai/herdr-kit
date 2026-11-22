pub mod adapter;
#[cfg(test)]
mod blocked_tests;
pub mod brief;
pub mod caller;
pub mod cli;
pub mod config;
pub mod conversation;
pub mod error;
pub mod exec;
pub mod fold;
pub mod git;
pub mod ids;
pub mod journal;
pub mod kinds;
pub mod launch;
pub mod lifecycle;
pub mod paths;
pub mod place;
pub mod reconcile;
pub mod release;
pub mod role;
pub mod rules;
pub mod state;
pub mod stats;
pub mod views;
pub mod watch;

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Ctx {
    pub root: PathBuf,
    pub paths: paths::CoordPaths,
    pub hc: Box<dyn adapter::Hc>,
    pub app: Box<dyn adapter::App>,
    pub clock: Box<dyn Fn() -> u64>,
    pub op: String,
    pub cause: Option<u64>,
}

impl Ctx {
    pub fn new(root: &Path) -> Result<Ctx> {
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()?.join(root)
        };
        Ok(Ctx {
            paths: paths::CoordPaths::new(&root),
            root,
            hc: Box::new(adapter::ProcHc::from_env()),
            app: Box::new(adapter::ProcApp::from_env()),
            op: new_op()?,
            cause: None,
            clock: Box::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX)
            }),
        })
    }

    pub fn now(&self) -> u64 {
        (self.clock)()
    }

    pub fn journal(&self) -> Result<journal::Journal> {
        journal::Journal::with_context(&self.paths, self.op.clone(), self.cause)
    }

    pub fn cli(&self) -> String {
        self.cli_argv()
            .iter()
            .map(|s| shell_quote(s))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn cli_argv(&self) -> Vec<String> {
        let stable = self.root.join("bin/kit-coord");
        let binary = if stable.is_file() {
            stable
        } else {
            std::env::current_exe().unwrap_or(stable)
        };
        let mut argv = vec![binary.to_string_lossy().into_owned()];
        if kit_core::paths::data_dir().ok().as_deref() != Some(self.root.as_path()) {
            argv.extend([
                "--data-dir".to_owned(),
                self.root.to_string_lossy().into_owned(),
            ]);
        }
        argv
    }

    pub fn config(&self) -> Result<config::Config> {
        Ok(config::Config::load(&self.paths)?.unwrap_or_default())
    }
}

pub fn new_op() -> Result<String> {
    let bytes = ids::random::<5>()?;
    let bits = bytes.into_iter().fold(0u64, |n, b| (n << 8) | u64::from(b));
    let alphabet = b"abcdefghijklmnopqrstuvwxyz234567";
    Ok((0..8)
        .rev()
        .map(|i| alphabet[((bits >> (i * 5)) & 31) as usize] as char)
        .collect())
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_+-./:=@".contains(&c))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub enum Output {
    Json(serde_json::Value),
    Text(String),
}

pub async fn run(root: &Path, args: cli::CoordArgs) -> Result<Output> {
    use caller::Caller;
    use cli::{ConfigCommand, CoordCommand as C, InboxCommand, RolesCommand, TaskCommand, WsCommand};
    use ids::{TaskId, WsId};
    use serde_json::json;
    use std::io::Read;

    let mut ctx = Ctx::new(root)?;
    ctx.cause = args.cause;
    let cmd = args.command;
    let read_only = matches!(
        &cmd,
        C::Status { .. }
            | C::Conversation { .. }
            | C::Board { .. }
            | C::Log { .. }
            | C::Explain { .. }
            | C::Roles { command: None, .. }
            | C::Ws {
                command: WsCommand::List { .. }
            }
            | C::Inbox { command: None, .. }
            | C::Stats { .. }
    );
    let caller = if read_only {
        Caller::Person { terminal: None }
    } else {
        caller::resolve(ctx.hc.as_ref())?
    };
    let person = matches!(caller, Caller::Person { .. });
    let coordinator = matches!(caller, Caller::Coordinator);
    let delegate = matches!(caller, Caller::Delegate { .. });
    let watcher = matches!(caller, Caller::Watcher);
    let allowed = if read_only {
        true
    } else {
        match &cmd {
            C::Up { .. }
            | C::Down
            | C::Restart { .. }
            | C::Rate { .. }
            | C::Config { .. }
            | C::Say { .. } => person,
            C::Handoff { .. } => coordinator,
            C::Report { .. } | C::Note { .. } => delegate,
            C::Watch => person || watcher,
            _ => person || coordinator,
        }
    };
    if !allowed {
        let code = if matches!(cmd, C::Report { .. } | C::Note { .. }) {
            "not_a_delegate"
        } else {
            "forbidden_caller"
        };
        return Err(error::refused(
            code,
            "caller is not permitted to run this command",
        ));
    }
    let value = match cmd {
        C::Up { if_enabled } => lifecycle::up(&ctx, if_enabled)?,
        C::Down => lifecycle::down(&ctx)?,
        C::Status { .. } => lifecycle::status(&ctx)?,
        C::Say { text, question } => conversation::say(&ctx, &caller, &text, question.as_deref())?,
        C::Conversation { limit, .. } => conversation::conversation(&ctx, usize::from(limit))?,
        C::Restart { force } => lifecycle::restart(&ctx, &caller, force)?,
        C::Handoff { note } => lifecycle::handoff(&ctx, &caller, note.as_deref())?,
        C::Watch => watch::run(ctx).await?,
        C::Ws { command } => match command {
            WsCommand::Open(a) => place::ws_open(&ctx, &caller, &a)?,
            WsCommand::Close {
                ws,
                summary,
                outcome,
            } => place::ws_close(&ctx, &caller, &WsId::parse(&ws)?, &summary, outcome.into())?,
            WsCommand::List { all } => place::ws_list(&ctx, all)?,
        },
        C::Task { command } => match command {
            TaskCommand::Start(a) => place::task_start(&ctx, &caller, &a)?,
            TaskCommand::Send { task, text, file } => {
                let text = if let Some(text) = text {
                    text
                } else {
                    let path = file.expect("clap requires a file or text");
                    let mut text = String::new();
                    if path == Path::new("-") {
                        std::io::stdin().take(1_048_577).read_to_string(&mut text)?;
                    } else {
                        std::fs::File::open(path)?
                            .take(1_048_577)
                            .read_to_string(&mut text)?;
                    }
                    anyhow::ensure!(text.len() <= 1_048_576, "message exceeds 1 MiB");
                    text
                };
                place::task_send(&ctx, &caller, &TaskId::parse(&task)?, &text)?
            }
            TaskCommand::Close {
                task,
                reason,
                outcome,
            } => place::task_close(
                &ctx,
                &caller,
                &TaskId::parse(&task)?,
                &reason,
                outcome.map(Into::into),
            )?,
        },
        C::Report {
            outcome,
            summary,
            result,
        } => place::report(&ctx, &caller, outcome, &summary, result.as_deref())?,
        C::Note { text } => place::note(&ctx, &caller, &text)?,
        C::Release { ws } => {
            let outcome = release::release_ws(
                &ctx,
                &ctx.journal()?,
                &caller.actor(),
                &WsId::parse(&ws)?,
                release::Mode::Command,
            )?;
            json!({"ws":ws,"cleaned":outcome.cleaned,"released":outcome.released,"retained":outcome.retained})
        }
        C::Decide(a) => place::decide(&ctx, &caller, &a)?,
        C::Ask(a) => place::ask(&ctx, &caller, &a)?,
        C::Instruct(a) => place::instruct(&ctx, &caller, &a)?,
        C::Thread { command } => place::thread(&ctx, &caller, &command)?,
        C::Correct(a) => place::correct(&ctx, &caller, &a)?,
        C::Scout(a) => place::scout(&ctx, &caller, &a)?,
        C::Inbox {
            command: Some(InboxCommand::Ack { seq }),
            ..
        } => place::inbox_ack(&ctx, &caller, seq)?,
        C::Inbox { command: None, .. } => {
            let j = ctx.journal()?;
            views::inbox(&j, &j.read_state()?)?
        }
        C::Board { json, all } => {
            let state = ctx.journal()?.read_state()?;
            return Ok(if json {
                Output::Json(views::board(&state, None, ctx.now(), all))
            } else {
                Output::Text(views::board_text(&state, ctx.now(), all))
            });
        }
        C::Log {
            json,
            limit,
            before_seq,
            after_seq,
            since,
            ws,
            task,
        } => {
            let (after_seq, since_ms) = match since {
                Some(s) => match s.parse::<u64>() {
                    Ok(seq) => (Some(after_seq.map_or(seq, |n| n.max(seq))), None),
                    Err(_) => (after_seq, Some(stats::parse_since(&s, ctx.now())?)),
                },
                None => (after_seq, None),
            };
            let q = journal::LogQuery {
                limit: usize::from(limit),
                before_seq,
                after_seq,
                since_ms,
                ws: ws.as_deref().map(WsId::parse).transpose()?,
                task: task.as_deref().map(TaskId::parse).transpose()?,
            };
            let j = ctx.journal()?;
            return Ok(if json {
                Output::Json(views::log(&j, &q, ctx.now())?)
            } else {
                Output::Text(views::log_text(&j.page(&q)?))
            });
        }
        C::Explain { id, json } => {
            let j = ctx.journal()?;
            let value = views::explain(&j, &j.read_state()?, &id, ctx.now())?;
            return Ok(if json {
                Output::Json(value)
            } else {
                Output::Text(views::explain_text(&value))
            });
        }
        C::Roles {
            defaults,
            name,
            command,
        } => match command {
            Some(RolesCommand::Create(args)) => {
                let created = role::create(&ctx.paths, &args)?;
                json!({"action": "created", "role": role::to_json(&created)})
            }
            Some(RolesCommand::Edit(args)) => {
                let updated = role::edit(&ctx.paths, &args)?;
                json!({"action": "updated", "role": role::to_json(&updated)})
            }
            Some(RolesCommand::Delete { name }) => {
                role::delete(&ctx.paths, &name)?;
                json!({"action": "deleted", "name": name})
            }
            Some(RolesCommand::Reset { name }) => {
                let restored = role::reset(&ctx.paths, &name)?;
                json!({"action": "reset", "role": role::to_json(&restored)})
            }
            None => {
                if let Some(name) = name {
                    let r = if defaults {
                        let text = role::builtin(&name).ok_or_else(|| {
                            error::refused("role_not_found", format!("unknown role: {name}"))
                        })?;
                        role::parse(&name, text, role::RoleSource::Builtin)?
                    } else {
                        role::load_or_not_found(&ctx.paths, &name)?
                    };
                    role::to_json(&r)
                } else {
                    let roles = if defaults {
                        role::BUILTIN
                            .iter()
                            .map(|(name, _)| ((*name).to_owned(), role::RoleSource::Builtin, false))
                            .collect()
                    } else {
                        role::list(&ctx.paths)?
                    };
                    let mut out = Vec::new();
                    for (name, _source, _overrides) in roles {
                        let r = if defaults {
                            role::parse(
                                &name,
                                role::builtin(&name).unwrap(),
                                role::RoleSource::Builtin,
                            )?
                        } else {
                            role::load(&ctx.paths, &name)?
                        };
                        out.push(role::to_json(&r));
                    }
                    json!({"roles": out})
                }
            }
        },
        C::Rate {
            target,
            value,
            note,
        } => place::rate(&ctx, &caller, &target, value.into(), note.as_deref())?,
        C::Config {
            command: ConfigCommand::Set { key, value },
        } => place::config_set(&ctx, &caller, &key, &value)?,
        C::Stats { since, .. } => place::stats(&ctx, since.as_deref())?,
    };
    Ok(Output::Json(value))
}
