use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn bundled_demo_replays_from_failure_to_success() {
    let state = tempdir().expect("temporary state directory should be created");
    let output = Command::new(env!("CARGO_BIN_EXE_fixtrace"))
        .args(["--state-dir"])
        .arg(state.path())
        .args(["demo", "--no-llm"])
        .output()
        .expect("fixtrace demo should start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "demo failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("demo should emit JSON");
    assert_eq!(report["baseline_outcome"], "stable_fail");
    assert_eq!(report["full_outcome"], "stable_pass");
    assert_eq!(report["final_outcome"], "stable_pass");
    assert_eq!(report["minimal_action_ids"], serde_json::json!([5, 6]));
    assert_eq!(report["ablations"].as_array().map(Vec::len), Some(2));
    assert!(
        report["ablations"]
            .as_array()
            .expect("ablations should be an array")
            .iter()
            .all(|ablation| ablation["outcome"] == "stable_fail")
    );
    assert!(
        !state.path().join("history.sqlite3").exists(),
        "standalone demo should not migrate or create persistent history"
    );
}
