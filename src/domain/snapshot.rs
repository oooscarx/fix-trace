use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::{DirEntry, WalkDir};

use crate::error::AppError;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileState {
    pub path: PathBuf,
    pub file_type: FileType,
    pub sha256: Option<String>,
    pub size: u64,
    pub unix_mode: Option<u32>,
    pub symlink_target: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotManifest {
    pub root_hash: String,
    pub files: BTreeMap<PathBuf, FileState>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotDelta {
    pub created: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub content_modified: Vec<PathBuf>,
    pub permission_modified: Vec<PathBuf>,
}

impl SnapshotManifest {
    pub fn capture(root: &Path, include_target: bool) -> Result<Self, AppError> {
        ensure_project_root(root)?;
        let mut files = BTreeMap::new();
        let walker = WalkDir::new(root)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| should_visit(entry, root, include_target));

        for entry in walker {
            let entry = entry?;
            if entry.path() == root {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| AppError::InvalidProject {
                    path: root.to_path_buf(),
                    reason: format!("walked path escaped root: {error}"),
                })?
                .to_path_buf();
            require_utf8_path(&relative)?;
            let state = capture_file_state(entry.path(), relative.clone())?;
            files.insert(relative, state);
        }

        let root_hash = hash_manifest(&files)?;
        Ok(Self { root_hash, files })
    }

    pub fn diff(&self, after: &Self) -> SnapshotDelta {
        let before_paths: BTreeSet<_> = self.files.keys().collect();
        let after_paths: BTreeSet<_> = after.files.keys().collect();
        let created = after_paths
            .difference(&before_paths)
            .map(|path| (*path).clone())
            .collect();
        let deleted = before_paths
            .difference(&after_paths)
            .map(|path| (*path).clone())
            .collect();
        let mut content_modified = Vec::new();
        let mut permission_modified = Vec::new();

        for path in before_paths.intersection(&after_paths) {
            let before = &self.files[*path];
            let current = &after.files[*path];
            if before.file_type != current.file_type
                || before.sha256 != current.sha256
                || before.size != current.size
                || before.symlink_target != current.symlink_target
            {
                content_modified.push((*path).clone());
            }
            if before.unix_mode != current.unix_mode {
                permission_modified.push((*path).clone());
            }
        }

        SnapshotDelta {
            created,
            deleted,
            content_modified,
            permission_modified,
        }
    }

    pub fn has_same_content(&self, other: &Self) -> bool {
        self.files.len() == other.files.len()
            && self.files.iter().all(|(path, state)| {
                other.files.get(path).is_some_and(|candidate| {
                    state.file_type == candidate.file_type
                        && state.sha256 == candidate.sha256
                        && state.size == candidate.size
                        && state.symlink_target == candidate.symlink_target
                })
            })
    }
}

pub(crate) fn is_excluded_relative(path: &Path, include_target: bool) -> bool {
    let Some(Component::Normal(first)) = path.components().next() else {
        return false;
    };
    first == ".git" || first == ".fixtrace" || (!include_target && first == "target")
}

fn ensure_project_root(root: &Path) -> Result<(), AppError> {
    let metadata =
        fs::metadata(root).map_err(|source| AppError::io("read project metadata", root, source))?;
    if !metadata.is_dir() {
        return Err(AppError::InvalidProject {
            path: root.to_path_buf(),
            reason: "path is not a directory".to_owned(),
        });
    }
    Ok(())
}

fn should_visit(entry: &DirEntry, root: &Path, include_target: bool) -> bool {
    entry
        .path()
        .strip_prefix(root)
        .map_or(true, |path| !is_excluded_relative(path, include_target))
}

fn capture_file_state(path: &Path, relative: PathBuf) -> Result<FileState, AppError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| AppError::io("read file metadata", path, source))?;
    let file_type = metadata.file_type();
    let (kind, sha256, size, symlink_target) = if file_type.is_file() {
        (
            FileType::Regular,
            Some(hash_file(path)?),
            metadata.len(),
            None,
        )
    } else if file_type.is_dir() {
        (FileType::Directory, None, 0, None)
    } else if file_type.is_symlink() {
        let target = fs::read_link(path)
            .map_err(|source| AppError::io("read symbolic link", path, source))?;
        require_utf8_path(&target)?;
        let encoded = target.to_string_lossy();
        (
            FileType::Symlink,
            Some(hex::encode(Sha256::digest(encoded.as_bytes()))),
            encoded.len() as u64,
            Some(target),
        )
    } else {
        return Err(AppError::InvalidProject {
            path: path.to_path_buf(),
            reason: "unsupported filesystem entry type".to_owned(),
        });
    };

    Ok(FileState {
        path: relative,
        file_type: kind,
        sha256,
        size,
        unix_mode: unix_mode(&metadata),
        symlink_target,
    })
}

fn hash_file(path: &Path) -> Result<String, AppError> {
    let mut file = File::open(path).map_err(|source| AppError::io("open file", path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| AppError::io("read file", path, source))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn hash_manifest(files: &BTreeMap<PathBuf, FileState>) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fixtrace-snapshot-v1\0");
    for (path, state) in files {
        update_len_prefixed(&mut hasher, path_text(path)?.as_bytes());
        hasher.update([match state.file_type {
            FileType::Regular => 1,
            FileType::Directory => 2,
            FileType::Symlink => 3,
        }]);
        update_optional_text(&mut hasher, state.sha256.as_deref());
        hasher.update(state.size.to_le_bytes());
        match state.unix_mode {
            Some(mode) => {
                hasher.update([1]);
                hasher.update(mode.to_le_bytes());
            }
            None => hasher.update([0]),
        }
        update_optional_text(
            &mut hasher,
            state.symlink_target.as_deref().map(path_text).transpose()?,
        );
    }
    Ok(hex::encode(hasher.finalize()))
}

fn update_len_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn update_optional_text(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(text) => {
            hasher.update([1]);
            update_len_prefixed(hasher, text.as_bytes());
        }
        None => hasher.update([0]),
    }
}

fn path_text(path: &Path) -> Result<&str, AppError> {
    path.to_str().ok_or_else(|| AppError::UnsafePath {
        path: path.to_path_buf(),
        reason: "non-UTF-8 paths are not supported by the JSON session format".to_owned(),
    })
}

fn require_utf8_path(path: &Path) -> Result<(), AppError> {
    path_text(path).map(|_| ())
}

#[cfg(unix)]
fn unix_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;

    Some(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn unix_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::SnapshotManifest;

    #[test]
    fn identical_directory_has_stable_hash() {
        let temp = tempdir().expect("temporary directory should be created");
        fs::create_dir(temp.path().join("src")).expect("source directory should be created");
        fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn answer() -> u8 { 42 }\n",
        )
        .expect("fixture file should be written");

        let first = SnapshotManifest::capture(temp.path(), false).expect("snapshot should work");
        let second = SnapshotManifest::capture(temp.path(), false).expect("snapshot should work");

        assert_eq!(first.root_hash, second.root_hash);
        assert_eq!(first, second);
    }

    #[test]
    fn diff_detects_create_delete_and_content_change() {
        let temp = tempdir().expect("temporary directory should be created");
        fs::write(temp.path().join("deleted.txt"), "old").expect("fixture should be written");
        fs::write(temp.path().join("changed.txt"), "before").expect("fixture should be written");
        let before = SnapshotManifest::capture(temp.path(), false).expect("snapshot should work");

        fs::remove_file(temp.path().join("deleted.txt")).expect("fixture should be deleted");
        fs::write(temp.path().join("changed.txt"), "after").expect("fixture should be updated");
        fs::write(temp.path().join("created.txt"), "new").expect("fixture should be created");
        let after = SnapshotManifest::capture(temp.path(), false).expect("snapshot should work");
        let delta = before.diff(&after);

        assert_eq!(delta.created, [PathBuf::from("created.txt")]);
        assert_eq!(delta.deleted, [PathBuf::from("deleted.txt")]);
        assert_eq!(delta.content_modified, [PathBuf::from("changed.txt")]);
    }

    #[cfg(unix)]
    #[test]
    fn diff_detects_permission_change() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().expect("temporary directory should be created");
        let script = temp.path().join("script.sh");
        fs::write(&script, "#!/bin/sh\n").expect("fixture should be written");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o644))
            .expect("permissions should be set");
        let before = SnapshotManifest::capture(temp.path(), false).expect("snapshot should work");

        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("permissions should be changed");
        let after = SnapshotManifest::capture(temp.path(), false).expect("snapshot should work");
        let delta = before.diff(&after);

        assert_eq!(delta.permission_modified, [PathBuf::from("script.sh")]);
        assert!(delta.content_modified.is_empty());
    }
}
