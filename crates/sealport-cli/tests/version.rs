use assert_cmd::Command;
use predicates::prelude::*;

fn sealport() -> Command {
    let mut command = Command::cargo_bin("sealport").expect("sealport binary");
    for variable in [
        "SEALPORT_CONFIG",
        "SEALPORT_PROFILE",
        "SEALPORT_REPOSITORY",
        "SEALPORT_LOG",
    ] {
        command.env_remove(variable);
    }
    command
}

#[test]
fn version_subcommand_prints_human_version() {
    let mut command = sealport();

    command
        .arg("version")
        .assert()
        .success()
        .stdout("sealport 0.0.0\n")
        .stderr("");
}

#[test]
fn version_subcommand_supports_json() {
    let mut command = sealport();

    let output = command
        .args(["--json", "version"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "version");
    assert_eq!(json["status"], "success");
    assert_eq!(json["data"]["command"], "sealport");
    assert_eq!(json["data"]["version"], "0.0.0");
}

#[test]
fn global_output_flags_work_after_subcommands() {
    let mut command = sealport();

    let output = command
        .args(["version", "--json"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(json["command"], "version");
    assert_eq!(json["data"]["command"], "sealport");
    assert_eq!(json["data"]["version"], "0.0.0");
}

#[test]
fn version_subcommand_supports_jsonl_events() {
    let mut command = sealport();

    let output = command
        .args(["--jsonl", "version"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = output.split(|byte| *byte == b'\n').collect();

    assert_eq!(lines.len(), 3);
    let started: serde_json::Value = serde_json::from_slice(lines[0]).expect("start event");
    let completed: serde_json::Value = serde_json::from_slice(lines[1]).expect("complete event");
    assert_eq!(started["event"], "command_started");
    assert_eq!(completed["event"], "command_completed");
    assert_eq!(completed["data"]["version"], "0.0.0");
}

#[test]
fn completion_subcommand_prints_shell_completion_data() {
    let mut command = sealport();

    command
        .args(["completion", "bash"])
        .assert()
        .success()
        .stdout(predicates::str::contains("_sealport").and(predicates::str::contains("version")))
        .stderr("");
}

#[test]
fn completion_subcommand_does_not_require_repository_config() {
    let mut command = sealport();

    command
        .env(
            "SEALPORT_REPOSITORY",
            "https://user:secret@example.com/repo",
        )
        .args(["completion", "zsh"])
        .assert()
        .success()
        .stdout(predicates::str::contains("#compdef sealport"))
        .stderr("");
}

#[test]
fn invalid_repository_exits_with_usage_error_and_redacts_secret_url_parts() {
    let mut command = sealport();

    command
        .args([
            "--repo",
            "https://user:secret@example.com/repo?token=sensitive",
            "version",
        ])
        .assert()
        .code(2)
        .stderr(
            predicates::str::contains("https://<redacted>@example.com/repo?<redacted>")
                .and(predicates::str::contains("secret").not())
                .and(predicates::str::contains("sensitive").not()),
        );
}
