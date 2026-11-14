use super::*;
use std::os::unix::fs::MetadataExt;

pub(super) struct Cache {
    dir: File,
    pub lock: Option<File>,
    pub value: Option<PersistedQuota>,
}

fn regular(file: &File) -> Result<(), ()> {
    let meta = file.metadata().map_err(|_| ())?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != rustix::process::geteuid().as_raw() {
        return Err(());
    }
    fs::fchmod(file, Mode::from_raw_mode(0o600)).map_err(|_| ())?;
    Ok(())
}

impl Cache {
    pub fn open(root: &Path, now: u64) -> Result<Self, ()> {
        let parent = root.parent().ok_or(())?;
        std::fs::create_dir_all(parent).map_err(|_| ())?;
        let name = root.file_name().ok_or(())?;
        let base = File::open(parent).map_err(|_| ())?;
        match fs::mkdirat(&base, name, Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => (),
            Err(_) => return Err(()),
        }
        let dir = File::from(
            fs::openat(
                &base,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| ())?,
        );
        if dir.metadata().map_err(|_| ())?.uid() != rustix::process::geteuid().as_raw() {
            return Err(());
        }
        fs::fchmod(&dir, Mode::from_raw_mode(0o700)).map_err(|_| ())?;
        let lock = File::from(
            fs::openat(
                &dir,
                "collect.lock",
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|_| ())?,
        );
        regular(&lock)?;
        let lock = match lock.try_lock_exclusive() {
            Ok(()) => Some(lock),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(_) => return Err(()),
        };
        let value = match fs::openat(
            &dir,
            "snapshot.json",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                regular(&file)?;
                let mut bytes = Vec::new();
                file.take(MAX_CACHE + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| ())?;
                if bytes.len() as u64 > MAX_CACHE {
                    None
                } else {
                    serde_json::from_slice::<PersistedQuota>(&bytes)
                        .ok()
                        .filter(|v| valid_snapshot(v, now))
                }
            }
            Err(rustix::io::Errno::NOENT) => None,
            Err(_) => return Err(()),
        };
        Ok(Self { dir, lock, value })
    }

    pub fn save(&self, value: &PersistedQuota) -> Result<(), ()> {
        let bytes = serde_json::to_vec(value).map_err(|_| ())?;
        if bytes.len() as u64 > MAX_CACHE {
            return Err(());
        }
        // Unique temporary file is created relative to the pinned directory; never follow an existing name.
        let mut tmp = None;
        for n in 0..1000 {
            let name = format!(".snapshot-{}-{n}", std::process::id());
            match fs::openat(
                &self.dir,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            ) {
                Ok(fd) => {
                    tmp = Some((name, File::from(fd)));
                    break;
                }
                Err(rustix::io::Errno::EXIST) => continue,
                Err(_) => return Err(()),
            }
        }
        let (name, mut file) = tmp.ok_or(())?;
        let result = (|| {
            file.write_all(&bytes).map_err(|_| ())?;
            file.sync_all().map_err(|_| ())?;
            fs::renameat(&self.dir, name.as_str(), &self.dir, "snapshot.json").map_err(|_| ())?;
            self.dir.sync_all().map_err(|_| ())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::unlinkat(&self.dir, name.as_str(), fs::AtFlags::empty());
        }
        result
    }
}
