use std::{collections::BTreeSet, fs, path::Path};

use crate::{
    domain::{
        action::FileReplacement,
        snapshot::{FileType, SnapshotManifest},
    },
    error::AppError,
};

pub fn replacements_between(
    root: &Path,
    before: &SnapshotManifest,
    after: &SnapshotManifest,
) -> Result<Vec<FileReplacement>, AppError> {
    let delta = before.diff(after);
    let changed: BTreeSet<_> = delta
        .created
        .iter()
        .chain(&delta.content_modified)
        .chain(&delta.permission_modified)
        .cloned()
        .collect();
    let mut replacements = Vec::new();

    for path in changed {
        let Some(state) = after.files.get(&path) else {
            continue;
        };
        match state.file_type {
            FileType::Regular => {
                let content = fs::read_to_string(root.join(&path)).map_err(|error| {
                    AppError::io("read checkpoint file as UTF-8", root.join(&path), error)
                })?;
                replacements.push(FileReplacement {
                    path,
                    content: Some(content),
                    unix_mode: state.unix_mode,
                });
            }
            FileType::Directory => {}
            FileType::Symlink => {
                return Err(AppError::NonReplayable {
                    action_id: 0,
                    reason: format!(
                        "checkpoint changed symbolic link `{}`; symlink patches are outside the MVP",
                        path.display()
                    ),
                });
            }
        }
    }
    for path in &delta.deleted {
        if before
            .files
            .get(path)
            .is_some_and(|state| state.file_type != FileType::Symlink)
        {
            replacements.push(FileReplacement {
                path: path.clone(),
                content: None,
                unix_mode: None,
            });
        }
    }
    Ok(replacements)
}
