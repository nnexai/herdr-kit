use anyhow::Result;

use super::adapter::Hc;
use super::error;
use super::ids::{TaskId, WsId};
use super::kinds::Actor;

#[derive(Clone, Debug)]
pub enum Caller {
    Person {
        terminal: Option<String>,
    },
    Coordinator,
    Delegate {
        terminal: String,
        task: TaskId,
        ws: WsId,
    },
    Watcher,
}
impl Caller {
    pub fn actor(&self) -> Actor {
        match self {
            Self::Person { terminal: None } => Actor::Cli,
            Self::Person { terminal: Some(_) } => Actor::User,
            Self::Coordinator => Actor::Coordinator,
            Self::Delegate { task, .. } => Actor::Task(task.clone()),
            Self::Watcher => Actor::Watcher,
        }
    }
}
pub fn resolve(hc: &dyn Hc) -> Result<Caller> {
    resolve_terminal(
        hc,
        std::env::var("HC_TERMINAL_ID")
            .ok()
            .filter(|s| !s.is_empty()),
    )
}
fn resolve_terminal(hc: &dyn Hc, terminal: Option<String>) -> Result<Caller> {
    let Some(terminal) = terminal else {
        return Ok(Caller::Person { terminal: None });
    };
    let term = hc
        .get(&terminal)
        .map_err(|e| {
            error::not_ready(
                "hc_unavailable",
                format!("cannot resolve caller terminal {terminal}: {e}"),
            )
        })?
        .ok_or_else(|| {
            error::not_ready(
                "hc_unavailable",
                format!("caller terminal {terminal} not found"),
            )
        })?;
    if term.managed() {
        match term.label("coord.role") {
            Some("coordinator") => return Ok(Caller::Coordinator),
            Some("watcher") => return Ok(Caller::Watcher),
            Some("delegate") => {
                let task = term.label("coord.task").ok_or_else(|| {
                    error::not_ready("hc_unavailable", "delegate terminal has no task label")
                })?;
                let task = TaskId::parse(task).map_err(|e| {
                    error::not_ready(
                        "hc_unavailable",
                        format!("invalid delegate task label: {e}"),
                    )
                })?;
                let ws = task.ws();
                return Ok(Caller::Delegate { terminal, task, ws });
            }
            _ => {}
        }
    }
    Ok(Caller::Person {
        terminal: Some(terminal),
    })
}

#[cfg(test)]
mod tests {
    use super::super::adapter::{Term, fake::FakeHc};
    use super::*;

    fn term(role: Option<&str>, managed: bool, task: Option<&str>) -> Term {
        let mut term: Term = serde_json::from_str(include_str!(
            "testdata/hc-get-agent.json"
        ))
        .unwrap();
        if managed {
            term.labels.insert("coord.managed".into(), "1".into());
        }
        if let Some(role) = role {
            term.labels.insert("coord.role".into(), role.into());
        }
        if let Some(task) = task {
            term.labels.insert("coord.task".into(), task.into());
        }
        term
    }
    #[test]
    fn caller_roles_and_actor() {
        let hc = FakeHc::default();
        assert!(matches!(
            resolve_terminal(&hc, None).unwrap().actor(),
            Actor::Cli
        ));
        hc.gets
            .borrow_mut()
            .push_back(Ok(Some(term(Some("coordinator"), false, None))));
        assert!(matches!(
            resolve_terminal(&hc, Some("t1".into())).unwrap().actor(),
            Actor::User
        ));
        hc.gets
            .borrow_mut()
            .push_back(Ok(Some(term(Some("coordinator"), true, None))));
        assert!(matches!(
            resolve_terminal(&hc, Some("t1".into())).unwrap().actor(),
            Actor::Coordinator
        ));
        hc.gets
            .borrow_mut()
            .push_back(Ok(Some(term(Some("watcher"), true, None))));
        assert!(matches!(
            resolve_terminal(&hc, Some("t2".into())).unwrap().actor(),
            Actor::Watcher
        ));
        hc.gets.borrow_mut().push_back(Ok(Some(term(
            Some("delegate"),
            true,
            Some("ws-abcdef-t1"),
        ))));
        match resolve_terminal(&hc, Some("t3".into())).unwrap() {
            Caller::Delegate { task, ws, .. } => {
                assert_eq!(task.as_str(), "ws-abcdef-t1");
                assert_eq!(ws.as_str(), "ws-abcdef");
            }
            _ => panic!("expected delegate"),
        }
    }
    #[test]
    fn unresolved_and_malformed_callers_fail_closed() {
        let hc = FakeHc::default();
        hc.gets.borrow_mut().push_back(Ok(None));
        let err = resolve_terminal(&hc, Some("missing".into())).unwrap_err();
        assert_eq!(error::exit_of(&err), ("hc_unavailable", 4));
        hc.gets
            .borrow_mut()
            .push_back(Err(anyhow::anyhow!("offline")));
        assert_eq!(
            error::exit_of(&resolve_terminal(&hc, Some("t1".into())).unwrap_err()),
            ("hc_unavailable", 4)
        );
        hc.gets
            .borrow_mut()
            .push_back(Ok(Some(term(Some("delegate"), true, Some("bad")))));
        assert!(resolve_terminal(&hc, Some("t1".into())).is_err());
    }
}
