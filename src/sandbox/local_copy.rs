use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use walkdir::WalkDir;

use crate::{domain::snapshot::is_excluded_relative, error::AppError};

pub fn copy_project(
    source: &Path,
    destination: &Path,
    include_target: bool,
) -> Result<(), AppError> {
    let source_metadata = fs::metadata(source)
        .map_err(|error| AppError::io("read source project metadata", source, error))?;
    if !source_metadata.is_dir() {
        return Err(AppError::InvalidProject {
            path: source.to_path_buf(),
            reason: "source is not a directory".to_owned(),
        });
    }
    if destination.exists() {
        return Err(AppError::InvalidProject {
            path: destination.to_path_buf(),
            reason: "copy destination already exists".to_owned(),
        });
    }
    fs::create_dir_all(destination)
        .map_err(|error| AppError::io("create copy destination", destination, error))?;

    let canonical_source = source
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize source project", source, error))?;
    let mut directory_permissions = Vec::new();
    let walker = WalkDir::new(source)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.path().strip_prefix(source).map_or(true, |relative| {
                !is_excluded_relative(relative, include_target)
            })
        });

    for entry in walker {
        let entry = entry?;
        if entry.path() == source {
            continue;
        }
        let relative =
            entry
                .path()
                .strip_prefix(source)
                .map_err(|error| AppError::InvalidProject {
                    path: source.to_path_buf(),
                    reason: format!("walked path escaped source: {error}"),
                })?;
        let target = destination.join(relative);
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| AppError::io("read copy source metadata", entry.path(), error))?;

        if metadata.file_type().is_dir() {
            fs::create_dir(&target)
                .map_err(|error| AppError::io("create copied directory", &target, error))?;
            directory_permissions.push((target, metadata.permissions()));
        } else if metadata.file_type().is_file() {
            fs::copy(entry.path(), &target)
                .map_err(|error| AppError::io("copy file", &target, error))?;
            fs::set_permissions(&target, metadata.permissions()).map_err(|error| {
                AppError::io("preserve copied file permissions", &target, error)
            })?;
        } else if metadata.file_type().is_symlink() {
            copy_safe_symlink(entry.path(), &target, &canonical_source)?;
        } else {
            return Err(AppError::InvalidProject {
                path: entry.path().to_path_buf(),
                reason: "unsupported filesystem entry type".to_owned(),
            });
        }
    }

    for (path, permissions) in directory_permissions.into_iter().rev() {
        fs::set_permissions(&path, permissions)
            .map_err(|error| AppError::io("preserve copied directory permissions", &path, error))?;
    }
    fs::set_permissions(destination, source_metadata.permissions()).map_err(|error| {
        AppError::io(
            "preserve copied root directory permissions",
            destination,
            error,
        )
    })?;
    Ok(())
}

pub fn safe_existing_path(root: &Path, relative: &Path) -> Result<PathBuf, AppError> {
    validate_relative(relative)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize sandbox root", root, error))?;
    let candidate = root.join(relative);
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize sandbox path", &candidate, error))?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(AppError::UnsafePath {
            path: relative.to_path_buf(),
            reason: "resolved path escapes the sandbox root".to_owned(),
        });
    }
    Ok(canonical_candidate)
}

pub fn safe_write_path(root: &Path, relative: &Path) -> Result<PathBuf, AppError> {
    validate_relative(relative)?;
    let mut candidate = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            continue;
        };
        candidate.push(segment);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::UnsafePath {
                    path: relative.to_path_buf(),
                    reason: "writes through symbolic links are not allowed".to_owned(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AppError::io(
                    "inspect sandbox path component",
                    &candidate,
                    error,
                ));
            }
        }
    }
    Ok(root.join(relative))
}

fn validate_relative(relative: &Path) -> Result<(), AppError> {
    if relative.as_os_str().is_empty() {
        return Ok(());
    }
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(AppError::UnsafePath {
            path: relative.to_path_buf(),
            reason: "path must stay project-relative and cannot contain `..`".to_owned(),
        });
    }
    Ok(())
}

fn copy_safe_symlink(source: &Path, destination: &Path, root: &Path) -> Result<(), AppError> {
    let link_target =
        fs::read_link(source).map_err(|error| AppError::io("read symbolic link", source, error))?;
    let resolved = if link_target.is_absolute() {
        link_target.canonicalize()
    } else {
        source
            .parent()
            .unwrap_or(root)
            .join(&link_target)
            .canonicalize()
    }
    .map_err(|error| AppError::io("resolve symbolic link", source, error))?;
    if !resolved.starts_with(root) {
        return Err(AppError::UnsafePath {
            path: source.to_path_buf(),
            reason: "symbolic link target escapes the project".to_owned(),
        });
    }
    create_symlink(&link_target, destination, &resolved)
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path, _resolved: &Path) -> Result<(), AppError> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|error| AppError::io("create copied symbolic link", link, error))
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path, resolved: &Path) -> Result<(), AppError> {
    let result = if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    result.map_err(|error| AppError::io("create copied symbolic link", link, error))
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &Path, link: &Path, _resolved: &Path) -> Result<(), AppError> {
    Err(AppError::InvalidProject {
        path: link.to_path_buf(),
        reason: "symbolic link copying is unsupported on this platform".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::{safe_existing_path, safe_write_path};

    #[test]
    fn project_paths_cannot_escape_root() {
        let root = tempdir().expect("temporary root should be created");

        assert!(safe_write_path(root.path(), Path::new("../escape.txt")).is_err());
        assert!(safe_existing_path(root.path(), Path::new("../escape.txt")).is_err());
    }
}
