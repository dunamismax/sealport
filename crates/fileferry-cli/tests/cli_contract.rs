use assert_cmd::Command;
use predicates::prelude::*;

fn fileferry() -> Command {
    let mut command = Command::cargo_bin("ferry").expect("ferry binary");
    for variable in [
        "FILEFERRY_CONFIG",
        "FILEFERRY_PROFILE",
        "FILEFERRY_REPOSITORY",
        "FILEFERRY_PASSWORD",
        "FILEFERRY_PASSWORD_FILE",
        "FILEFERRY_LOG",
    ] {
        command.env_remove(variable);
    }
    command
}

#[test]
fn top_level_help_lists_stable_global_flags_and_commands() {
    let mut command = fileferry();

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
                .and(predicates::str::contains("init"))
                .and(predicates::str::contains("backup"))
                .and(predicates::str::contains("snapshots"))
                .and(predicates::str::contains("ls"))
                .and(predicates::str::contains("version")),
        )
        .stderr("");
}

#[test]
fn output_mode_flags_conflict() {
    let mut command = fileferry();

    command
        .args(["--json", "--jsonl", "version"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("cannot be used with"));
}

#[test]
fn unknown_argument_exits_with_usage_error() {
    let mut command = fileferry();

    command
        .args(["version", "--not-a-real-flag"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unexpected argument"));
}
