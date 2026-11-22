use super::adapter::{AppSnapshot, Term};
use super::caller::Caller;
use super::ids::{TaskId, WsId};
use super::journal::Journal;
use super::kinds::{
    AttentionState, CorrectionWhat, Follows, Kind, Outcome, Rejected, Relation, ResType,
    ResourceRef, SendVia, SessionMode, SessionRef, TaskOutcome, Tier, WsOutcome,
};
use super::state::{State, Task, TaskState, Ws, WsState};
use super::{Ctx, brief, cli, error, git, ids, launch, release, role};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

fn refuse(code: &'static str, msg: impl Into<String>) -> anyhow::Error {
    error::refused(code, msg)
}
pub fn role_hash(role: &role::Role) -> Result<String> {
    let text = match &role.source {
        role::RoleSource::Builtin => role::builtin(&role.name)
            .context("built-in role missing")?
            .as_bytes()
            .to_vec(),
        role::RoleSource::File(path) => fs::read(path)?,
    };
    Ok(format!("{:x}", Sha256::digest(text))[..12].to_owned())
}
fn append(
    ctx: &Ctx,
    c: &Caller,
    ws: Option<WsId>,
    task: Option<TaskId>,
    kind: Kind,
) -> Result<Value> {
    let j = ctx.journal()?;
    let entries = j
        .lock()?
        .append(&c.actor(), ctx.now(), vec![(ws, task, kind)])?;
    Ok(json!({"seq":entries[0].seq}))
}
fn read_brief(path: &Path) -> Result<String> {
    let mut text = String::new();
    if path == Path::new("-") {
        io::stdin().take(1_048_577).read_to_string(&mut text)?;
    } else {
        fs::File::open(path)?
            .take(1_048_577)
            .read_to_string(&mut text)?;
    }
    anyhow::ensure!(text.len() <= 1_048_576, "brief exceeds 1 MiB");
    Ok(text)
}
pub(super) fn send_guard(term: &super::adapter::Term) -> Result<()> {
    if matches!(term.word(), Some("blocked" | "error")) {
        return Err(refuse("agent_blocked", "agent is blocked"));
    }
    if !term.sendable() {
        return Err(error::not_ready(
            "agent_not_ready",
            "agent has not completed a settled turn",
        ));
    }
    Ok(())
}
fn launch_failed(
    ctx: &Ctx,
    j: &Journal,
    c: &Caller,
    ws: &WsId,
    task: &TaskId,
    error: &anyhow::Error,
) -> Result<()> {
    let mut lock = j.lock()?;
    let mut entries = vec![(
        Some(ws.clone()),
        Some(task.clone()),
        Kind::TaskClosed {
            reason: format!("launch failed: {error}"),
            outcome: Some(TaskOutcome::Abandoned),
        },
    )];
    for intent in lock
        .state()
        .intents
        .values()
        .filter(|i| i.task.as_ref() == Some(task))
    {
        entries.push((
            Some(ws.clone()),
            Some(task.clone()),
            Kind::ResReleased {
                r#type: intent.r.r#type,
                name: intent.r.name.clone(),
                note: Some("never created".into()),
            },
        ));
    }
    lock.append(&c.actor(), ctx.now(), entries)?;
    Ok(())
}
pub fn ws_open(ctx: &Ctx, c: &Caller, a: &cli::WsOpen) -> Result<Value> {
    let repo = a.repo.as_deref().map(git::main_checkout).transpose()?;
    let branch = match (&repo, &a.delivery_branch) {
        (_, Some(b)) => Some(b.clone()),
        (Some(r), None) => git::current_branch(r)?,
        _ => None,
    };
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let id = loop {
        let id = WsId::generate()?;
        if !lock.state().workstreams.contains_key(&id) {
            break id;
        }
    };
    lock.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(id.clone()),
            None,
            Kind::WsOpened {
                title: a.title.clone(),
                repo,
                delivery_target: Some(a.target.clone()),
                delivery_branch: branch,
                purpose: a.purpose.clone(),
                request: a.request.clone(),
            },
        )],
    )?;
    Ok(json!({"ws":id}))
}
pub fn ws_close(
    ctx: &Ctx,
    c: &Caller,
    ws: &WsId,
    summary: &str,
    outcome: WsOutcome,
) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let w = l
        .state()
        .workstreams
        .get(ws)
        .ok_or_else(|| refuse("unknown_ws", "unknown workstream"))?;
    if w.state != WsState::Open {
        return Err(refuse("ws_not_open", "workstream is not open"));
    }
    if w.tasks.iter().any(|t| {
        l.state()
            .tasks
            .get(t)
            .is_some_and(|t| matches!(t.state, TaskState::Placed | TaskState::Running))
    }) {
        return Err(refuse("tasks_running", "workstream has running tasks"));
    }
    let entries = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(ws.clone()),
            None,
            Kind::WsClosed {
                summary: summary.into(),
                outcome: Some(outcome),
            },
        )],
    )?;
    Ok(json!({"seq":entries[0].seq}))
}
pub fn ws_list(ctx: &Ctx, all: bool) -> Result<Value> {
    let state = ctx.journal()?.read_state()?;
    Ok(
        json!({"workstreams":state.workstreams.values().filter(|w| all || w.state != WsState::Cleaned).collect::<Vec<_>>()}),
    )
}

/// Where a rung places the task, with the arguments only that rung accepts.
#[derive(Clone, Copy)]
enum Target<'a> {
    /// Rung 1: continue a previous task's session.
    Session,
    /// Rung 2: another tab in the workstream's space.
    WorkstreamSpace,
    /// Rung 3: an existing space of the person.
    Space(&'a str),
    /// Rung 4: a new worktree in its own space.
    Worktree {
        branch: &'a str,
        base: Option<&'a str>,
    },
    /// Rung 5: a new project folder in its own space.
    Folder(&'a Path),
}
impl<'a> Target<'a> {
    fn parse(a: &'a cli::TaskStart) -> Result<Self> {
        let given = (
            a.rung,
            a.session_of.is_some(),
            a.space.as_deref(),
            a.branch.as_deref(),
            a.base.as_deref(),
            a.folder.as_deref(),
        );
        Ok(match given {
            (1, true, None, None, None, None) => Self::Session,
            (2, false, None, None, None, None) => Self::WorkstreamSpace,
            (3, false, Some(space), None, None, None) => Self::Space(space),
            (4, false, None, Some(branch), base, None) => Self::Worktree { branch, base },
            (5, false, None, None, None, Some(folder)) if folder.is_absolute() => {
                Self::Folder(folder)
            }
            _ => return Err(refuse("rung_arguments", "invalid rung arguments")),
        })
    }
    fn creates_space(self) -> bool {
        matches!(self, Self::Worktree { .. } | Self::Folder(_))
    }
}

/// Every rung below the chosen one must be rejected with a reason.
fn rejected_rungs(a: &cli::TaskStart) -> Result<Vec<Rejected>> {
    if !(1..=5).contains(&a.rung) || a.reason.trim().is_empty() {
        return Err(refuse("rung_arguments", "rung and reason required"));
    }
    let mut rejected: Vec<Rejected> = Vec::new();
    for item in &a.rejected {
        let (n, reason) = item
            .split_once('=')
            .ok_or_else(|| refuse("ladder_reason_extra", "invalid rejection"))?;
        let n: u8 = n
            .parse()
            .map_err(|_| refuse("ladder_reason_extra", "invalid rejection rung"))?;
        if n == 0 || n >= a.rung || rejected.iter().any(|r| r.rung == n) {
            return Err(refuse("ladder_reason_extra", "unexpected rejection"));
        }
        if reason.trim().is_empty() {
            return Err(refuse("ladder_reason_missing", "empty rejection reason"));
        }
        rejected.push(Rejected {
            rung: n,
            reason: reason.into(),
        });
    }
    if rejected.len() != usize::from(a.rung - 1) {
        return Err(refuse(
            "ladder_reason_missing",
            "a reason for each lower rung is required",
        ));
    }
    rejected.sort_by_key(|r| r.rung);
    Ok(rejected)
}

/// The task whose session `--session-of` continues.
fn predecessor(state: &State, ws: &WsId, a: &cli::TaskStart) -> Result<Option<Task>> {
    let Some(id) = &a.session_of else {
        return Ok(None);
    };
    let previous = state
        .tasks
        .get(&TaskId::parse(id)?)
        .cloned()
        .ok_or_else(|| refuse("no_session", "unknown predecessor"))?;
    let resumable = previous.session.as_ref().is_some_and(|s| s.id.is_some());
    if previous.ws != *ws || !resumable {
        return Err(refuse("no_session", "predecessor has no session"));
    }
    if a.role.as_ref().is_some_and(|r| *r != previous.role) {
        return Err(refuse(
            "role_mismatch",
            "resumed session must keep its role",
        ));
    }
    Ok(Some(previous))
}

/// The explicit `--follows` relation, or a follow-up of the continued session.
fn followed_task(
    state: &State,
    a: &cli::TaskStart,
    previous: Option<&Task>,
) -> Result<Option<Follows>> {
    match (&a.follows, a.relation) {
        (Some(task), Some(relation)) => {
            let task = TaskId::parse(task)?;
            if !state.tasks.contains_key(&task) {
                return Err(refuse("unknown_task", "followed task does not exist"));
            }
            let relation = match relation {
                cli::RelationArg::FollowUp => Relation::FollowUp,
                cli::RelationArg::Rework => Relation::Rework,
                cli::RelationArg::Escalation => Relation::Escalation,
            };
            Ok(Some(Follows { task, relation }))
        }
        (None, None) => Ok(previous.map(|p| Follows {
            task: p.id.clone(),
            relation: Relation::FollowUp,
        })),
        _ => Err(refuse(
            "rung_arguments",
            "follows and relation must be supplied together",
        )),
    }
}

/// The task's terminal while its recorded agent process is still running there.
fn live_terminal(ctx: &Ctx, task: &Task) -> Result<Option<Term>> {
    let Some(address) = &task.terminal else {
        return Ok(None);
    };
    Ok(ctx
        .hc
        .get(address)?
        .filter(|term| term.live() && term.pid == task.pid))
}

/// Only a managed terminal that is labelled for the task and recorded as its
/// owned tab may receive coordinator input.
fn owns_terminal(state: &State, term: &Term, task: &TaskId) -> bool {
    term.managed()
        && term.label("coord.task") == Some(task.as_str())
        && state.resources.iter().any(|r| {
            r.owned
                && !r.released
                && r.r.r#type == ResType::Tab
                && r.r.name == term.address
                && r.task.as_ref() == Some(task)
        })
}

/// A repository is trusted once the person has it open themselves: in a
/// space the coordinator does not own, either live or adopted earlier.
fn repo_trusted(state: &State, snapshot: &AppSnapshot, repo: &Path) -> bool {
    let checkout_of = |folder: &Path| git::same_repo(folder, repo);
    let owned_space = |id: &str| {
        state
            .resources
            .iter()
            .any(|r| r.r.r#type == ResType::Space && r.r.name == id && r.owned && !r.released)
    };
    snapshot
        .spaces
        .iter()
        .any(|s| !owned_space(&s.id) && checkout_of(Path::new(&s.folder)))
        || state.resources.iter().any(|r| {
            r.r.r#type == ResType::Space
                && !r.owned
                && r.r.folder.as_deref().is_some_and(&checkout_of)
        })
}

/// Apply a role's `trust:` override only when the resolved launch directory is
/// proven to belong to the workstream's already-trusted repository: either a
/// coordinator worktree being created for it, or an existing checkout/worktree
/// whose git identity resolves to the same repository.
fn launch_trusted(
    state: &State,
    snapshot: &AppSnapshot,
    repo: &Path,
    cwd: &Path,
    creating_worktree: bool,
) -> bool {
    (creating_worktree || git::same_repo(cwd, repo)) && repo_trusted(state, snapshot, repo)
}

struct Placement {
    space: String,
    cwd: PathBuf,
    /// Name for a created space, or the current name of an adopted one.
    space_label: Option<String>,
}

fn placement(
    ctx: &Ctx,
    target: Target<'_>,
    state: &State,
    w: &Ws,
    previous: Option<&Task>,
    snapshot: &AppSnapshot,
) -> Result<Placement> {
    let missing = |what: &'static str| refuse("no_space", what);
    Ok(match target {
        Target::Session => {
            let p = previous.context("rung 1 continues a predecessor")?;
            Placement {
                space: p
                    .space
                    .clone()
                    .ok_or_else(|| missing("predecessor has no space"))?,
                cwd: p
                    .cwd
                    .clone()
                    .ok_or_else(|| missing("predecessor has no cwd"))?,
                space_label: None,
            }
        }
        Target::WorkstreamSpace => Placement {
            space: w
                .space
                .as_ref()
                .ok_or_else(|| missing("workstream has no space"))?
                .id
                .clone(),
            cwd: w
                .cwd
                .clone()
                .ok_or_else(|| missing("workstream has no cwd"))?,
            space_label: None,
        },
        Target::Space(id) => {
            let s = snapshot
                .space(id)
                .ok_or_else(|| missing("space not found"))?;
            if state.coordinator.space.as_deref() == Some(&s.id) {
                return Err(refuse("coordinator_space", "cannot use coordinator space"));
            }
            Placement {
                space: s.id.clone(),
                cwd: PathBuf::from(&s.folder),
                space_label: Some(s.name.clone()),
            }
        }
        Target::Worktree { branch, .. } => {
            let repo = w
                .repo
                .as_ref()
                .ok_or_else(|| refuse("rung_arguments", "worktree needs a repo"))?;
            Placement {
                space: ids::space_id()?,
                cwd: git::worktree_path(&ctx.config()?.worktree_root()?, repo, branch),
                space_label: Some(format!(
                    "{} · {}",
                    repo.file_name().unwrap_or_default().to_string_lossy(),
                    w.title
                )),
            }
        }
        Target::Folder(folder) => Placement {
            space: ids::space_id()?,
            cwd: folder.to_owned(),
            space_label: Some(w.title.clone()),
        },
    })
}

fn new_session(ctx: &Ctx, role: &role::Role, id: &TaskId) -> Result<SessionRef> {
    let dir = (role.session == SessionMode::Dir).then(|| ctx.paths.session_dir(id.as_str()));
    if let Some(dir) = &dir {
        fs::create_dir_all(dir)?;
    }
    let id = if role.session == SessionMode::Preset {
        Some(ids::uuid_v4()?)
    } else {
        None
    };
    Ok(SessionRef {
        mode: role.session,
        id,
        dir,
    })
}

/// The rung 4 worktree, followed by its branch when that has to be created too.
fn worktree_intents(
    repo: &Path,
    branch: &str,
    base: Option<&str>,
    delivery_branch: Option<&str>,
    cwd: &Path,
) -> Result<Vec<ResourceRef>> {
    let create_branch = !git::branch_exists(repo, branch)?;
    let mut worktree = ResourceRef::new(ResType::Worktree, cwd.to_string_lossy());
    worktree.repo = Some(repo.to_owned());
    worktree.branch = Some(branch.to_owned());
    worktree.base = base.map(str::to_owned);
    worktree.create_branch = Some(create_branch);
    let mut intents = vec![worktree];
    if create_branch {
        let start = base.or(delivery_branch).unwrap_or("HEAD");
        let mut new_branch = ResourceRef::new(ResType::Branch, branch);
        new_branch.repo = Some(repo.to_owned());
        new_branch.base_sha = Some(git::rev(repo, start)?);
        intents.push(new_branch);
    }
    Ok(intents)
}

/// Add the planned worktree and return the resources that now exist.
fn create_worktree(
    repo: &Path,
    branch: &str,
    cwd: &Path,
    intents: &[ResourceRef],
) -> Result<Vec<ResourceRef>> {
    let of_type = |kind| intents.iter().find(|r| r.r#type == kind);
    let new_branch = of_type(ResType::Branch);
    if let Some(parent) = cwd.parent() {
        fs::create_dir_all(parent)?;
    }
    git::worktree_add(
        repo,
        cwd,
        branch,
        new_branch.and_then(|r| r.base_sha.as_deref()),
    )?;
    let mut created: Vec<ResourceRef> = of_type(ResType::Worktree).cloned().into_iter().collect();
    if let Some(intent) = new_branch {
        let mut branch = intent.clone();
        branch.base = branch.base_sha.clone();
        created.push(branch);
    }
    Ok(created)
}

pub fn task_start(ctx: &Ctx, c: &Caller, a: &cli::TaskStart) -> Result<Value> {
    let ws = WsId::parse(&a.ws)?;
    let rejected = rejected_rungs(a)?;
    let target = Target::parse(a)?;
    let snapshot = ctx.app.ls()?;
    let input = read_brief(&a.brief)?;
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let state = l.state();
    let w = state
        .workstreams
        .get(&ws)
        .ok_or_else(|| refuse("unknown_ws", "unknown workstream"))?;
    if w.state != WsState::Open {
        return Err(refuse("ws_not_open", "workstream is not open"));
    }
    if target.creates_space() && !w.tasks.is_empty() {
        return Err(refuse(
            "follow_up_cannot_create_space",
            "follow-up cannot create a space",
        ));
    }
    let previous = predecessor(state, &ws, a)?;
    let role_name = a
        .role
        .as_deref()
        .or_else(|| previous.as_ref().map(|p| p.role.as_str()))
        .ok_or_else(|| refuse("role_invalid", "role required"))?;
    let role =
        role::load(&ctx.paths, role_name).map_err(|e| refuse("role_invalid", e.to_string()))?;
    let follows = followed_task(state, a, previous.as_ref())?;
    let role_hash = role_hash(&role)?;
    let last_number = w.tasks.iter().map(TaskId::n).max().unwrap_or(0);
    let id = TaskId::new(&ws, last_number.checked_add(1).context("task id overflow")?);
    let title = a.title.clone().unwrap_or_else(|| brief::title_of(&input));
    // A predecessor whose agent is still running takes the brief as its next
    // turn; otherwise the role command starts or resumes a session in a new tab.
    let live_previous = match &previous {
        Some(p) => live_terminal(ctx, p)?,
        None => None,
    };
    if let Some(term) = &live_previous {
        send_guard(term)?;
    }
    let Placement {
        space,
        cwd,
        space_label,
    } = placement(ctx, target, state, w, previous.as_ref(), &snapshot)?;
    let new_space = target.creates_space();
    let creating_worktree = matches!(target, Target::Worktree { .. });
    let trusted = w
        .repo
        .as_deref()
        .is_some_and(|repo| launch_trusted(state, &snapshot, repo, &cwd, creating_worktree));
    let repo = w.repo.clone();
    let delivery_branch = w.delivery_branch.clone();
    let session = match &previous {
        Some(p) => p.session.clone(),
        None => Some(new_session(ctx, &role, &id)?),
    };
    let message = if previous.is_some() {
        brief::follow_up(&input, &ctx.cli(), &ctx.paths.result(&id))
    } else {
        brief::delegate_first(&role, &input, &ctx.cli(), &ctx.paths.result(&id))
    };
    let brief_path = ctx.paths.brief(id.as_str());
    fs::write(&brief_path, &message)?;
    let cold_resume = previous.is_some() && live_previous.is_none();
    let template = if cold_resume {
        role.resume
            .as_ref()
            .ok_or_else(|| refuse("no_session", "role cannot resume"))?
    } else {
        &role.start
    };
    if live_previous.is_none() && !template.iter().any(|arg| arg.contains("{brief}")) {
        return Err(refuse("role_invalid", "role command must include {brief}"));
    }
    let argv_message = brief::for_argv(message.clone(), &brief_path);
    let vars = role::Vars {
        brief: &argv_message,
        session_dir: session.as_ref().and_then(|s| s.dir.as_deref()),
        session_id: session.as_ref().and_then(|s| s.id.as_deref()),
        repo_root: repo.as_deref(),
        cwd: &cwd,
        model: None,
        effort: None,
    };
    let argv = if cold_resume {
        role.expand_resume(&vars, trusted)?
    } else {
        role.expand_start(&vars, trusted)?
    };
    let placed_model = if cold_resume {
        role::model_of(&argv)
    } else {
        role.effective_model().or_else(|| role::model_of(&argv))
    };
    let placed_effort = if cold_resume {
        role::effort_of(&argv)
    } else {
        role.effective_effort().or_else(|| role::effort_of(&argv))
    };

    let entry = |kind: Kind| (Some(ws.clone()), Some(id.clone()), kind);
    let superseded = |p: &Task| {
        let reason = format!("superseded by {}", id.as_str());
        let outcome = Some(TaskOutcome::Superseded);
        (
            Some(ws.clone()),
            Some(p.id.clone()),
            Kind::TaskClosed { reason, outcome },
        )
    };
    let space_resource = || {
        let mut r = ResourceRef::new(ResType::Space, space.clone());
        r.folder = Some(cwd.clone());
        r.label = space_label.clone();
        r
    };
    let mut items = vec![entry(Kind::TaskPlaced {
        rung: a.rung,
        reason: a.reason.clone(),
        rejected,
        role: role.name.clone(),
        harness: role.harness.clone(),
        model: placed_model,
        effort: placed_effort,
        title: title.clone(),
        brief: brief_path.clone(),
        session_of: previous.as_ref().map(|p| p.id.clone()),
        session: session.clone(),
        request: a.request.clone(),
        follows,
        role_hash,
        brief_bytes: input.len() as u64,
    })];
    let adopted = state.resources.iter().any(|r| {
        r.r.r#type == ResType::Space
            && r.r.name == space
            && r.ws.as_ref() == Some(&ws)
            && !r.released
    });
    if matches!(target, Target::Space(_)) && !adopted {
        items.push(entry(Kind::ResAdopted(space_resource())));
    }
    let mut intents = Vec::new();
    if let Target::Worktree { branch, base } = target {
        let repo = repo.as_deref().expect("placement required a repo");
        intents = worktree_intents(repo, branch, base, delivery_branch.as_deref(), &cwd)?;
    }
    if new_space {
        intents.push(space_resource());
    }
    if live_previous.is_none() {
        let mut tab = ResourceRef::new(ResType::Tab, id.as_str());
        tab.space = Some(space.clone());
        intents.push(tab);
    }
    items.extend(intents.iter().cloned().map(|r| entry(Kind::ResIntent(r))));
    if let (Some(term), Some(p)) = (&live_previous, &previous) {
        if !owns_terminal(state, term, &p.id) {
            return Err(refuse(
                "terminal_not_owned",
                "predecessor terminal is not owned",
            ));
        }
    }
    let _handover_guard = live_previous
        .as_ref()
        .map(|term| ctx.terminal_lock(&term.address))
        .transpose()?;
    let claimed_sessions = launch::claimed_session_ids(l.state());
    l.append(&c.actor(), ctx.now(), items)?;

    if let (Some(term), Some(p)) = (live_previous, &previous) {
        l.append(
            &c.actor(),
            ctx.now(),
            vec![
                superseded(p),
                entry(Kind::TaskStarted {
                    terminal: term.address.clone(),
                    tab: p.tab.clone(),
                    tab_name: title.clone(),
                    space: space.clone(),
                    cwd: cwd.clone(),
                    pid: term.pid,
                    created_ms: term.created_ms,
                    session: session.clone(),
                    resumed: true,
                }),
                entry(Kind::TaskSent {
                    message: brief_path,
                    via: SendVia::Send,
                    terminal: term.address.clone(),
                    started: None,
                }),
            ],
        )?;
        drop(l);
        let labels = [("coord.task", id.as_str()), ("coord.ws", ws.as_str())];
        let delivered = ctx
            .hc
            .label(&term.address, &labels)
            .and_then(|()| ctx.hc.send(&term.address, &message).map(drop));
        if let Err(e) = delivered {
            let attention = Kind::TaskAttention {
                state: AttentionState::Error,
                msg: Some(format!("handover failed: {e}")),
            };
            append(ctx, c, Some(ws.clone()), Some(id.clone()), attention)?;
            return Err(e);
        }
        if let Some(tab) = &p.tab {
            ctx.app
                .command("tab.rename", json!({"id":tab,"name":title}))?;
        }
        return Ok(json!({
            "task": id,
            "state": "running",
            "terminal": term.address,
            "space": space,
            "tab": p.tab,
            "session": session.and_then(|s| s.id),
        }));
    }
    drop(l);

    let created = match target {
        Target::Worktree { branch, .. } => {
            let repo = repo.as_deref().expect("placement required a repo");
            create_worktree(repo, branch, &cwd, &intents).and_then(|created| {
                let entries = created
                    .into_iter()
                    .map(|r| entry(Kind::ResCreated(r)))
                    .collect();
                j.lock()?.append(&c.actor(), ctx.now(), entries)?;
                Ok(())
            })
        }
        Target::Folder(_) => fs::create_dir_all(&cwd).map_err(Into::into),
        _ => Ok(()),
    };
    if let Err(e) = created {
        // A failed worktree operation can still have created a checkout. Leave
        // the intent for reconciliation unless git proves the path absent.
        let safely_absent = !matches!(target, Target::Worktree { .. })
            || repo
                .as_deref()
                .and_then(|r| git::worktree_list(r).ok())
                .is_some_and(|trees| !trees.iter().any(|t| t.path == cwd));
        if safely_absent {
            launch_failed(ctx, &j, c, &ws, &id, &e)?;
        }
        return Err(e);
    }
    let plan = launch::LaunchPlan {
        name: id.as_str().into(),
        role_label: "delegate",
        ws: Some(ws.clone()),
        task: Some(id.clone()),
        space: space.clone(),
        tab: None,
        cwd: cwd.clone(),
        argv,
        session,
        session_matcher: launch::task_session_matcher(&ctx.paths, &id, Some(&brief_path)),
        claimed_sessions,
        tab_name: title.clone(),
        space_name: new_space.then(|| space_label.clone().unwrap_or_default()),
    };
    let started = |term: &Term| {
        let mut entries = Vec::new();
        if let Some(p) = previous.as_ref().filter(|p| p.state != TaskState::Closed) {
            entries.push(superseded(p));
        }
        if new_space {
            entries.push(entry(Kind::ResCreated(space_resource())));
        }
        let mut tab = ResourceRef::new(ResType::Tab, term.address.clone());
        tab.space = Some(space.clone());
        entries.push(entry(Kind::ResCreated(tab)));
        entries.push(entry(Kind::TaskStarted {
            terminal: term.address.clone(),
            tab: None,
            tab_name: title.clone(),
            space: space.clone(),
            cwd: cwd.clone(),
            pid: term.pid,
            created_ms: term.created_ms,
            session: plan.session.clone(),
            resumed: previous.is_some(),
        }));
        j.lock()?.append(&c.actor(), ctx.now(), entries)?;
        Ok(())
    };
    let launched = match launch::launch_with_started(ctx, &plan, started) {
        Ok(launched) => launched,
        Err(e) => {
            if e.downcast_ref::<launch::PostLaunchError>().is_none() {
                launch_failed(ctx, &j, c, &ws, &id, &e)?;
            }
            return Err(e);
        }
    };
    let details = Kind::TaskLaunchDetails {
        terminal: launched.term.address.clone(),
        tab: launched.tab.clone(),
        session: launched.session.clone().filter(|s| s.id.is_some()),
    };
    j.lock()?
        .append(&c.actor(), ctx.now(), vec![entry(details)])?;
    Ok(json!({
        "task": id,
        "state": "running",
        "terminal": launched.term.address,
        "space": space,
        "tab": launched.tab,
        "session": launched.session.and_then(|s| s.id),
    }))
}

pub fn task_send(ctx: &Ctx, c: &Caller, task: &TaskId, text: &str) -> Result<Value> {
    let j = ctx.journal()?;
    let state = j.read_state()?;
    let t = state
        .tasks
        .get(task)
        .ok_or_else(|| refuse("unknown_task", "unknown task"))?;
    if t.state == TaskState::Closed {
        return Err(refuse("task_closed", "task is closed"));
    }
    let message = brief::follow_up(text, &ctx.cli(), &ctx.paths.result(task));
    let path = ctx.paths.brief(&format!(
        "{}-followup-{}",
        task.as_str(),
        state.last_seq + 1
    ));
    match live_terminal(ctx, t)? {
        Some(term) => send_to_terminal(ctx, c, &j, t, &term, &path, &message),
        None => resume_with_message(ctx, c, &j, &state, t, path, message),
    }
}

/// Deliver to the task's running agent: steer a working OMP turn, otherwise
/// start a new turn once the agent is ready for one.
fn send_to_terminal(
    ctx: &Ctx,
    c: &Caller,
    j: &Journal,
    t: &Task,
    term: &Term,
    path: &Path,
    message: &str,
) -> Result<Value> {
    let entry = |kind: Kind| (Some(t.ws.clone()), Some(t.id.clone()), kind);
    let _terminal_guard = ctx.terminal_lock(&term.address)?;
    let working_ready =
        term.live() && term.word() == Some("working") && term.turn >= 1 && term.reported;
    let app = term
        .status
        .as_ref()
        .and_then(|status| status.app.as_deref());
    let steered = working_ready && role::app_supports_steering(app);
    if working_ready && !steered {
        let harness = app.unwrap_or(&t.harness);
        return Err(error::not_ready(
            "steering_unsupported",
            format!(
                "mid-turn steering is not supported while {harness} is working; wait until the agent is settled"
            ),
        ));
    }
    if !steered {
        send_guard(term)?;
    }
    fs::write(path, message)?;
    let mut l = j.lock()?;
    let current = l
        .state()
        .tasks
        .get(&t.id)
        .ok_or_else(|| refuse("unknown_task", "task disappeared"))?;
    if current.last_activity_seq != t.last_activity_seq
        || current.terminal != t.terminal
        || current.state != t.state
    {
        return Err(error::not_ready(
            "agent_not_ready",
            "task changed before send",
        ));
    }
    if !owns_terminal(l.state(), term, &t.id) {
        return Err(refuse("terminal_not_owned", "task terminal is not owned"));
    }
    let sent = Kind::TaskSent {
        message: path.to_owned(),
        via: SendVia::Send,
        terminal: term.address.clone(),
        started: None,
    };
    l.append(&c.actor(), ctx.now(), vec![entry(sent)])?;
    drop(l);
    let outcome = if steered {
        ctx.hc.steer(&term.address, message)
    } else {
        ctx.hc.send(&term.address, message)
    };
    let sent = match outcome {
        Ok(sent) => sent,
        Err(e) => {
            let attention = Kind::TaskAttention {
                state: AttentionState::Error,
                msg: Some(format!("send failed: {e}")),
            };
            append(ctx, c, Some(t.ws.clone()), Some(t.id.clone()), attention)?;
            return Err(e);
        }
    };
    Ok(json!({
        "task": t.id,
        "sent": true,
        "delivery": if steered { "steering" } else { "new_turn" },
        "steered": steered,
        "turn": sent.turn,
        "started": sent.started,
    }))
}

/// The agent is gone: resume its recorded session in a new tab with the
/// message as the prompt.
fn resume_with_message(
    ctx: &Ctx,
    c: &Caller,
    j: &Journal,
    state: &State,
    t: &Task,
    path: PathBuf,
    message: String,
) -> Result<Value> {
    let entry = |kind: Kind| (Some(t.ws.clone()), Some(t.id.clone()), kind);
    let session = t
        .session
        .clone()
        .filter(|s| s.id.is_some())
        .ok_or_else(|| refuse("no_session", "task has no resumable session"))?;
    let role = role::load(&ctx.paths, &t.role)?;
    let cwd = t
        .cwd
        .clone()
        .ok_or_else(|| refuse("no_space", "task has no cwd"))?;
    let space = t
        .space
        .clone()
        .ok_or_else(|| refuse("no_space", "task has no space"))?;
    let repo = state.workstreams.get(&t.ws).and_then(|w| w.repo.as_deref());
    let snapshot = ctx.app.ls()?;
    let trusted = repo.is_some_and(|repo| launch_trusted(state, &snapshot, repo, &cwd, false));
    let arg = brief::for_argv(message.clone(), &path);
    let vars = role::Vars {
        brief: &arg,
        session_dir: session.dir.as_deref(),
        session_id: session.id.as_deref(),
        repo_root: repo,
        cwd: &cwd,
        model: None,
        effort: None,
    };
    let template = role
        .resume
        .as_ref()
        .ok_or_else(|| refuse("no_session", "role cannot resume"))?;
    if !template.iter().any(|arg| arg.contains("{brief}")) {
        return Err(refuse(
            "role_invalid",
            "role resume command must include {brief}",
        ));
    }
    let argv = role.expand_resume(&vars, trusted)?;
    fs::write(&path, &message)?;
    let mut intent = ResourceRef::new(ResType::Tab, t.id.as_str());
    intent.space = Some(space.clone());
    j.lock()?.append(
        &c.actor(),
        ctx.now(),
        vec![entry(Kind::ResIntent(intent.clone()))],
    )?;
    let plan = launch::LaunchPlan {
        name: t.id.as_str().into(),
        role_label: "delegate",
        ws: Some(t.ws.clone()),
        task: Some(t.id.clone()),
        space: space.clone(),
        tab: None,
        cwd: cwd.clone(),
        argv,
        session: Some(session),
        session_matcher: launch::task_session_matcher(&ctx.paths, &t.id, Some(&path)),
        claimed_sessions: launch::claimed_session_ids(state),
        tab_name: t.title.clone(),
        space_name: None,
    };
    let started = |term: &Term| {
        let mut tab = ResourceRef::new(ResType::Tab, term.address.clone());
        tab.space = Some(space.clone());
        let mut entries = Vec::new();
        if t.state != TaskState::Dormant && t.state != TaskState::Reported {
            entries.push(entry(Kind::TaskDormant {
                reason: "terminal_missing".into(),
            }));
        }
        entries.extend([
            entry(Kind::ResCreated(tab)),
            entry(Kind::TaskStarted {
                terminal: term.address.clone(),
                tab: None,
                tab_name: t.title.clone(),
                space: space.clone(),
                cwd: cwd.clone(),
                pid: term.pid,
                created_ms: term.created_ms,
                session: plan.session.clone(),
                resumed: true,
            }),
            entry(Kind::TaskSent {
                message: path.clone(),
                via: SendVia::Resume,
                terminal: term.address.clone(),
                started: None,
            }),
        ]);
        j.lock()?.append(&c.actor(), ctx.now(), entries)?;
        Ok(())
    };
    let launched = match launch::launch_with_started(ctx, &plan, started) {
        Ok(launched) => launched,
        Err(e) => {
            if e.downcast_ref::<launch::PostLaunchError>().is_none() {
                let never_created = Kind::ResReleased {
                    r#type: ResType::Tab,
                    name: intent.name,
                    note: Some("never created".into()),
                };
                j.lock()?
                    .append(&c.actor(), ctx.now(), vec![entry(never_created)])?;
            }
            return Err(e);
        }
    };
    let details = Kind::TaskLaunchDetails {
        terminal: launched.term.address.clone(),
        tab: launched.tab.clone(),
        session: launched.session.clone().filter(|s| s.id.is_some()),
    };
    j.lock()?
        .append(&c.actor(), ctx.now(), vec![entry(details)])?;
    Ok(json!({
        "task": t.id,
        "state": "running",
        "delivery": "new_turn",
        "steered": false,
        "terminal": launched.term.address,
        "space": space,
        "tab": launched.tab,
        "session": launched.session.and_then(|s| s.id),
    }))
}

pub fn task_close(
    ctx: &Ctx,
    c: &Caller,
    task: &TaskId,
    reason: &str,
    outcome: Option<TaskOutcome>,
) -> Result<Value> {
    if matches!(c, Caller::Coordinator) && outcome.is_none() {
        return Err(refuse(
            "outcome_required",
            "coordinator must specify an outcome",
        ));
    }
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let t = l
        .state()
        .tasks
        .get(task)
        .ok_or_else(|| refuse("unknown_task", "unknown task"))?;
    if t.state == TaskState::Closed {
        return Err(refuse("task_closed", "task already closed"));
    }
    let ws = t.ws.clone();
    let entries = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(ws),
            Some(task.clone()),
            Kind::TaskClosed {
                reason: reason.into(),
                outcome,
            },
        )],
    )?;
    drop(l);
    let checks = release::release_task_tab(ctx, &j, &c.actor(), task)?;
    Ok(json!({"seq":entries[0].seq,"checks":checks}))
}
fn delegate<'a>(state: &'a State, c: &Caller) -> Result<&'a super::state::Task> {
    let Caller::Delegate { task, ws, terminal } = c else {
        return Err(refuse("not_a_delegate", "only delegates can report"));
    };
    let t = state
        .tasks
        .get(task)
        .ok_or_else(|| refuse("unknown_task", "unknown task"))?;
    if &t.ws != ws || t.terminal.as_deref() != Some(terminal) || t.state == TaskState::Closed {
        return Err(refuse(
            "not_a_delegate",
            "delegate does not own an active task",
        ));
    }
    Ok(t)
}
pub fn report(
    ctx: &Ctx,
    c: &Caller,
    outcome: cli::ReportKind,
    summary: &str,
    result: Option<&Path>,
) -> Result<Value> {
    let result = result
        .map(|p| -> Result<PathBuf> {
            let p = fs::canonicalize(p)?;
            anyhow::ensure!(p.is_file(), "result must be a regular file");
            Ok(p)
        })
        .transpose()?;
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let t = delegate(l.state(), c)?;
    let ws = t.ws.clone();
    let id = t.id.clone();
    let kind = match outcome {
        cli::ReportKind::Progress => Kind::TaskProgress {
            summary: summary.into(),
        },
        _ => Kind::TaskReported {
            outcome: match outcome {
                cli::ReportKind::Done => Outcome::Done,
                cli::ReportKind::Blocked => Outcome::Blocked,
                cli::ReportKind::Question => Outcome::Question,
                cli::ReportKind::Progress => unreachable!(),
            },
            summary: summary.into(),
            result,
        },
    };
    let entries = l.append(&c.actor(), ctx.now(), vec![(Some(ws), Some(id), kind)])?;
    Ok(json!({"seq":entries[0].seq}))
}
pub fn note(ctx: &Ctx, c: &Caller, text: &str) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let t = delegate(l.state(), c)?;
    let ws = t.ws.clone();
    let id = t.id.clone();
    let entries = l.append(
        &c.actor(),
        ctx.now(),
        vec![(Some(ws), Some(id), Kind::TaskNote { text: text.into() })],
    )?;
    Ok(json!({"seq":entries[0].seq}))
}

fn scope(ws: &Option<String>, task: &Option<String>) -> Result<(Option<WsId>, Option<TaskId>)> {
    let task = task.as_deref().map(TaskId::parse).transpose()?;
    let ws = ws
        .as_deref()
        .map(WsId::parse)
        .transpose()?
        .or_else(|| task.as_ref().map(TaskId::ws));
    if let (Some(w), Some(t)) = (&ws, &task) {
        if *w != t.ws() {
            return Err(refuse(
                "invalid_scope",
                "task does not belong to workstream",
            ));
        }
    }
    Ok((ws, task))
}
pub fn decide(ctx: &Ctx, c: &Caller, a: &cli::Decide) -> Result<Value> {
    let (ws, task) = scope(&a.ws, &a.task)?;
    let tier = match a.tier {
        cli::DecisionTier::Silent => Tier::Silent,
        cli::DecisionTier::Digest => Tier::Digest,
        cli::DecisionTier::Ask => Tier::Ask,
    };
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let qid = (tier == Tier::Ask).then(|| format!("q{}", l.state().counters.question));
    let mut entries = vec![(
        ws.clone(),
        task.clone(),
        Kind::Decision {
            what: a.what.clone(),
            why: a.why.clone(),
            tier,
            qid: qid.clone(),
        },
    )];
    if let Some(qid) = &qid {
        entries.push((
            ws,
            task,
            Kind::QuestionAsked {
                qid: qid.clone(),
                text: a.what.clone(),
                options: (!a.options.is_empty()).then(|| a.options.clone()),
            },
        ));
    }
    let entries = l.append(&c.actor(), ctx.now(), entries)?;
    Ok(json!({"seq":entries[0].seq,"qid":qid}))
}
pub(super) fn answer_entry(
    state: &State,
    qid: &str,
    text: &str,
) -> Result<(Option<WsId>, Option<TaskId>, Kind)> {
    let q = state
        .questions
        .get(qid)
        .ok_or_else(|| refuse("unknown_question", "unknown question"))?;
    if q.answered.is_some() {
        return Err(refuse("question_answered", "question already answered"));
    }
    Ok((
        q.ws.clone(),
        q.task.clone(),
        Kind::QuestionAnswered {
            qid: qid.to_owned(),
            text: text.to_owned(),
        },
    ))
}

pub fn ask(ctx: &Ctx, c: &Caller, a: &cli::Ask) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    if let Some(qid) = &a.answer {
        let item = answer_entry(l.state(), qid, &a.text)?;
        let e = l.append(&c.actor(), ctx.now(), vec![item])?;
        return Ok(json!({"seq":e[0].seq,"qid":qid}));
    }
    let (ws, task) = scope(&a.ws, &a.task)?;
    let qid = format!("q{}", l.state().counters.question);
    let e = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            ws,
            task,
            Kind::QuestionAsked {
                qid: qid.clone(),
                text: a.text.clone(),
                options: (!a.options.is_empty()).then(|| a.options.clone()),
            },
        )],
    )?;
    Ok(json!({"seq":e[0].seq,"qid":qid}))
}
pub fn instruct(ctx: &Ctx, c: &Caller, a: &cli::Instruct) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let id = a
        .remove
        .clone()
        .or_else(|| a.replace.clone())
        .unwrap_or_else(|| format!("i{}", l.state().counters.instruction));
    if (a.remove.is_some() || a.replace.is_some()) && !l.state().instructions.contains_key(&id) {
        return Err(refuse("unknown_instruction", "unknown instruction"));
    }
    let e = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            None,
            None,
            Kind::InstructionSet {
                id,
                text: a.text.clone(),
            },
        )],
    )?;
    Ok(json!({"seq":e[0].seq}))
}
pub fn thread(ctx: &Ctx, c: &Caller, a: &cli::ThreadCmd) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let kind = match a {
        cli::ThreadCmd::Open { title, note } => Kind::ThreadOpened {
            id: format!("th{}", l.state().counters.thread),
            title: title.clone(),
            note: note.clone(),
        },
        cli::ThreadCmd::Close { id, note } => {
            let t = l
                .state()
                .threads
                .get(id)
                .ok_or_else(|| refuse("unknown_thread", "unknown thread"))?;
            Kind::ThreadClosed {
                id: id.clone(),
                title: t.title.clone(),
                note: note.clone(),
            }
        }
    };
    let e = l.append(&c.actor(), ctx.now(), vec![(None, None, kind)])?;
    Ok(json!({"seq":e[0].seq}))
}
pub fn correct(ctx: &Ctx, c: &Caller, a: &cli::Correct) -> Result<Value> {
    let what = match a.what {
        cli::CorrectionKind::Placement => CorrectionWhat::Placement,
        cli::CorrectionKind::Model => CorrectionWhat::Model,
        cli::CorrectionKind::Cleanup => CorrectionWhat::Cleanup,
        cli::CorrectionKind::Other => CorrectionWhat::Other,
    };
    let (ws, task) = if let Ok(task) = TaskId::parse(&a.reference) {
        (Some(task.ws()), Some(task))
    } else {
        (Some(WsId::parse(&a.reference)?), None)
    };
    append(
        ctx,
        c,
        ws,
        task,
        Kind::Correction {
            what,
            why: a.why.clone(),
            r#ref: a.reference.clone(),
        },
    )
}
pub fn scout(ctx: &Ctx, c: &Caller, a: &cli::Scout) -> Result<Value> {
    append(
        ctx,
        c,
        a.ws.as_deref().map(WsId::parse).transpose()?,
        None,
        Kind::ScoutResult {
            question: a.question.clone(),
            summary: a.summary.clone(),
        },
    )
}
pub fn inbox_ack(ctx: &Ctx, c: &Caller, seq: u64) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let state = l.state();
    if seq < state.cursor || seq > state.last_seq {
        return Err(refuse("invalid_cursor", "cursor outside journal"));
    }
    let count = state.waking.range(..=seq).count() as u64;
    l.append(
        &c.actor(),
        ctx.now(),
        vec![(None, None, Kind::InboxDrained { cursor: seq, count })],
    )?;
    Ok(json!({"cursor":seq,"count":count}))
}

#[cfg(test)]
mod tests {
    use super::super::adapter::{
        AgentStatus, Term,
        fake::{FakeApp, FakeHc},
    };
    use super::*;
    fn setup(root: &Path) -> Ctx {
        Ctx {
            root: root.into(),
            paths: super::super::paths::CoordPaths::new(root),
            hc: Box::new(FakeHc::default()),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 1000),
            op: "test".into(),
            cause: None,
        }
    }
    fn person() -> Caller {
        Caller::Person { terminal: None }
    }
    #[test]
    fn send_readiness_refusals() {
        let mut t: Term = serde_json::from_str(include_str!("testdata/hc-get-agent.json")).unwrap();
        t.state = "live".into();
        t.reported = true;
        t.turn = 0;
        t.status = Some(AgentStatus {
            state: "idle".into(),
            app: None,
            kind: None,
            msg: None,
        });
        assert_eq!(
            error::exit_of(&send_guard(&t).unwrap_err()),
            ("agent_not_ready", 4)
        );
        t.turn = 1;
        t.status.as_mut().unwrap().state = "blocked".into();
        assert_eq!(
            error::exit_of(&send_guard(&t).unwrap_err()),
            ("agent_blocked", 3)
        );
        t.status.as_mut().unwrap().state = "done".into();
        assert!(send_guard(&t).is_ok());
    }
    #[test]
    fn ladder_and_non_delegate_refuse_without_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = setup(tmp.path());
        let a = cli::TaskStart {
            ws: "ws-abcdef".into(),
            rung: 3,
            reason: "reason".into(),
            brief: "unused".into(),
            role: Some("coord-light".into()),
            rejected: vec!["1=no".into()],
            title: None,
            session_of: None,
            space: Some("sp".into()),
            branch: None,
            base: None,
            folder: None,
            request: None,
            follows: None,
            relation: None,
        };
        assert_eq!(
            error::exit_of(&task_start(&ctx, &person(), &a).unwrap_err()),
            ("ladder_reason_missing", 3)
        );
        assert_eq!(
            error::exit_of(
                &report(&ctx, &person(), cli::ReportKind::Done, "done", None).unwrap_err()
            ),
            ("not_a_delegate", 3)
        );
        assert_eq!(Journal::open(&ctx.paths).unwrap().last_seq().unwrap(), 0);
    }
    #[test]
    fn workstream_close_refuses_placed_task() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = setup(tmp.path());
        let c = person();
        let opened = ws_open(
            &ctx,
            &c,
            &cli::WsOpen {
                title: "x".into(),
                target: "x".into(),
                repo: None,
                delivery_branch: None,
                purpose: None,
                request: None,
            },
        )
        .unwrap();
        let ws = WsId::parse(opened["ws"].as_str().unwrap()).unwrap();
        let id = TaskId::new(&ws, 1);
        let j = Journal::open(&ctx.paths).unwrap();
        j.lock()
            .unwrap()
            .append(
                &c.actor(),
                ctx.now(),
                vec![(
                    Some(ws.clone()),
                    Some(id),
                    Kind::TaskPlaced {
                        rung: 5,
                        reason: "x".into(),
                        rejected: vec![],
                        role: "coord-light".into(),
                        harness: "omp".into(),
                        model: None,
                        effort: None,
                        title: "x".into(),
                        brief: "/tmp/x".into(),
                        session_of: None,
                        session: None,
                        request: None,
                        follows: None,
                        role_hash: String::new(),
                        brief_bytes: 0,
                    },
                )],
            )
            .unwrap();
        assert_eq!(
            error::exit_of(&ws_close(&ctx, &c, &ws, "done", WsOutcome::Delivered).unwrap_err()),
            ("tasks_running", 3)
        );
    }
}

#[cfg(test)]
mod placement_failure_tests {
    use super::super::adapter::{
        AppSnapshot,
        fake::{FakeApp, FakeHc},
    };
    use super::*;
    #[test]
    fn launch_failure_closes_task_and_resolves_intents() {
        let temp = tempfile::tempdir().unwrap();
        let app = FakeApp::default();
        app.snapshots
            .borrow_mut()
            .push_back(Ok(AppSnapshot { spaces: vec![] }));
        let hc = FakeHc::default();
        hc.runs
            .borrow_mut()
            .push_back(Err(anyhow::anyhow!("run failed")));
        let ctx = Ctx {
            root: temp.path().into(),
            paths: super::super::paths::CoordPaths::new(temp.path()),
            hc: Box::new(hc),
            app: Box::new(app),
            clock: Box::new(|| 1000),
            op: "test".into(),
            cause: None,
        };
        let c = Caller::Person { terminal: None };
        let opened = ws_open(
            &ctx,
            &c,
            &cli::WsOpen {
                title: "test".into(),
                target: "test".into(),
                repo: None,
                delivery_branch: None,
                purpose: None,
                request: None,
            },
        )
        .unwrap();
        let ws = opened["ws"].as_str().unwrap().to_owned();
        let input = temp.path().join("input");
        fs::write(&input, "brief").unwrap();
        let a = cli::TaskStart {
            ws: ws.clone(),
            rung: 5,
            reason: "new project".into(),
            brief: input,
            role: Some("coord-light".into()),
            rejected: (1..5).map(|n| format!("{n}=not suitable")).collect(),
            title: None,
            session_of: None,
            space: None,
            branch: None,
            base: None,
            folder: Some(temp.path().join("project")),
            request: None,
            follows: None,
            relation: None,
        };
        assert!(task_start(&ctx, &c, &a).is_err());
        let j = Journal::open(&ctx.paths).unwrap();
        let state = j.read_state().unwrap();
        assert!(state.intents.is_empty());
        assert_eq!(
            state.tasks.values().next().unwrap().state,
            TaskState::Closed
        );
        let kinds: Vec<_> = j
            .read_after(0)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                "ws.opened",
                "task.placed",
                "res.intent",
                "res.intent",
                "task.closed",
                "res.released",
                "res.released"
            ]
        );
    }
}

#[cfg(test)]
mod placement_acceptance_tests {
    use super::super::adapter::{
        App, AppSnapshot, AppSpace, AppTab, AppTerm, Hc, RunSpec, SendOutcome, Term,
    };
    use super::*;
    use std::cell::RefCell;
    use std::process::Command;
    use std::rc::Rc;

    #[derive(Default)]
    struct Recording {
        runs: Vec<RunSpec>,
        spaces: Vec<AppSpace>,
        live: bool,
        labels: Vec<Vec<(String, String)>>,
        sends: Vec<String>,
        journal_at_label: Vec<String>,
        report_on_rename: Option<super::super::paths::CoordPaths>,
    }
    struct RecordingHc {
        shared: Rc<RefCell<Recording>>,
        paths: super::super::paths::CoordPaths,
    }
    impl Hc for RecordingHc {
        fn run(&self, spec: &RunSpec) -> Result<Term> {
            let mut s = self.shared.borrow_mut();
            s.runs.push(spec.clone());
            let address = format!("terminal-{}", s.runs.len());
            let mut term: Term = serde_json::from_str(include_str!("testdata/hc-get-agent.json"))?;
            term.address = address.clone();
            term.state = "live".into();
            term.pid = Some(123);
            term.turn = 1;
            term.reported = true;
            term.status = Some(super::super::adapter::AgentStatus {
                state: "idle".into(),
                app: None,
                kind: None,
                msg: None,
            });
            term.labels = spec.labels.iter().cloned().collect();
            let id = spec
                .labels
                .iter()
                .find(|(k, _)| k == "herdr.space")
                .unwrap()
                .1
                .clone();
            if !s.spaces.iter().any(|space| space.id == id) {
                s.spaces.push(AppSpace {
                    id: id.clone(),
                    name: id.clone(),
                    folder: spec.cwd.to_string_lossy().into_owned(),
                    machine: None,
                    tabs: vec![],
                });
            }
            s.spaces
                .iter_mut()
                .find(|space| space.id == id)
                .unwrap()
                .tabs
                .push(AppTab {
                    id: format!("tab-{}", address),
                    name: "tab".into(),
                    terminals: vec![AppTerm {
                        terminal: address,
                        pane: "pane".into(),
                    }],
                });
            Ok(term)
        }
        fn get(&self, terminal: &str) -> Result<Option<Term>> {
            let s = self.shared.borrow();
            if !s.live {
                return Ok(None);
            }
            let Some(spec) = s.runs.first() else {
                return Ok(None);
            };
            let mut term: Term = serde_json::from_str(include_str!("testdata/hc-get-agent.json"))?;
            term.address = terminal.into();
            term.state = "live".into();
            term.pid = Some(123);
            term.turn = 1;
            term.reported = true;
            term.status = Some(super::super::adapter::AgentStatus {
                state: "idle".into(),
                app: None,
                kind: None,
                msg: None,
            });
            term.labels = spec.labels.iter().cloned().collect();
            Ok(Some(term))
        }
        fn list(&self, _: bool, _: Option<(&str, &str)>) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn send(&self, _: &str, text: &str) -> Result<SendOutcome> {
            self.shared.borrow_mut().sends.push(text.into());
            Ok(SendOutcome {
                turn: 2,
                started: Some(true),
                status: None,
            })
        }
        fn send_command(&self, _: &str, _: &str) -> Command {
            unreachable!()
        }
        fn label(&self, _: &str, labels: &[(&str, &str)]) -> Result<()> {
            let kinds = Journal::open(&self.paths)?
                .read_after(0)?
                .into_iter()
                .map(|e| e.kind)
                .collect();
            let mut s = self.shared.borrow_mut();
            s.journal_at_label = kinds;
            s.labels.push(
                labels
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect(),
            );
            Ok(())
        }
        fn kill(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn rm(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn events_command(&self) -> Command {
            Command::new("false")
        }
    }
    struct RecordingApp(Rc<RefCell<Recording>>);
    impl App for RecordingApp {
        fn ls(&self) -> Result<AppSnapshot> {
            Ok(AppSnapshot {
                spaces: self.0.borrow().spaces.clone(),
            })
        }
        fn command(&self, _: &str, _: Value) -> Result<Value> {
            let report = {
                let mut s = self.0.borrow_mut();
                s.report_on_rename
                    .take()
                    .map(|paths| (paths, s.runs.last().unwrap().clone()))
            };
            if let Some((paths, spec)) = report {
                let task = TaskId::parse(
                    spec.labels
                        .iter()
                        .find(|(k, _)| k == "coord.task")
                        .unwrap()
                        .1
                        .as_str(),
                )?;
                let ws = task.ws();
                let ctx = Ctx {
                    root: paths.dir.parent().unwrap().to_path_buf(),
                    paths,
                    hc: Box::new(super::super::adapter::fake::FakeHc::default()),
                    app: Box::new(super::super::adapter::fake::FakeApp::default()),
                    clock: Box::new(|| 1000),
                    op: "report".into(),
                    cause: None,
                };
                super::report(
                    &ctx,
                    &Caller::Delegate {
                        terminal: "terminal-1".into(),
                        task,
                        ws,
                    },
                    cli::ReportKind::Done,
                    "early",
                    None,
                )?;
            }
            Ok(json!({}))
        }
    }
    fn setup(root: &Path) -> (Ctx, Rc<RefCell<Recording>>) {
        let paths = super::super::paths::CoordPaths::new(root);
        paths.ensure().unwrap();
        fs::write(paths.roles.join("accept.md"), "---\nharness: omp\nstart: runner start {session_id} {cwd} {brief}\nresume: runner resume {session_id} {cwd} {brief}\nsession: preset\ntrust: --trusted\n---\nRole instructions\n").unwrap();
        let shared = Rc::new(RefCell::new(Recording::default()));
        let mut ctx = Ctx::new(root).unwrap();
        ctx.hc = Box::new(RecordingHc {
            shared: shared.clone(),
            paths,
        });
        ctx.app = Box::new(RecordingApp(shared.clone()));
        ctx.clock = Box::new(|| 1000);
        (ctx, shared)
    }
    fn person() -> Caller {
        Caller::Person { terminal: None }
    }
    fn open(ctx: &Ctx, repo: Option<&Path>) -> WsId {
        let result = ws_open(
            ctx,
            &person(),
            &cli::WsOpen {
                title: "Work".into(),
                target: "target".into(),
                repo: repo.map(Path::to_path_buf),
                delivery_branch: None,
                purpose: None,
                request: None,
            },
        )
        .unwrap();
        WsId::parse(result["ws"].as_str().unwrap()).unwrap()
    }
    fn input(root: &Path) -> PathBuf {
        let p = root.join("input");
        fs::write(&p, "Do the work").unwrap();
        p
    }
    fn args(ws: &WsId, rung: u8, input: &Path) -> cli::TaskStart {
        cli::TaskStart {
            ws: ws.as_str().into(),
            rung,
            reason: "chosen".into(),
            brief: input.into(),
            role: Some("accept".into()),
            rejected: (1..rung).map(|n| format!("{n}=unsuitable")).collect(),
            title: Some("Task title".into()),
            session_of: None,
            space: None,
            branch: None,
            base: None,
            folder: None,
            request: None,
            follows: None,
            relation: None,
        }
    }
    fn space(shared: &Rc<RefCell<Recording>>, id: &str, folder: &Path) {
        shared.borrow_mut().spaces.push(AppSpace {
            id: id.into(),
            name: format!("Name {id}"),
            folder: folder.to_string_lossy().into_owned(),
            machine: None,
            tabs: vec![],
        });
    }
    fn start_in_space(ctx: &Ctx, ws: &WsId, input: &Path, name: &str) -> Value {
        let mut a = args(ws, 3, input);
        a.space = Some(name.into());
        task_start(ctx, &person(), &a).unwrap()
    }
    fn assert_spec(
        ctx: &Ctx,
        spec: &RunSpec,
        ws: &WsId,
        id: &TaskId,
        space: &str,
        cwd: &Path,
        mode: &str,
        session: &str,
        trusted: bool,
        text: &str,
    ) {
        assert_eq!(spec.name, id.as_str());
        assert_eq!(spec.cwd, cwd);
        assert_eq!(
            spec.labels,
            vec![
                ("herdr.space".into(), space.into()),
                ("coord.managed".into(), "1".into()),
                ("coord.role".into(), "delegate".into()),
                ("coord.ws".into(), ws.as_str().into()),
                ("coord.task".into(), id.as_str().into())
            ]
        );
        let role = role::load(&ctx.paths, "accept").unwrap();
        let message = if mode == "start" {
            brief::delegate_first(&role, text, &ctx.cli(), &ctx.paths.result(id))
        } else {
            brief::follow_up(text, &ctx.cli(), &ctx.paths.result(id))
        };
        let mut expected = vec!["runner".to_owned()];
        if trusted {
            expected.push("--trusted".into());
        }
        expected.extend([
            mode.to_owned(),
            session.to_owned(),
            cwd.to_string_lossy().into_owned(),
            message,
        ]);
        assert_eq!(spec.argv, expected);
    }
    fn init_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ][..],
        ] {
            assert!(
                kit_core::git_env::git()
                    .env("HOME", root)
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        repo
    }
    #[test]
    fn report_during_post_launch_rename_remains_reported() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let ws = open(&ctx, None);
        space(&rec, "existing", temp.path());
        rec.borrow_mut().report_on_rename = Some(ctx.paths.clone());
        let result = start_in_space(&ctx, &ws, &input, "existing");
        let id = TaskId::parse(result["task"].as_str().unwrap()).unwrap();
        let state = ctx.journal().unwrap().read_state().unwrap();
        let task = &state.tasks[&id];
        assert_eq!(task.state, TaskState::Reported);
        assert_eq!(task.summary.as_deref(), Some("early"));
        assert_eq!(task.tab.as_deref(), Some("tab-terminal-1"));
    }
    #[test]
    fn run_specs_for_all_five_rungs() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let repo = init_repo(temp.path());
        let mut config = super::super::config::Config::default();
        config.worktree_root = temp.path().join("trees").to_string_lossy().into_owned();
        config.save(&ctx.paths).unwrap();
        let ws = open(&ctx, Some(&repo));
        space(&rec, "existing", &repo);
        let first = start_in_space(&ctx, &ws, &input, "existing");
        let id1 = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
        let session1 = first["session"].as_str().unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[0],
            &ws,
            &id1,
            "existing",
            &repo,
            "start",
            session1,
            true,
            "Do the work",
        );
        let mut a = args(&ws, 2, &input);
        let second = task_start(&ctx, &person(), &a).unwrap();
        let id2 = TaskId::parse(second["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[1],
            &ws,
            &id2,
            "existing",
            &repo,
            "start",
            second["session"].as_str().unwrap(),
            true,
            "Do the work",
        );
        a = args(&ws, 1, &input);
        a.session_of = Some(id1.as_str().into());
        let resumed = task_start(&ctx, &person(), &a).unwrap();
        let id3 = TaskId::parse(resumed["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[2],
            &ws,
            &id3,
            "existing",
            &repo,
            "resume",
            session1,
            true,
            "Do the work",
        );
        let ws4 = open(&ctx, Some(&repo));
        let mut a4 = args(&ws4, 4, &input);
        a4.branch = Some("feature".into());
        let fourth = task_start(&ctx, &person(), &a4).unwrap();
        let id4 = TaskId::parse(fourth["task"].as_str().unwrap()).unwrap();
        let tree = temp.path().join("trees/repo/feature");
        assert_spec(
            &ctx,
            &rec.borrow().runs[3],
            &ws4,
            &id4,
            fourth["space"].as_str().unwrap(),
            &tree,
            "start",
            fourth["session"].as_str().unwrap(),
            true,
            "Do the work",
        );
        let ws5 = open(&ctx, None);
        let folder = temp.path().join("new-folder");
        let mut a5 = args(&ws5, 5, &input);
        a5.folder = Some(folder.clone());
        let fifth = task_start(&ctx, &person(), &a5).unwrap();
        let id5 = TaskId::parse(fifth["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[4],
            &ws5,
            &id5,
            fifth["space"].as_str().unwrap(),
            &folder,
            "start",
            fifth["session"].as_str().unwrap(),
            false,
            "Do the work",
        );
    }
    #[test]
    fn dormant_send_resumes_same_task_and_session() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let ws = open(&ctx, None);
        space(&rec, "existing", temp.path());
        let first = start_in_space(&ctx, &ws, &input, "existing");
        let id = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
        Journal::open(&ctx.paths)
            .unwrap()
            .lock()
            .unwrap()
            .append(
                &person().actor(),
                ctx.now(),
                vec![(
                    Some(ws.clone()),
                    Some(id.clone()),
                    Kind::TaskDormant {
                        reason: "terminal exited".into(),
                    },
                )],
            )
            .unwrap();
        let result = task_send(&ctx, &person(), &id, "Next step").unwrap();
        assert_eq!(result["task"], id.as_str());
        assert_eq!(result["session"], first["session"]);
        assert_spec(
            &ctx,
            &rec.borrow().runs[1],
            &ws,
            &id,
            "existing",
            temp.path(),
            "resume",
            first["session"].as_str().unwrap(),
            false,
            "Next step",
        );
        let entries = Journal::open(&ctx.paths).unwrap().read_after(0).unwrap();
        assert_eq!(
            entries
                .iter()
                .rev()
                .take(5)
                .map(|e| e.kind.as_str())
                .collect::<Vec<_>>(),
            vec![
                "task.launch_details",
                "task.sent",
                "task.started",
                "res.created",
                "res.intent"
            ]
        );
        assert_eq!(
            entries
                .iter()
                .rev()
                .find(|e| e.kind == "task.sent")
                .unwrap()
                .data["via"],
            "resume"
        );
    }
    #[test]
    fn live_handover_is_journaled_before_relabel_and_released_tab_refused() {
        for released in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (ctx, rec) = setup(temp.path());
            let input = input(temp.path());
            let ws = open(&ctx, None);
            space(&rec, "existing", temp.path());
            let first = start_in_space(&ctx, &ws, &input, "existing");
            let id = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
            rec.borrow_mut().live = true;
            if released {
                Journal::open(&ctx.paths)
                    .unwrap()
                    .lock()
                    .unwrap()
                    .append(
                        &person().actor(),
                        ctx.now(),
                        vec![(
                            Some(ws.clone()),
                            Some(id.clone()),
                            Kind::ResReleased {
                                r#type: ResType::Tab,
                                name: "terminal-1".into(),
                                note: None,
                            },
                        )],
                    )
                    .unwrap();
            }
            let mut a = args(&ws, 1, &input);
            a.session_of = Some(id.as_str().into());
            let result = task_start(&ctx, &person(), &a);
            if released {
                assert_eq!(
                    error::exit_of(&result.unwrap_err()),
                    ("terminal_not_owned", 3)
                );
                assert!(rec.borrow().labels.is_empty());
                assert!(rec.borrow().sends.is_empty());
            } else {
                let next = result.unwrap();
                let s = rec.borrow();
                assert_eq!(s.runs.len(), 1);
                assert_eq!(s.sends.len(), 1);
                assert_eq!(
                    s.labels,
                    vec![vec![
                        ("coord.task".into(), next["task"].as_str().unwrap().into()),
                        ("coord.ws".into(), ws.as_str().into())
                    ]]
                );
                assert_eq!(
                    &s.journal_at_label[s.journal_at_label.len() - 4..],
                    ["task.placed", "task.closed", "task.started", "task.sent"]
                );
            }
        }
    }
    #[test]
    fn trust_requires_a_person_space_for_the_same_repo() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let repo = init_repo(temp.path());
        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(
            kit_core::git_env::git()
                .env("HOME", temp.path())
                .arg("-C")
                .arg(&other)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        space(&rec, "other", &other);
        let ws = open(&ctx, Some(&repo));
        let mut a = args(&ws, 5, &input);
        a.folder = Some(repo.clone());
        let first = task_start(&ctx, &person(), &a).unwrap();
        let id = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[0],
            &ws,
            &id,
            first["space"].as_str().unwrap(),
            a.folder.as_ref().unwrap(),
            "start",
            first["session"].as_str().unwrap(),
            false,
            "Do the work",
        );
        let a_owned = args(&ws, 2, &input);
        let owned = task_start(&ctx, &person(), &a_owned).unwrap();
        let owned_id = TaskId::parse(owned["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[1],
            &ws,
            &owned_id,
            first["space"].as_str().unwrap(),
            &repo,
            "start",
            owned["session"].as_str().unwrap(),
            false,
            "Do the work",
        );
        space(&rec, "person", &repo);
        let ws2 = open(&ctx, Some(&repo));
        let mut a2 = args(&ws2, 5, &input);
        a2.folder = Some(temp.path().join("second"));
        let second = task_start(&ctx, &person(), &a2).unwrap();
        let id2 = TaskId::parse(second["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[2],
            &ws2,
            &id2,
            second["space"].as_str().unwrap(),
            a2.folder.as_ref().unwrap(),
            "start",
            second["session"].as_str().unwrap(),
            false,
            "Do the work",
        );
        let ws3 = open(&ctx, Some(&repo));
        let third = start_in_space(&ctx, &ws3, &input, "person");
        let id3 = TaskId::parse(third["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[3],
            &ws3,
            &id3,
            "person",
            &repo,
            "start",
            third["session"].as_str().unwrap(),
            true,
            "Do the work",
        );
    }
    #[test]
    fn trust_is_confined_to_workstream_repo_across_rungs_and_cold_resume() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let mut config = super::super::config::Config::default();
        config.worktree_root = temp.path().join("trees").to_string_lossy().into_owned();
        config.save(&ctx.paths).unwrap();

        let repo_a = init_repo(temp.path());
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_b).unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ][..],
        ] {
            assert!(
                kit_core::git_env::git()
                    .env("HOME", temp.path())
                    .arg("-C")
                    .arg(&repo_b)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let wt_a = temp.path().join("manual-wt-a");
        git::worktree_add(&repo_a, &wt_a, "manual-branch", Some("HEAD")).unwrap();

        space(&rec, "person-a", &repo_a);
        space(&rec, "space-b", &repo_b);
        space(&rec, "space-wt-a", &wt_a);

        // Rung-3 placement of repo A's workstream into unrelated repo B must NOT trust,
        // nor may follow-up rung 2, cold task_send, or rung 1 cold resume.
        let ws_unrelated = open(&ctx, Some(&repo_a));
        let r3_b = start_in_space(&ctx, &ws_unrelated, &input, "space-b");
        let id_b1 = TaskId::parse(r3_b["task"].as_str().unwrap()).unwrap();
        let sess_b1 = r3_b["session"].as_str().unwrap().to_owned();
        assert_spec(
            &ctx,
            &rec.borrow().runs[0],
            &ws_unrelated,
            &id_b1,
            "space-b",
            &repo_b,
            "start",
            &sess_b1,
            false,
            "Do the work",
        );

        let r2_b = task_start(&ctx, &person(), &args(&ws_unrelated, 2, &input)).unwrap();
        let id_b2 = TaskId::parse(r2_b["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[1],
            &ws_unrelated,
            &id_b2,
            "space-b",
            &repo_b,
            "start",
            r2_b["session"].as_str().unwrap(),
            false,
            "Do the work",
        );

        task_send(&ctx, &person(), &id_b1, "Cold follow up").unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[2],
            &ws_unrelated,
            &id_b1,
            "space-b",
            &repo_b,
            "resume",
            &sess_b1,
            false,
            "Cold follow up",
        );

        let mut a1_b = args(&ws_unrelated, 1, &input);
        a1_b.session_of = Some(id_b1.as_str().into());
        let r1_b = task_start(&ctx, &person(), &a1_b).unwrap();
        let id_b3 = TaskId::parse(r1_b["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[3],
            &ws_unrelated,
            &id_b3,
            "space-b",
            &repo_b,
            "resume",
            &sess_b1,
            false,
            "Do the work",
        );

        // Rung-3 placement into an existing worktree of repo A DOES receive trust on start and cold send.
        let ws_same_wt = open(&ctx, Some(&repo_a));
        let r3_wt = start_in_space(&ctx, &ws_same_wt, &input, "space-wt-a");
        let id_wt = TaskId::parse(r3_wt["task"].as_str().unwrap()).unwrap();
        let sess_wt = r3_wt["session"].as_str().unwrap().to_owned();
        assert_spec(
            &ctx,
            &rec.borrow().runs[4],
            &ws_same_wt,
            &id_wt,
            "space-wt-a",
            &wt_a,
            "start",
            &sess_wt,
            true,
            "Do the work",
        );
        task_send(&ctx, &person(), &id_wt, "Continue in worktree").unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[5],
            &ws_same_wt,
            &id_wt,
            "space-wt-a",
            &wt_a,
            "resume",
            &sess_wt,
            true,
            "Continue in worktree",
        );

        // Rung-4 coordinator-created worktree of repo A DOES receive trust on start and cold send.
        let ws_coord_wt = open(&ctx, Some(&repo_a));
        let mut a4 = args(&ws_coord_wt, 4, &input);
        a4.branch = Some("coord-branch".into());
        let r4 = task_start(&ctx, &person(), &a4).unwrap();
        let id_r4 = TaskId::parse(r4["task"].as_str().unwrap()).unwrap();
        let sess_r4 = r4["session"].as_str().unwrap().to_owned();
        let coord_tree = temp.path().join("trees/repo/coord-branch");
        assert_spec(
            &ctx,
            &rec.borrow().runs[6],
            &ws_coord_wt,
            &id_r4,
            r4["space"].as_str().unwrap(),
            &coord_tree,
            "start",
            &sess_r4,
            true,
            "Do the work",
        );
        task_send(&ctx, &person(), &id_r4, "Continue in coord worktree").unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[7],
            &ws_coord_wt,
            &id_r4,
            r4["space"].as_str().unwrap(),
            &coord_tree,
            "resume",
            &sess_r4,
            true,
            "Continue in coord worktree",
        );
    }
    #[test]
    fn rung_three_adopts_space_for_each_workstream() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        space(&rec, "shared", temp.path());
        let ws1 = open(&ctx, None);
        let ws2 = open(&ctx, None);
        start_in_space(&ctx, &ws1, &input, "shared");
        start_in_space(&ctx, &ws2, &input, "shared");
        let state = Journal::open(&ctx.paths).unwrap().read_state().unwrap();
        for ws in [&ws1, &ws2] {
            assert!(state.resources.iter().any(|r| r.ws.as_ref() == Some(ws)
                && r.r.r#type == ResType::Space
                && r.r.name == "shared"
                && !r.owned
                && !r.released));
            assert_eq!(state.workstreams[ws].space.as_ref().unwrap().id, "shared");
        }
        assert!(state.anomalies.is_empty(), "{:?}", state.anomalies);
    }
}

pub fn rate(
    ctx: &Ctx,
    c: &Caller,
    target: &str,
    value: super::kinds::RatingValue,
    note: Option<&str>,
) -> Result<Value> {
    if !matches!(c, Caller::Person { .. }) {
        return Err(refuse("forbidden_caller", "only the person can rate work"));
    }
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let (ws, task) = if let Ok(id) = TaskId::parse(target) {
        if !lock.state().tasks.contains_key(&id) {
            return Err(refuse("unknown_task", "unknown task"));
        }
        (Some(id.ws()), Some(id))
    } else {
        let id = WsId::parse(target)?;
        if !lock.state().workstreams.contains_key(&id) {
            return Err(refuse("unknown_ws", "unknown workstream"));
        }
        (Some(id), None)
    };
    let entries = lock.append(
        &c.actor(),
        ctx.now(),
        vec![(
            ws,
            task,
            Kind::Rating {
                target: target.into(),
                value,
                note: note.map(str::to_owned),
            },
        )],
    )?;
    Ok(json!({"seq":entries[0].seq}))
}
pub fn config_set(ctx: &Ctx, c: &Caller, key: &str, value: &str) -> Result<Value> {
    if !matches!(c, Caller::Person { .. }) {
        return Err(refuse(
            "forbidden_caller",
            "only the person can change configuration",
        ));
    }
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let cfg = ctx.config()?;
    let mut object = serde_json::to_value(&cfg)?;
    let old = object
        .get(key)
        .cloned()
        .ok_or_else(|| refuse("unknown_config_key", "unknown configuration key"))?;
    let new = match old {
        Value::String(_) => Value::String(value.into()),
        Value::Bool(_) => Value::Bool(
            value
                .parse()
                .map_err(|_| refuse("invalid_config_value", "expected true or false"))?,
        ),
        Value::Number(_) => json!(
            value
                .parse::<u64>()
                .map_err(|_| refuse("invalid_config_value", "expected unsigned integer"))?
        ),
        _ => {
            return Err(refuse(
                "invalid_config_value",
                "unsupported configuration type",
            ));
        }
    };
    if old == new {
        return Ok(json!({"changed":false}));
    }
    object[key] = new.clone();
    let next: super::config::Config = serde_json::from_value(object)?;
    next.save(&ctx.paths)?;
    let entries = lock.append(
        &c.actor(),
        ctx.now(),
        vec![(
            None,
            None,
            Kind::ConfigChanged {
                key: key.into(),
                old,
                new,
            },
        )],
    )?;
    Ok(json!({"changed":true,"seq":entries[0].seq}))
}
pub fn stats(ctx: &Ctx, since: Option<&str>) -> Result<Value> {
    super::stats::stats(ctx, since)
}

#[cfg(test)]
mod insight_handler_tests {
    use super::*;
    #[test]
    fn config_only_records_changes_and_delegate_cannot_rate() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = Ctx::new(temp.path()).unwrap();
        ctx.cause = Some(9);
        let person = Caller::Person { terminal: None };
        assert_eq!(
            config_set(&ctx, &person, "stall_secs", "300").unwrap()["changed"],
            false
        );
        assert_eq!(
            config_set(&ctx, &person, "stall_secs", "301").unwrap()["changed"],
            true
        );
        let entries = ctx.journal().unwrap().read_after(0).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "config.changed");
        assert_eq!(entries[0].data["old"], 300);
        assert_eq!(entries[0].data["new"], 301);
        assert_eq!(entries[0].cause, Some(9));
        let ws = WsId::parse("ws-abcdef").unwrap();
        let task = TaskId::new(&ws, 1);
        let delegate = Caller::Delegate {
            terminal: "t".into(),
            task,
            ws,
        };
        assert_eq!(
            error::exit_of(
                &rate(
                    &ctx,
                    &delegate,
                    "ws-abcdef",
                    super::super::kinds::RatingValue::Good,
                    None
                )
                .unwrap_err()
            ),
            ("forbidden_caller", 3)
        );
    }
}
