use std::path::Path;

use ts_rs::{Config, ExportError, TS};

use crate::{AppRequest, AppResponsePayload, ClientFrame, ServerFrame};

pub fn export_typescript(output: &Path) -> Result<(), ExportError> {
    let config = Config::default()
        .with_out_dir(output)
        .with_large_int("number");
    ClientFrame::export_all(&config)?;
    ServerFrame::export_all(&config)?;
    AppRequest::export_all(&config)?;
    AppResponsePayload::export_all(&config)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use tempfile::tempdir;

    use super::export_typescript;

    #[test]
    fn checked_in_typescript_bindings_are_current() {
        let temp = tempdir().expect("temporary directory should be created");
        export_typescript(temp.path()).expect("TypeScript bindings should generate");

        let committed = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/fixtrace-desktop/src/generated/protocol");
        assert_eq!(
            read_tree(temp.path()),
            read_tree(&committed),
            "run `cargo run -p fixtrace-protocol --example export_types -- \
             apps/fixtrace-desktop/src/generated/protocol`"
        );
    }

    fn read_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    fn collect(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("binding directory should be readable") {
            let path = entry.expect("binding entry should be readable").path();
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("binding path should be below root")
                    .to_path_buf();
                files.insert(
                    relative,
                    fs::read(&path).expect("binding should be readable"),
                );
            }
        }
    }
}
