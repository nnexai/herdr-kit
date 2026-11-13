//! Append-only coordinator journal and cached materialized state.
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde_json::Value;

use super::error::not_ready;
use super::fold;
use super::ids::{TaskId, WsId};
use super::kinds::{Actor, Entry, FORMAT, Kind};
use super::paths::{CoordPaths, no_links};
use super::state::{FOLD_VERSION, State};

const MAX_LINE: usize = 64 * 1024;
#[cfg(test)]
thread_local! { static JOURNAL_SYNCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub struct Journal {
    paths: CoordPaths,
    op: String,
    cause: Option<u64>,
}
pub struct Locked<'a> {
    journal: &'a Journal,
    file: File,
    state: State,
}
pub struct LogQuery {
    pub limit: usize,
    pub before_seq: Option<u64>,
    pub after_seq: Option<u64>,
    pub since_ms: Option<u64>,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
}
pub struct LogPage {
    pub entries: Vec<Value>,
    pub has_more: bool,
}

impl Journal {
    pub fn open(paths: &CoordPaths) -> Result<Self> {
        Self::with_context(paths, super::new_op()?, None)
    }
    pub fn with_context(paths: &CoordPaths, op: String, cause: Option<u64>) -> Result<Self> {
        paths.ensure()?;
        Ok(Self {
            paths: paths.clone(),
            op,
            cause,
        })
    }
    fn files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for item in fs::read_dir(&self.paths.journal)? {
            let item = item?;
            let name = item.file_name();
            let name = name.to_string_lossy();
            let b = name.as_bytes();
            if b.len() == 13
                && b[4] == b'-'
                && &b[7..] == b".jsonl"
                && b[..4].iter().chain(&b[5..7]).all(u8::is_ascii_digit)
            {
                no_links(&item.path())?;
                files.push(item.path());
            }
        }
        files.sort();
        Ok(files)
    }
    fn lines(&self) -> Result<Vec<(Entry, Value)>> {
        self.lines_from(None)
    }
    fn first_seq(path: &Path) -> Result<Option<u64>> {
        let mut line = Vec::new();
        BufReader::new(File::open(path)?).read_until(b'\n', &mut line)?;
        if !line.ends_with(b"\n") {
            return Ok(None);
        }
        let entry: Entry = serde_json::from_slice(&line)
            .with_context(|| format!("invalid journal entry in {}", path.display()))?;
        Ok(Some(entry.seq))
    }
    fn file_lines(path: &Path) -> Result<Vec<(Entry, Value)>> {
        let bytes = fs::read(path)?;
        let mut entries = Vec::new();
        for line in bytes
            .split_inclusive(|b| *b == b'\n')
            .filter(|l| l.ends_with(b"\n"))
        {
            let value: Value = serde_json::from_slice(line)
                .with_context(|| format!("invalid journal line in {}", path.display()))?;
            let entry = serde_json::from_value(value.clone())
                .with_context(|| format!("invalid journal entry in {}", path.display()))?;
            entries.push((entry, value));
        }
        Ok(entries)
    }
    fn lines_from(&self, minimum: Option<&Path>) -> Result<Vec<(Entry, Value)>> {
        let mut entries = Vec::new();
        for path in self
            .files()?
            .into_iter()
            .filter(|p| minimum.is_none_or(|m| p.as_path() >= m))
        {
            entries.extend(Self::file_lines(&path)?);
        }
        entries.sort_by_key(|(e, _): &(Entry, Value)| e.seq);
        Ok(entries)
    }
    // Senders and handovers must append their intent while holding this lock before acting.
    pub fn lock(&self) -> Result<Locked<'_>> {
        no_links(&self.paths.lock)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.paths.lock)?;
        let start = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() >= Duration::from_secs(20) {
                        return Err(not_ready("lock_busy", "journal lock is busy"));
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e.into()),
            }
        }
        // Repair every partial tail before assigning another sequence.
        for path in self.files()? {
            repair(&path)?;
        }
        let state = self.read_state()?;
        Ok(Locked {
            journal: self,
            file,
            state,
        })
    }
    pub fn read_state(&self) -> Result<State> {
        let cache = fs::read(&self.paths.state)
            .ok()
            .and_then(|b| serde_json::from_slice::<State>(&b).ok())
            .filter(|s| s.fold_version == FOLD_VERSION);
        let (mut state, entries) = if let Some(cache) = cache {
            let month = self.paths.month_file(cache.last_ts_ms);
            let entries = self.lines_from(Some(&month))?;
            if cache.last_seq == 0 || entries.iter().any(|(e, _)| e.seq == cache.last_seq) {
                (cache, entries)
            } else {
                (State::default(), self.lines()?)
            }
        } else {
            (State::default(), self.lines()?)
        };
        for (entry, _) in &entries {
            if entry.seq > state.last_seq {
                fold::apply(&mut state, entry);
            }
        }
        Ok(state)
    }
    pub fn read_after(&self, seq: u64) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for path in self.files()?.into_iter().rev() {
            let first = Self::first_seq(&path)?;
            let lines = Self::file_lines(&path)?;
            entries.extend(
                lines
                    .into_iter()
                    .map(|(entry, _)| entry)
                    .filter(|entry| entry.seq > seq),
            );
            if first.is_some_and(|first| first <= seq.saturating_add(1)) {
                break;
            }
        }
        entries.sort_by_key(|entry| entry.seq);
        Ok(entries)
    }
    pub fn page(&self, q: &LogQuery) -> Result<LogPage> {
        let mut values = Vec::new();
        for path in self.files()?.into_iter().rev() {
            let first = Self::first_seq(&path)?;
            if first.is_some_and(|first| q.before_seq.is_some_and(|before| first >= before)) {
                continue;
            }
            let lines = Self::file_lines(&path)?;
            for (e, v) in lines.into_iter().rev() {
                if q.before_seq.is_none_or(|v| e.seq < v)
                    && q.after_seq.is_none_or(|v| e.seq > v)
                    && q.since_ms.is_none_or(|v| e.ts_ms >= v)
                    && q.ws.as_ref().is_none_or(|v| e.ws.as_ref() == Some(v))
                    && q.task.as_ref().is_none_or(|v| e.task.as_ref() == Some(v))
                {
                    values.push(v);
                    if values.len() > q.limit {
                        break;
                    }
                }
            }
            if values.len() > q.limit
                || first.is_some_and(|first| {
                    q.after_seq
                        .is_some_and(|after| first <= after.saturating_add(1))
                })
            {
                break;
            }
        }
        let has_more = values.len() > q.limit;
        values.truncate(q.limit);
        Ok(LogPage {
            entries: values,
            has_more,
        })
    }
    pub fn last_seq(&self) -> Result<u64> {
        Ok(self.lines()?.last().map_or(0, |(e, _)| e.seq))
    }
}
fn repair(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
        file.set_len(end as u64)?;
        file.sync_all()?;
    }
    Ok(())
}
impl Locked<'_> {
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn append(
        &mut self,
        actor: &Actor,
        now_ms: u64,
        items: Vec<(Option<WsId>, Option<TaskId>, Kind)>,
    ) -> Result<Vec<Entry>> {
        let mut entries = Vec::with_capacity(items.len());
        let mut lines = Vec::with_capacity(items.len());
        for (ws, task, kind) in items {
            let encoded = serde_json::to_value(&kind)?;
            let data = encoded
                .get("data")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let entry = Entry {
                v: FORMAT,
                seq: self.state.last_seq + entries.len() as u64 + 1,
                ts_ms: now_ms,
                actor: actor.clone(),
                kind: kind.name().to_owned(),
                ws,
                task,
                op: Some(self.journal.op.clone()),
                cause: self.journal.cause,
                data,
            };
            let mut line = serde_json::to_vec(&entry)?;
            line.push(b'\n');
            ensure!(line.len() <= MAX_LINE, "journal entry exceeds 64 KiB");
            entries.push(entry);
            lines.push(line);
        }
        let requested = self.journal.paths.month_file(now_ms);
        let path = self
            .journal
            .files()?
            .last()
            .filter(|p| **p > requested)
            .cloned()
            .unwrap_or(requested);
        no_links(&path)?;
        if !lines.is_empty() {
            let mut file = OpenOptions::new().create(true).append(true).open(path)?;
            for (line, entry) in lines.iter().zip(&entries) {
                file.write_all(line)?;
                file.sync_all()?;
                fold::apply(&mut self.state, entry);
            }
            File::open(&self.journal.paths.journal)?.sync_all()?;
            #[cfg(test)]
            JOURNAL_SYNCS.with(|n| n.set(n.get() + 1));
            let mut temp = tempfile::NamedTempFile::new_in(&self.journal.paths.dir)?;
            serde_json::to_writer(&mut temp, &self.state)?;
            temp.flush()?;
            temp.as_file().sync_all()?;
            temp.persist(&self.journal.paths.state)?;
            File::open(&self.journal.paths.dir)?.sync_all()?;
        }
        Ok(entries)
    }
}
impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
pub fn transact<T>(
    j: &Journal,
    actor: &Actor,
    now_ms: u64,
    f: impl FnOnce(&State) -> Result<(Vec<(Option<WsId>, Option<TaskId>, Kind)>, T)>,
) -> Result<(Vec<Entry>, T)> {
    let mut lock = j.lock()?;
    let (items, result) = f(lock.state())?;
    Ok((lock.append(actor, now_ms, items)?, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn month_creation_syncs_journal_directory_before_cache() {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(dir.path());
        let j = Journal::open(&paths).unwrap();
        JOURNAL_SYNCS.with(|n| n.set(0));
        for now in [1_704_067_199_999, 1_704_067_200_000] {
            transact(&j, &Actor::Cli, now, |_| {
                Ok((vec![(None, None, event())], ()))
            })
            .unwrap();
            assert!(paths.state.exists());
        }
        JOURNAL_SYNCS.with(|n| assert_eq!(n.get(), 2));
        assert_eq!(j.files().unwrap().len(), 2);
    }
    use std::sync::Arc;
    fn event() -> Kind {
        Kind::ScoutResult {
            question: "q".into(),
            summary: "s".into(),
        }
    }
    #[test]
    fn concurrent_appends_are_contiguous() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Arc::new(CoordPaths::new(dir.path()));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let paths = paths.clone();
            threads.push(std::thread::spawn(move || {
                let j = Journal::open(&paths).unwrap();
                for _ in 0..50 {
                    transact(&j, &Actor::Cli, 1, |_| {
                        Ok((vec![(None, None, event())], ()))
                    })
                    .unwrap();
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        let j = Journal::open(&paths).unwrap();
        assert_eq!(
            j.read_after(0)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect::<Vec<_>>(),
            (1..=400).collect::<Vec<_>>()
        );
    }
    #[test]
    fn cache_rollover_and_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(dir.path());
        let j = Journal::open(&paths).unwrap();
        for now in [1_704_067_199_999, 1_704_067_200_000, 1_704_067_200_001] {
            transact(&j, &Actor::Cli, now, |_| {
                Ok((vec![(None, None, event())], ()))
            })
            .unwrap();
            let all = j.read_after(0).unwrap();
            assert_eq!(
                serde_json::to_value(j.read_state().unwrap()).unwrap(),
                serde_json::to_value(fold::fold(&all)).unwrap()
            );
        }
        assert_eq!(j.files().unwrap().len(), 2);
        let path = paths.month_file(1_704_067_200_000);
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"{torn")
            .unwrap();
        assert_eq!(j.last_seq().unwrap(), 3);
        transact(&j, &Actor::Cli, 1_704_067_200_002, |_| {
            Ok((vec![(None, None, event())], ()))
        })
        .unwrap();
        assert_eq!(j.last_seq().unwrap(), 4);
    }
}

#[cfg(test)]
mod insight_tests {
    use super::*;
    #[test]
    fn invocation_op_and_cause_are_stamped_on_every_entry() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(temp.path());
        let one = Journal::with_context(&paths, super::super::new_op().unwrap(), Some(42)).unwrap();
        let two = Journal::with_context(&paths, super::super::new_op().unwrap(), None).unwrap();
        let event = || Kind::ScoutResult {
            question: "q".into(),
            summary: "s".into(),
        };
        let first = one
            .lock()
            .unwrap()
            .append(
                &Actor::Coordinator,
                1,
                vec![(None, None, event()), (None, None, event())],
            )
            .unwrap();
        let second = two
            .lock()
            .unwrap()
            .append(&Actor::Cli, 2, vec![(None, None, event())])
            .unwrap();
        assert_eq!(first[0].op, first[1].op);
        assert_ne!(first[0].op, second[0].op);
        assert_eq!(first[0].cause, Some(42));
        assert_eq!(first[1].cause, Some(42));
        assert_eq!(second[0].cause, None);
        let saved = one.read_after(0).unwrap();
        assert_eq!(saved[0].op, first[0].op);
        assert_eq!(saved[0].cause, Some(42));
    }
}

#[cfg(test)]
mod bounded_read_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn recent_cursor_and_pages_do_not_open_older_months() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(temp.path());
        let j = Journal::open(&paths).unwrap();
        fs::write(paths.journal.join("2026-01.jsonl"), b"invalid old month\n").unwrap();
        let line = |seq| {
            format!(
                "{}\n",
                json!({"v":1,"seq":seq,"ts_ms":seq,"actor":"cli","kind":"scout.result","ws":null,"task":null,"data":{"question":"q","summary":"s"}})
            )
        };
        fs::write(
            paths.journal.join("2026-02.jsonl"),
            format!("{}{}", line(3), line(4)),
        )
        .unwrap();
        fs::write(
            paths.journal.join("2026-03.jsonl"),
            format!("{}{}{{torn", line(5), line(6)),
        )
        .unwrap();
        assert_eq!(
            j.read_after(4)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect::<Vec<_>>(),
            vec![5, 6]
        );
        let q = LogQuery {
            limit: 1,
            before_seq: Some(6),
            after_seq: None,
            since_ms: None,
            ws: None,
            task: None,
        };
        let p = j.page(&q).unwrap();
        assert_eq!(p.entries[0]["seq"], 5);
        assert!(p.has_more);
    }
}
