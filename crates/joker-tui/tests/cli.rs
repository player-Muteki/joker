use std::process::Command;

fn joker_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_joker"))
}

#[test]
fn init_creates_config_and_refuses_to_overwrite() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_path = temp_dir.path().join("joker.toml");

    let output = joker_binary()
        .args(["init", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let created = std::fs::read_to_string(&config_path).unwrap();
    assert!(created.contains("provider = \"scripted\""));

    let output = joker_binary()
        .args(["init", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--force"),
        "stderr should explain that --force is required"
    );
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), created);
}

#[test]
fn exec_scripted_prints_only_response_to_stdout() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let config_path = temp_dir.path().join("joker.toml");

    let output = joker_binary()
        .args(["exec", "hello", "--provider", "scripted"])
        .args(["--scripted-response", "CLI response"])
        .args(["--config"])
        .arg(&config_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "exec failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "CLI response\n");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("INFO"),
        "tracing logs must not pollute stdout"
    );
}
