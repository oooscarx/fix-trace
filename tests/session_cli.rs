#![cfg(unix)]

use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn init_history_export_and_import_work_through_cli() {
    let temp = tempdir().expect("temporary directory should be created");
    let project = temp.path().join("project");
    let source_state = temp.path().join("source-state");
    let destination_state = temp.path().join("destination-state");
    fs::create_dir(&project).expect("fixture project should be created");
    fs::write(project.join("fixture.txt"), "broken")
        .expect("fixture project file should be written");

    let initialized = run(&[
        "--state-dir",
        path(&source_state),
        "init",
        path(&project),
        "--oracle",
        "false",
    ]);
    assert!(
        initialized.status.success(),
        "init failed: {}",
        stderr(&initialized)
    );
    let stdout = String::from_utf8_lossy(&initialized.stdout);
    let session_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id="))
        .expect("init should print a session ID");

    let history = run(&["--state-dir", path(&source_state), "history", "list"]);
    assert!(
        history.status.success(),
        "history failed: {}",
        stderr(&history)
    );
    assert!(String::from_utf8_lossy(&history.stdout).contains(session_id));

    let export_file = temp.path().join("session.json");
    let exported = run(&[
        "--state-dir",
        path(&source_state),
        "export",
        session_id,
        "--output",
        path(&export_file),
    ]);
    assert!(
        exported.status.success(),
        "export failed: {}",
        stderr(&exported)
    );
    assert!(export_file.is_file());

    let imported = run(&[
        "--state-dir",
        path(&destination_state),
        "import",
        path(&export_file),
    ]);
    assert!(
        imported.status.success(),
        "import failed: {}",
        stderr(&imported)
    );
    let imported_history = run(&["--state-dir", path(&destination_state), "history", "list"]);
    assert!(String::from_utf8_lossy(&imported_history.stdout).contains(session_id));
}

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fixtrace"))
        .args(arguments)
        .output()
        .expect("fixtrace command should start")
}

fn path(path: &std::path::Path) -> &str {
    path.to_str().expect("test paths should be UTF-8")
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
