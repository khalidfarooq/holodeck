use std::process::Command;

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
