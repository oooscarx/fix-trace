use std::{
    fs,
    path::{Path, PathBuf},
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("token file operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("token file {0} is empty or malformed")]
    Invalid(PathBuf),
    #[cfg(unix)]
    #[error("token file {0} is accessible by group or other users; require mode 0600")]
    InsecurePermissions(PathBuf),
    #[error("operating-system randomness failed: {0}")]
    Random(#[from] getrandom::Error),
}

pub fn load_or_create_token(path: &Path) -> Result<String, TokenError> {
    if path.exists() {
        validate_permissions(path)?;
        let token = fs::read_to_string(path)
            .map_err(|source| TokenError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .trim()
            .to_owned();
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(TokenError::Invalid(path.to_path_buf()));
        }
        return Ok(token);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| TokenError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)?;
    let token = hex::encode(bytes);
    write_secret(path, &token)?;
    Ok(token)
}

#[cfg(unix)]
fn write_secret(path: &Path, token: &str) -> Result<(), TokenError> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};

    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| TokenError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    writeln!(file, "{token}").map_err(|source| TokenError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn write_secret(path: &Path, token: &str) -> Result<(), TokenError> {
    fs::write(path, format!("{token}\n")).map_err(|source| TokenError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn validate_permissions(path: &Path) -> Result<(), TokenError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|source| TokenError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(TokenError::InsecurePermissions(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_permissions(_path: &Path) -> Result<(), TokenError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_random_persistent_and_not_world_readable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.token");
        let created = load_or_create_token(&path).unwrap();
        assert_eq!(created.len(), 64);
        assert!(created.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(load_or_create_token(&path).unwrap(), created);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
