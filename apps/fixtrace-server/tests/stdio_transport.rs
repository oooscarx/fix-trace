use std::{
    io::Write,
    process::{Command, Stdio},
};

use fixtrace_protocol::{
    AppRequest, ClientCapabilities, ClientFrame, ClientInfo, EmptyRequest, InitializeRequest,
    PROTOCOL_VERSION, ServerFrame,
};
use uuid::Uuid;

fn wire_line(request: AppRequest) -> String {
    serde_json::to_string(&ClientFrame::Request(
        request
            .into_envelope(Uuid::new_v4(), Uuid::new_v4())
            .unwrap(),
    ))
    .unwrap()
}

#[test]
fn stdio_stdout_contains_only_jsonl_protocol_frames() {
    let temp = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fixtrace-server"))
        .args([
            "--state-dir",
            temp.path().to_str().unwrap(),
            "--listen",
            "stdio",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let initialize = AppRequest::Initialize(InitializeRequest {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        client: ClientInfo {
            name: "stdio-test".to_owned(),
            title: "Stdio Test".to_owned(),
            version: "1".to_owned(),
        },
        capabilities: ClientCapabilities {
            supports_streaming: true,
            supports_approvals: true,
            supports_diff: true,
            supports_graph: true,
            supports_artifacts: true,
        },
    });
    let config = AppRequest::ConfigGet(EmptyRequest {});
    let input = format!(
        "{{not-json\n{}\n{}\n{}\n",
        wire_line(config.clone()),
        wire_line(initialize),
        wire_line(config),
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let frames: Vec<ServerFrame> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("every stdout line must be a server frame"))
        .collect();
    assert_eq!(frames.len(), 4);
    let responses: Vec<_> = frames
        .iter()
        .map(|frame| match frame {
            ServerFrame::Response(response) => response,
            ServerFrame::Event(_) => panic!("no event was expected"),
        })
        .collect();
    assert_eq!(
        responses[0].error.as_ref().unwrap().code,
        fixtrace_protocol::ErrorCode::InvalidRequest
    );
    assert_eq!(
        responses[1].error.as_ref().unwrap().code,
        fixtrace_protocol::ErrorCode::NotInitialized
    );
    assert!(responses[2].result.is_some());
    assert!(responses[3].result.is_some());
}
