use std::{fs, path::Path};

#[test]
fn repair_contract_is_satisfied() {
    let config = fs::read_to_string("config.toml").expect("fixture config should be readable");
    assert!(
        config.contains("port = 8080"),
        "server.port must be repaired to 8080"
    );

    assert_script_is_executable(Path::new("scripts/start.sh"));
}

#[cfg(unix)]
fn assert_script_is_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .expect("fixture script should exist")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "start script must be executable");
}

#[cfg(not(unix))]
fn assert_script_is_executable(path: &Path) {
    assert!(path.is_file(), "fixture script should exist");
}
