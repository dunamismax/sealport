use assert_cmd::Command;

#[test]
fn version_subcommand_prints_human_version() {
    let mut command = Command::cargo_bin("sealport").expect("sealport binary");

    command
        .arg("version")
        .assert()
        .success()
        .stdout("sealport 0.0.0\n")
        .stderr("");
}

#[test]
fn version_subcommand_supports_json() {
    let mut command = Command::cargo_bin("sealport").expect("sealport binary");

    let output = command
        .args(["--json", "version"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(json["command"], "sealport");
    assert_eq!(json["version"], "0.0.0");
}

#[test]
fn global_output_flags_work_after_subcommands() {
    let mut command = Command::cargo_bin("sealport").expect("sealport binary");

    let output = command
        .args(["version", "--json"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).expect("valid JSON");
    assert_eq!(json["command"], "sealport");
    assert_eq!(json["version"], "0.0.0");
}
