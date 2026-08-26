use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LockError {
    #[error("could not open App Server writer lock {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("another FixTrace App Server already owns {path}")]
    AlreadyLocked { path: PathBuf },
}

pub struct WriterLock {
    _file: std::fs::File,
    path: PathBuf,
}

impl WriterLock {
    pub fn acquire(path: impl Into<PathBuf>) -> Result<Self, LockError> {
        let path = path.into();
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| LockError::Open {
                path: path.clone(),
                source,
            })?;
        file.try_lock_exclusive()
            .map_err(|_| LockError::AlreadyLocked { path: path.clone() })?;
        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_writer_can_hold_a_state_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("writer.lock");
        let first = WriterLock::acquire(&path).unwrap();
        assert_eq!(first.path(), path);
        assert!(matches!(
            WriterLock::acquire(&path),
            Err(LockError::AlreadyLocked { .. })
        ));
        drop(first);
        WriterLock::acquire(path).expect("lock should be released when its owner is dropped");
    }
}
