//! The real host: Herdr terminals through `hc`, processes through Linux `/proc`, and the
//! plugin channel in `$XDG_RUNTIME_DIR/herdr-kit/omp`.
use super::*;
use crate::manager::bounded;
use anyhow::{Context, ensure};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_JSON: u64 = 4 << 20;

pub struct System {
    dir: PathBuf,
}

/// The plugin channel, accepted only as a private directory of this user.
fn channel() -> Result<PathBuf> {
    ensure!(
        cfg!(target_os = "linux"),
        "unsupported_os: herdr-kit omp reads process images from Linux /proc"
    );
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .context("XDG_RUNTIME_DIR must be set to an absolute path")?;
    Ok(runtime.join("herdr-kit/omp"))
}

fn private(dir: &Path) -> bool {
    let uid = rustix::process::getuid().as_raw();
    [dir.parent(), Some(dir)].into_iter().flatten().all(|d| {
        fs::symlink_metadata(d)
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.permissions().mode() & 0o077 == 0)
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_JSON).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Standard output of a bounded command that must succeed.
fn output(cmd: &mut Command, seconds: u64, what: &str) -> Result<Vec<u8>> {
    let read = |mut file: fs::File| -> Result<Vec<u8>> {
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.take(MAX_JSON).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let (out, err) = (tempfile::tempfile()?, tempfile::tempfile()?);
    let status = bounded(
        cmd.stdout(out.try_clone()?).stderr(err.try_clone()?),
        seconds,
    )
    .with_context(|| format!("{what} did not finish"))?;
    let stdout = read(out)?;
    if !status.success() {
        let stderr = read(err)?;
        let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_owned();
        bail!("{what} failed: {} {}", text(&stdout), text(&stderr));
    }
    Ok(stdout)
}

fn hc(args: &[&str]) -> Command {
    let mut cmd = Command::new("hc");
    cmd.args(args).env_remove("HC_TERMINAL_ID");
    cmd
}

/// The executable sessions should run after the upgrade: `--omp`, else `omp` on PATH.
/// It must start and print its version before anything is restarted into it.
pub fn target(explicit: Option<PathBuf>) -> Result<Target> {
    let path = match explicit {
        Some(path) => path,
        None => std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("omp"))
            .find(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0))
            .context("omp not found on PATH; pass --omp")?,
    };
    ensure!(path.is_absolute(), "--omp must be an absolute path");
    let meta = fs::metadata(&path).with_context(|| format!("cannot read {}", path.display()))?;
    ensure!(
        meta.is_file() && meta.mode() & 0o111 != 0,
        "{} is not executable",
        path.display()
    );
    let resolved = fs::canonicalize(&path)?;
    let version = output(
        Command::new(&path).arg("--version").stdin(Stdio::null()),
        30,
        "omp --version",
    )?;
    let version = String::from_utf8_lossy(&version).trim().to_owned();
    ensure!(!version.is_empty(), "omp --version printed nothing");
    let id = FileId {
        dev: meta.dev(),
        ino: meta.ino(),
    };
    ensure!(
        fs::metadata(&path).is_ok_and(|m| m.dev() == id.dev && m.ino() == id.ino),
        "{} changed during preflight; is an upgrade still running?",
        path.display()
    );
    Ok(Target {
        path,
        resolved,
        id,
        version,
    })
}

impl System {
    pub fn new() -> Result<Self> {
        Ok(Self { dir: channel()? })
    }
    fn file(&self, pid: i32, suffix: &str) -> PathBuf {
        self.dir.join(format!("{pid}{suffix}"))
    }
    /// Serializes restart runs; absent channel means no plugin ever ran, so nothing to lock.
    pub fn lock(&self) -> Result<Option<fs::File>> {
        if !self.dir.exists() {
            return Ok(None);
        }
        ensure!(
            private(&self.dir),
            "{} is not a private directory",
            self.dir.display()
        );
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(self.dir.join("restart.lock"))?;
        use fs2::FileExt;
        file.try_lock_exclusive().map_err(|_| {
            anyhow::anyhow!("already_running: another herdr-kit omp restart is in progress")
        })?;
        Ok(Some(file))
    }
}

impl Host for System {
    fn terminals(&self) -> Result<Vec<Terminal>> {
        #[derive(Deserialize)]
        struct List {
            terminals: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            address: String,
        }
        let list: List = serde_json::from_slice(&output(&mut hc(&["ls", "--json"]), 15, "hc ls")?)
            .context("invalid hc ls JSON")?;
        let mut terms = Vec::new();
        for entry in list.terminals {
            if let Some(term) = self.terminal(&entry.address)? {
                terms.push(term);
            }
        }
        Ok(terms)
    }
    fn terminal(&self, address: &str) -> Result<Option<Terminal>> {
        match output(&mut hc(&["get", "--json", address]), 15, "hc get") {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("invalid hc get JSON")?,
            )),
            Err(e) if format!("{e:#}").contains("not_found") => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn image(&self, pid: i32) -> Option<Image> {
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let link = fs::read_link(proc.join("exe")).ok()?;
        let meta = fs::metadata(proc.join("exe")).ok()?;
        let stat = fs::read_to_string(proc.join("stat")).ok()?;
        let start_time = stat
            .get(stat.rfind(')')? + 2..)?
            .split(' ')
            .nth(19)?
            .to_owned();
        let path = link.to_string_lossy().into_owned();
        Some(Image {
            deleted: path.ends_with(" (deleted)"),
            path,
            id: FileId {
                dev: meta.dev(),
                ino: meta.ino(),
            },
            start_time,
        })
    }
    fn presence(&self, pid: i32) -> Option<Presence> {
        private(&self.dir).then(|| read_json(&self.file(pid, ".json")))?
    }
    fn send(&self, request: &Request) -> Result<()> {
        ensure!(
            private(&self.dir),
            "{} is not a private directory",
            self.dir.display()
        );
        let _ = fs::remove_file(self.file(request.pid, ".reply.json"));
        let path = self.file(request.pid, ".request.json");
        let tmp = self.file(request.pid, &format!(".request.{}.tmp", request.nonce));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(request)?)?;
        file.sync_all()?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
    fn reply(&self, pid: i32) -> Option<Reply> {
        read_json(&self.file(pid, ".reply.json"))
    }
    fn withdraw(&self, pid: i32) {
        let _ = fs::remove_file(self.file(pid, ".request.json"));
    }
    fn file_id(&self, path: &Path) -> Option<FileId> {
        fs::metadata(path).ok().map(|m| FileId {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    fn nonce(&self) -> String {
        fs::read_to_string("/proc/sys/kernel/random/uuid")
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|_| format!("{}-{}", std::process::id(), self.now_ms()))
    }
}
