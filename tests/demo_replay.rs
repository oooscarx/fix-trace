use std::process::Command;

#[test]
fn bundled_demo_replays_from_failure_to_success() {
    let output = Command::new(env!("CARGO_BIN_EXE_fixtrace"))
        .args(["demo", "--no-llm"])
        .output()
        .expect("fixtrace demo should start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "demo failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("\"baseline_outcome\": \"stable_fail\""));
    assert!(stdout.contains("\"full_outcome\": \"stable_pass\""));
    assert!(stdout.contains("\"action_ids\": ["));
}
