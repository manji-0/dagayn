//! The cross-process graph lock shared with the Python layer.
//!
//! Same file and protocol as `dagayn.write_lock`: a `flock` on
//! `<graph.db>.write.lock` (resolved path), exclusive for writers and shared
//! for readers, polled with `LOCK_NB`; a writer records its pid for
//! diagnostics. A Python reader or writer and this binary therefore
//! serialize against each other.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockMode {
    /// Writers: excludes every other holder.
    Exclusive,
    /// Readers: excludes writers only.
    Shared,
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("another process is using {db} (lock: {lock})")]
    Busy { db: PathBuf, lock: PathBuf },
    #[error("timed out after {timeout:?} waiting for {db} (lock: {lock})")]
    Timeout {
        db: PathBuf,
        lock: PathBuf,
        timeout: Duration,
    },
    #[error("cannot open the graph lock {lock}: {source}")]
    Open {
        lock: PathBuf,
        source: std::io::Error,
    },
}

/// Held until dropped.
pub struct GraphLock {
    file: File,
}

/// The lock a build or update holds while it writes.
pub type GraphWriteLock = GraphLock;

impl GraphLock {
    /// Take the exclusive lock for `db_path`, waiting up to `timeout`.
    pub fn acquire(db_path: &Path, timeout: Duration) -> Result<Self, LockError> {
        Self::acquire_mode(db_path, LockMode::Exclusive, Some(timeout))
    }

    /// Take the lock in `mode`. `wait` of `None` fails at once with
    /// [`LockError::Busy`] instead of waiting: a hook-triggered update must
    /// skip rather than queue behind another writer.
    pub fn acquire_mode(
        db_path: &Path,
        mode: LockMode,
        wait: Option<Duration>,
    ) -> Result<Self, LockError> {
        let key = db_path
            .canonicalize()
            .unwrap_or_else(|_| std::path::absolute(db_path).unwrap_or(db_path.to_path_buf()));
        let mut name = key.file_name().unwrap_or_default().to_os_string();
        name.push(".write.lock");
        let lock_path = key.with_file_name(name);
        let open_error = |source| LockError::Open {
            lock: lock_path.clone(),
            source,
        };
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(open_error)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&lock_path)
            .map_err(open_error)?;

        let operation = match mode {
            LockMode::Exclusive => libc::LOCK_EX,
            LockMode::Shared => libc::LOCK_SH,
        } | libc::LOCK_NB;
        let started = Instant::now();
        let mut interval = Duration::from_millis(50);
        // SAFETY: flock on a descriptor this function owns.
        while unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
            let Some(timeout) = wait else {
                return Err(LockError::Busy {
                    db: key,
                    lock: lock_path,
                });
            };
            if started.elapsed() >= timeout {
                return Err(LockError::Timeout {
                    db: key,
                    lock: lock_path,
                    timeout,
                });
            }
            std::thread::sleep(interval);
            interval = (interval * 2).min(MAX_POLL_INTERVAL);
        }
        if mode == LockMode::Exclusive {
            // Diagnostics only, as in Python: a failed pid write keeps the lock.
            let _ = file
                .set_len(0)
                .and_then(|()| file.seek(SeekFrom::Start(0)))
                .and_then(|_| writeln!(file, "{}", std::process::id()));
        }
        Ok(Self { file })
    }
}

impl Drop for GraphLock {
    fn drop(&mut self) {
        // SAFETY: unlocking the descriptor locked in `acquire_mode`.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
