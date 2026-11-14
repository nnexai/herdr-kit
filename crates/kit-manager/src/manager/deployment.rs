//! Manager commands. Each one holds the cross-process manager lock, which
//! first completes any interrupted activation.
use super::*;

mod build;
mod generations;
mod transaction;

use transaction::apply_mod;

/// Seconds between scheduled source checks, and the retry delay after a failed one.
const CHECK_INTERVAL: u64 = 86_400;
const FAILURE_BACKOFF: u64 = 3_600;

struct Locked {
    data: PathBuf,
    _lock: File,
}
impl Locked {
    fn open(path: &Path) -> Result<Self> {
        let data = absolute(path)?;
        directory(&data)?;
        let path = data.join("manager.lock");
        no_links(&path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => bail!("manager lock busy"),
                Err(e) => return Err(e.into()),
            }
        }
        let this = Self { data, _lock: lock };
        this.recover()?;
        Ok(this)
    }
    fn state(&self) -> Result<State> {
        read_json(&state_path(&self.data)).map(|state| state.unwrap_or_default())
    }
    fn save(&self, state: &State) -> Result<()> {
        save(&state_path(&self.data), state)
    }
    /// The immutable release tree must hold exactly what the receipt recorded.
    fn verify_package(&self, rev: &str, receipt: &PackageReceipt) -> Result<()> {
        let root = release(&self.data, rev);
        if let Some(binary) = &receipt.definition.binary {
            ensure!(
                file_hash(&root.join(&binary.name))? == receipt.binary_hash,
                "release binary drift detected"
            );
        }
        for (base, hashes) in &receipt.mods {
            ensure!(
                &tree(&root.join("mods").join(base))? == hashes,
                "release mod drift detected"
            );
        }
        if receipt.definition.layout.is_some() {
            ensure!(
                file_hash(&root.join(LAYOUT_SOURCE))? == receipt.layout_hash,
                "release layout drift detected"
            );
        }
        Ok(())
    }
    /// `(component, revision, receipt)` for the selection, after checking
    /// dependency pins and release integrity.
    fn selected_receipts<'a>(
        &self,
        state: &'a State,
    ) -> Result<Vec<(&'a str, &'a str, &'a PackageReceipt)>> {
        state
            .selection
            .iter()
            .map(|(id, rev)| {
                let receipt = state.receipt(id, rev).context("missing package receipt")?;
                for required in &receipt.definition.requires {
                    ensure!(
                        state.selection.get(required) == Some(rev),
                        "dependent component pins must be updated together: {id} requires {required}"
                    );
                }
                self.verify_package(rev, receipt)?;
                Ok((id.as_str(), rev.as_str(), receipt))
            })
            .collect()
    }
    /// Every activated file and mod must still match its receipt.
    fn verify_active(&self, state: &State) -> Result<()> {
        for (target, receipt) in &state.files {
            ensure!(
                file_hash(target)?.as_deref() == Some(&receipt.hash),
                "managed file drift detected: {}",
                target.display()
            );
            ensure!(
                file_hash(&receipt.source)?.as_deref() == Some(&receipt.hash),
                "release file drift detected"
            );
        }
        for (_, rev, receipt) in self.selected_receipts(state)? {
            for (base, hashes) in &receipt.mods {
                let home = state.app_home.as_ref().context("missing target")?;
                ensure!(
                    &tree(&mod_path(home, base, rev))? == hashes,
                    "mod drift detected"
                );
            }
        }
        Ok(())
    }
}

/// The response body every command shares.
fn response(state: &State, available: Option<&str>, status: &str) -> Value {
    let resolved_mods: BTreeMap<&String, Vec<String>> = state
        .selection
        .iter()
        .filter_map(|(id, rev)| {
            let receipt = state.receipt(id, rev)?;
            Some((
                id,
                receipt.mods.keys().map(|base| name(base, rev)).collect(),
            ))
        })
        .collect();
    json!({
        "state": status,
        "current_revision": state.current,
        "available_revision": available,
        "source": state.source,
        "targets": {"app_home": state.app_home},
        "next_check": state.next_check,
        "components": state.selection,
        "resolved_mods": resolved_mods,
    })
}

/// Install and update responses also say whether a selection was activated
/// and whether that replaced the coordinator generation.
fn activated(mut value: Value, old: &State, next: &State) -> Value {
    let (before, after) = (
        old.selection.get("kit-coord"),
        next.selection.get("kit-coord"),
    );
    value["activated"] = json!(true);
    value["coord_changed"] = json!(after.is_some() && before != after);
    value
}
fn unchanged(mut value: Value) -> Value {
    value["activated"] = json!(false);
    value["coord_changed"] = json!(false);
    value
}

pub fn install(
    data: &Path,
    home: &Path,
    source: &Path,
    components: &[String],
    pinned: Option<&str>,
) -> Result<Value> {
    let lock = Locked::open(data)?;
    let old = lock.state()?;
    lock.verify_active(&old)?;
    let home = absolute(home)?;
    ensure!(home.is_dir(), "app home must exist");
    if old.current.is_some() {
        ensure!(
            old.app_home.as_ref() == Some(&home),
            "different install target already registered"
        );
    }
    let source = source_path(source)?;
    let rev = match pinned {
        Some(rev) => rev.to_owned(),
        None => revision(&source)?,
    };
    let mut next = old.clone();
    next.source = Some(source);
    next.app_home = Some(home);
    let ids = lock.build(&mut next, &rev, components)?;
    let selection = ids.into_iter().map(|id| (id, rev.clone())).collect();
    if old.selection == selection && old.current.is_some() {
        let next = lock.sweep_generations(next)?;
        return Ok(unchanged(response(&next, Some(&rev), "current")));
    }
    let transaction = lock.prepare(&old, next, selection)?;
    let next = lock.activate(transaction)?;
    Ok(activated(
        response(&next, Some(&rev), "installed"),
        &old,
        &next,
    ))
}

pub fn status(data: &Path) -> Result<Value> {
    let lock = Locked::open(data)?;
    let state = lock.state()?;
    lock.verify_active(&state)?;
    let status = if state.current.is_some() {
        "installed"
    } else {
        "not_installed"
    };
    Ok(response(&state, None, status))
}

pub fn source(data: &Path, repo: Option<&Path>) -> Result<Value> {
    let lock = Locked::open(data)?;
    let mut state = lock.state()?;
    if let Some(repo) = repo {
        state.source = Some(source_path(repo)?);
        state.next_check = 0;
        lock.save(&state)?;
    }
    Ok(response(&state, None, "source"))
}

#[derive(Clone, Copy, PartialEq, Eq, clap::Subcommand)]
pub enum UpdateAction {
    Check,
    Apply,
    Maintain,
}

pub fn update(data: &Path, action: UpdateAction, components: &[String]) -> Result<Value> {
    let lock = Locked::open(data)?;
    let mut state = lock.state()?;
    ensure!(state.current.is_some(), "suite is not installed");
    lock.verify_active(&state)?;
    if action != UpdateAction::Check {
        state = lock.sweep_generations(state)?;
    }
    if action == UpdateAction::Maintain && now() < state.next_check {
        return Ok(unchanged(response(&state, None, "not_due")));
    }
    for id in components {
        ensure!(
            state.selection.contains_key(id),
            "component is not selected: {id}"
        );
    }
    let work = update_from_source(&lock, &mut state, action, components);
    // A failed scheduled check backs off unless an activation is still pending recovery.
    if action == UpdateAction::Maintain && work.is_err() && !journal_path(&lock.data).exists() {
        state.next_check = now().saturating_add(FAILURE_BACKOFF);
        lock.save(&state)?;
    }
    work
}

fn update_from_source(
    lock: &Locked,
    state: &mut State,
    action: UpdateAction,
    components: &[String],
) -> Result<Value> {
    let source = state
        .source
        .as_ref()
        .context("no trusted local source configured")?;
    let rev = revision(source)?;
    let ids = if components.is_empty() {
        state.selection.keys().cloned().collect::<Vec<_>>()
    } else {
        components.to_vec()
    };
    let changed = ids.iter().any(|id| state.selection.get(id) != Some(&rev));
    if action == UpdateAction::Check {
        let status = if changed { "available" } else { "current" };
        return Ok(unchanged(response(state, Some(&rev), status)));
    }
    let old = state.clone();
    if changed {
        let built = lock.build(state, &rev, &ids)?;
        let mut selection = state.selection.clone();
        for id in built {
            selection.insert(id, rev.clone());
        }
        let transaction = lock.prepare(&old, state.clone(), selection)?;
        *state = lock.activate(transaction)?;
    }
    if action == UpdateAction::Maintain {
        state.next_check = now().saturating_add(CHECK_INTERVAL);
        lock.save(state)?;
    }
    let value = response(state, Some(&rev), "current");
    Ok(if changed {
        activated(value, &old, state)
    } else {
        unchanged(value)
    })
}

#[cfg(test)]
mod tests;
