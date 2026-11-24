use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn test_cli_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_holodeck"))
        .arg("--help")
        .output()
        .expect("Failed to execute holodeck binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("zero-config, in-memory local mock cloud"));
    assert!(stdout.contains("start"));
    assert!(stdout.contains("mcp"));
    assert!(stdout.contains("trigger"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("env"));
}

#[test]
fn test_cli_mcp_help_and_stdio() {
    let help_output = Command::new(env!("CARGO_BIN_EXE_holodeck"))
        .args(["mcp", "--help"])
        .output()
        .expect("Failed to execute holodeck mcp --help");

    assert!(help_output.status.success());
    let help_stdout = String::from_utf8_lossy(&help_output.stdout);
    assert!(help_stdout.contains("--url"));
    assert!(help_stdout.contains("--port"));
    assert!(help_stdout.contains("--standalone"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_holodeck"))
        .args(["mcp", "--standalone"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn holodeck mcp");

    {
        let stdin = child.stdin.as_mut().expect("Failed to open stdin");
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#
        )
        .unwrap();
    }

    let output = child.wait_with_output().expect("Failed to wait on child");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("holodeck-mcp"));
    assert!(stdout.contains("holodeck_trigger_webhook"));
    assert!(stdout.contains("holodeck_inspect_events"));
    assert!(stdout.contains("holodeck_reset_state"));
}

#[test]
fn test_cli_env_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_holodeck"))
        .args(["env", "--provider", "all"])
        .output()
        .expect("Failed to execute holodeck env");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("STRIPE_SECRET_KEY"));
    assert!(stdout.contains("AUTH_CLIENT_ID"));
    assert!(stdout.contains("AWS_ACCESS_KEY_ID"));
    assert!(stdout.contains("RESEND_API_KEY"));
}

#[test]
fn test_cli_env_provider_specific() {
    let output = Command::new(env!("CARGO_BIN_EXE_holodeck"))
        .args(["env", "--provider", "stripe"])
        .output()
        .expect("Failed to execute holodeck env");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("STRIPE_SECRET_KEY"));
    assert!(!stdout.contains("AWS_ACCESS_KEY_ID"));
}
