//! One writer per data directory.
//!
//! Two recorders appending the same logs duplicate rows and clobber each
//! other's snapshots. The lock is keyed to the directory, not the HTTP port:
//! the embedded host never opens a port, so the port never guarded it.
//!
//! The pidfile is reclaimed when its recorded owner is gone, so a crash (which
//! cannot run the release) does not wedge the next start.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use thiserror::Error;

const LOCK_FILE: &str = "historian.lock";
const CLAIM_ATTEMPTS: usize = 5;

#[derive(Debug, Error)]
pub enum LockError {
    #[error("another collector already owns this data directory: {0}")]
    Busy(String),
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

fn claims() -> &'static Mutex<HashSet<PathBuf>> {
    static CLAIMS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    CLAIMS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Owns a data directory until dropped.
#[derive(Debug)]
pub struct DataDirLock {
    directory: PathBuf,
    pid: u32,
}

impl DataDirLock {
    /// Claim `directory`, creating it and its lock file if needed.
    pub fn acquire(directory: impl AsRef<Path>) -> Result<Self, LockError> {
        let directory = directory.as_ref();
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        let lock_path = directory.join(LOCK_FILE);
        let pid = std::process::id();

        {
            let mut claims = claims().lock().expect("lock registry");
            if !claims.insert(directory.clone()) {
                return Err(LockError::Busy(format!(
                    "this process already owns {} — refusing to start a second writer in the same process",
                    directory.display()
                )));
            }
        }

        for _ in 0..CLAIM_ATTEMPTS {
            match write_new_lock(&lock_path, pid) {
                Ok(()) => {
                    return Ok(Self { directory, pid });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let owner = fs::read_to_string(&lock_path)
                        .ok()
                        .and_then(|text| text.trim().parse::<u32>().ok());
                    if let Some(owner) = owner
                        && owner != pid
                        && process_alive(owner)
                    {
                        release_claim(&directory);
                        return Err(LockError::Busy(format!(
                            "another collector (pid {owner}) already owns {} — refusing to start a second writer",
                            directory.display()
                        )));
                    }
                    // The recorded owner is gone; drop its stale lock and race
                    // for it again.
                    match fs::remove_file(&lock_path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => {
                            release_claim(&directory);
                            return Err(LockError::Io(error));
                        }
                    }
                }
                Err(error) => {
                    release_claim(&directory);
                    return Err(LockError::Io(error));
                }
            }
        }

        release_claim(&directory);
        Err(LockError::Busy(format!(
            "could not claim {} — refusing to start",
            directory.display()
        )))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for DataDirLock {
    fn drop(&mut self) {
        let lock_path = self.directory.join(LOCK_FILE);
        let owned = fs::read_to_string(&lock_path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            == Some(self.pid);
        if owned {
            let _ = fs::remove_file(lock_path);
        }
        release_claim(&self.directory);
    }
}

fn write_new_lock(lock_path: &Path, pid: u32) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)?;
    write!(file, "{pid}")?;
    file.flush()
}

fn release_claim(directory: &Path) {
    claims().lock().expect("lock registry").remove(directory);
}

/// Signal 0 probes for the process without delivering anything: ESRCH means it
/// is gone, EPERM means it is alive but owned by another user.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: kill with signal 0 performs only the permission/existence check.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    // SPEC-RECORDER-007: one process owns a data directory.
    #[test]
    fn a_second_writer_is_refused_until_the_first_is_dropped() {
        let directory = tempdir().expect("temp dir");
        let first = DataDirLock::acquire(directory.path()).expect("first claim");
        assert!(matches!(
            DataDirLock::acquire(directory.path()),
            Err(LockError::Busy(_))
        ));
        drop(first);
        assert!(DataDirLock::acquire(directory.path()).is_ok());
    }

    #[test]
    fn a_stale_pidfile_is_reclaimed() {
        let directory = tempdir().expect("temp dir");
        fs::write(directory.path().join(LOCK_FILE), "4000000").expect("stale lock");
        let lock = DataDirLock::acquire(directory.path()).expect("reclaim");
        assert_eq!(lock.pid(), std::process::id());
    }

    #[test]
    fn a_live_owner_is_refused() {
        let directory = tempdir().expect("temp dir");
        // pid 1 is always present on the targets this ships to.
        fs::write(directory.path().join(LOCK_FILE), "1").expect("live lock");
        assert!(matches!(
            DataDirLock::acquire(directory.path()),
            Err(LockError::Busy(_))
        ));
        // The refusal must not delete a lock it does not own.
        assert_eq!(
            fs::read_to_string(directory.path().join(LOCK_FILE)).expect("lock text"),
            "1"
        );
    }

    #[test]
    fn dropping_only_removes_our_own_pidfile() {
        let directory = tempdir().expect("temp dir");
        let lock = DataDirLock::acquire(directory.path()).expect("claim");
        fs::write(directory.path().join(LOCK_FILE), "1").expect("someone else's lock");
        drop(lock);
        assert_eq!(
            fs::read_to_string(directory.path().join(LOCK_FILE)).expect("lock text"),
            "1"
        );
    }
}
