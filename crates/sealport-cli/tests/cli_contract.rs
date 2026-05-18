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
fn top_level_help_lists_stable_global_flags_and_commands() {
    let mut command = sealport();

    command
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicates::str::contains("Encrypted backups. Same everywhere.")
                .and(predicates::str::contains("--repo <REPO>"))
                .and(predicates::str::contains("--profile <PROFILE>"))
                .and(predicates::str::contains("--config <CONFIG>"))
                .and(predicates::str::contains("--json"))
                .and(predicates::str::contains("--jsonl"))
                .and(predicates::str::contains("completion"))
                .and(predicates::str::contains("version")),
        )
        .stderr("");
}

#[test]
fn output_mode_flags_conflict() {
    let mut command = sealport();

    command
        .args(["--json", "--jsonl", "version"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("cannot be used with"));
}

#[test]
fn unknown_argument_exits_with_usage_error() {
    let mut command = sealport();

    command
        .args(["version", "--not-a-real-flag"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unexpected argument"));
}
