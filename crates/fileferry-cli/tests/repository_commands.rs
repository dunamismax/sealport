use assert_cmd::Command;
use serde_json::Value;
use std::fs;

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

fn init_repo(repo_url: &str, passphrase: &str) {
    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", repo_url, "init"])
        .assert()
        .success()
        .stderr("");
}

fn backup_source(repo_url: &str, passphrase: &str, source: &std::path::Path) -> Value {
    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            repo_url,
            "--json",
            "backup",
            "--tag",
            "cli",
            source.to_str().expect("source path"),
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&output).expect("backup json")
}

#[test]
fn init_creates_encrypted_local_repository_and_snapshots_lists_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";

    let init_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "init"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let init: Value = serde_json::from_slice(&init_output).expect("init json");
    assert_eq!(init["command"], "init");
    assert_eq!(init["status"], "success");
    assert_eq!(init["data"]["backend"], "local");
    assert_eq!(init["data"]["created"], true);
    assert_eq!(init["data"]["format_version"], 0);
    assert_eq!(init["data"]["key_slots"], 1);
    assert!(repo.join("bootstrap").is_file());

    let empty_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "snapshots"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let empty: Value = serde_json::from_slice(&empty_output).expect("snapshots json");
    assert_eq!(
        empty["data"]["snapshots"]
            .as_array()
            .expect("snapshot array")
            .len(),
        0
    );
}

#[test]
fn restore_writes_file_bytes_from_committed_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    fs::create_dir(source.join("nested")).expect("create nested");
    fs::write(source.join("nested").join("keep.txt"), b"keep").expect("write nested");
    let backup = backup_source(&repo_url, passphrase, &source);

    let destination = temp.path().join("restore-tag");
    let restore_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "restore",
            "--tag",
            "cli",
            "--path",
            "nested/keep.txt",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let restore: Value = serde_json::from_slice(&restore_output).expect("restore json");
    assert_eq!(restore["command"], "restore");
    assert_eq!(restore["status"], "success");
    assert_eq!(
        restore["data"]["snapshot_id"],
        backup["data"]["snapshot_id"]
    );
    assert_eq!(
        restore["data"]["paths"],
        serde_json::json!(["nested/keep.txt"])
    );
    assert_eq!(restore["data"]["dry_run"], false);
    assert_eq!(restore["data"]["overwrite"], "fail_if_exists");
    assert_eq!(restore["data"]["entries_selected"], 1);
    assert_eq!(restore["data"]["files_written"], 1);
    assert_eq!(restore["data"]["directories_written"], 0);
    assert_eq!(restore["data"]["symlinks_written"], 0);
    assert_eq!(restore["data"]["metadata_applied"], 0);
    assert_eq!(restore["data"]["metadata_warnings"], serde_json::json!([]));
    assert_eq!(restore["data"]["bytes_written"], 4);
    assert_eq!(restore["data"]["verified_files"], 1);
    assert_eq!(
        fs::read(destination.join("nested").join("keep.txt")).expect("restored nested file"),
        b"keep"
    );
    assert!(!destination.join("sample.txt").exists());

    let snapshot_id = backup["data"]["snapshot_id"].as_str().expect("snapshot id");
    let dry_run_destination = temp.path().join("restore-dry-run");
    let dry_run_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "restore",
            "--snapshot",
            snapshot_id,
            "--dry-run",
            dry_run_destination
                .to_str()
                .expect("dry run destination path"),
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let dry_run: Value = serde_json::from_slice(&dry_run_output).expect("dry-run json");
    assert_eq!(
        dry_run["data"]["snapshot_id"],
        backup["data"]["snapshot_id"]
    );
    assert_eq!(dry_run["data"]["dry_run"], true);
    assert_eq!(dry_run["data"]["verified_files"], 0);
    assert!(!dry_run_destination.exists());

    let latest_destination = temp.path().join("restore-latest");
    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "restore",
            "--latest",
            latest_destination
                .to_str()
                .expect("latest destination path"),
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("Restored snapshot"))
        .stderr("");
    assert_eq!(
        fs::read(latest_destination.join("sample.txt")).expect("latest restored file"),
        b"sample"
    );
}

#[test]
fn restore_jsonl_emits_progress_events_without_stderr() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let destination = temp.path().join("restore");
    let restore_jsonl_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--jsonl",
            "restore",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = restore_jsonl_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 8);
    let started: Value = serde_json::from_slice(lines[0]).expect("started event");
    assert_eq!(started["event"], "command_started");
    assert_eq!(started["command"], "restore");
    let progress: Vec<Value> = lines[1..7]
        .iter()
        .map(|line| serde_json::from_slice(line).expect("progress event"))
        .collect();
    assert_eq!(progress[0]["event"], "progress");
    assert_eq!(progress[0]["data"]["phase"], "load_manifest");
    assert_eq!(progress[5]["data"]["phase"], "complete");
    let completed: Value = serde_json::from_slice(lines[7]).expect("completed event");
    assert_eq!(completed["event"], "command_completed");
    assert_eq!(completed["data"]["files_written"], 1);
}

#[test]
fn restore_requires_correct_password_and_safe_destination() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let destination = temp.path().join("restore");
    fileferry()
        .env("FILEFERRY_PASSWORD", "wrong-passphrase")
        .args([
            "--repo",
            &repo_url,
            "restore",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .code(4)
        .stdout("")
        .stderr(predicates::str::contains(
            "repository could not be unlocked",
        ));

    fs::create_dir(&destination).expect("create destination");
    fs::write(destination.join("sample.txt"), b"existing").expect("write existing file");
    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "restore",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicates::str::contains("already exists"));
    assert_eq!(
        fs::read(destination.join("sample.txt")).expect("existing file remains"),
        b"existing"
    );

    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "restore",
            "--overwrite",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .success()
        .stderr("");
    assert_eq!(
        fs::read(destination.join("sample.txt")).expect("overwritten file"),
        b"sample"
    );
}

#[test]
fn backup_writes_committed_snapshot_that_snapshots_and_ls_can_discover() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";

    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "init"])
        .assert()
        .success()
        .stderr("");

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");

    let backup_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "backup",
            "--tag",
            "cli",
            source.to_str().expect("source path"),
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let backup: Value = serde_json::from_slice(&backup_output).expect("backup json");
    assert_eq!(backup["command"], "backup");
    assert_eq!(backup["status"], "success");
    assert_eq!(backup["data"]["tags"], serde_json::json!(["cli"]));
    assert_eq!(backup["data"]["entries_scanned"], 2);
    assert_eq!(backup["data"]["files_backed_up"], 1);
    assert_eq!(backup["data"]["directories_backed_up"], 1);
    assert_eq!(backup["data"]["bytes_scanned"], 6);
    assert_eq!(backup["data"]["chunks_seen"], 1);
    assert_eq!(backup["data"]["chunks_written"], 1);
    assert_eq!(backup["data"]["chunks_reused"], 0);
    assert_eq!(backup["data"]["manifest_id"], backup["data"]["snapshot_id"]);
    assert_eq!(
        backup["data"]["index_ids"]
            .as_array()
            .expect("index id array")
            .len(),
        1
    );

    let snapshots_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "snapshots"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let snapshots: Value = serde_json::from_slice(&snapshots_output).expect("snapshots json");
    let snapshot = &snapshots["data"]["snapshots"][0];
    assert_eq!(snapshot["snapshot_id"], backup["data"]["snapshot_id"]);
    assert_eq!(snapshot["tags"], serde_json::json!(["cli"]));
    assert_eq!(snapshot["entry_count"], 2);

    let ls_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "ls"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let ls: Value = serde_json::from_slice(&ls_output).expect("ls json");
    assert_eq!(ls["command"], "ls");
    assert_eq!(ls["data"]["snapshot_id"], snapshot["snapshot_id"]);
    assert_eq!(ls["data"]["path"], ".");
    assert_eq!(ls["data"]["entries"][0]["path"], "sample.txt");
    assert_eq!(ls["data"]["entries"][0]["kind"], "regular_file");
    assert_eq!(ls["data"]["entries"][0]["size_bytes"], 6);
    assert_eq!(ls["data"]["entries"][0]["modified"]["status"], "captured");

    let snapshots_jsonl_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--jsonl", "snapshots"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let jsonl_lines: Vec<_> = snapshots_jsonl_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(jsonl_lines.len(), 2);
    let completed: Value = serde_json::from_slice(jsonl_lines[1]).expect("completed event");
    assert_eq!(completed["event"], "command_completed");
    assert_eq!(
        completed["data"]["snapshots"][0]["snapshot_id"],
        snapshot["snapshot_id"]
    );

    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "ls"])
        .assert()
        .success()
        .stdout(predicates::str::contains("file\t6\tsample.txt"))
        .stderr("");
}

#[test]
fn backup_jsonl_emits_progress_events_without_stderr() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";

    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "init"])
        .assert()
        .success()
        .stderr("");

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");

    let backup_jsonl_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--jsonl",
            "backup",
            "--tag",
            "cli",
            source.to_str().expect("source path"),
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = backup_jsonl_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 9);
    let started: Value = serde_json::from_slice(lines[0]).expect("started event");
    assert_eq!(started["event"], "command_started");
    assert_eq!(started["command"], "backup");
    let progress: Vec<Value> = lines[1..8]
        .iter()
        .map(|line| serde_json::from_slice(line).expect("progress event"))
        .collect();
    assert_eq!(progress[0]["event"], "progress");
    assert_eq!(progress[0]["data"]["phase"], "walk_sources");
    assert_eq!(progress[6]["data"]["phase"], "complete");
    let completed: Value = serde_json::from_slice(lines[8]).expect("completed event");
    assert_eq!(completed["event"], "command_completed");
    assert_eq!(completed["data"]["tags"], serde_json::json!(["cli"]));
}

#[test]
fn backup_requires_initialized_repository_and_correct_password() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");

    fileferry()
        .env("FILEFERRY_PASSWORD", "test-passphrase")
        .args([
            "--repo",
            &repo_url,
            "backup",
            source.to_str().expect("source path"),
        ])
        .assert()
        .code(3)
        .stdout("")
        .stderr(predicates::str::contains("repository object write failed"));

    fileferry()
        .env("FILEFERRY_PASSWORD", "test-passphrase")
        .args(["--repo", &repo_url, "init"])
        .assert()
        .success()
        .stderr("");

    fileferry()
        .env("FILEFERRY_PASSWORD", "wrong-passphrase")
        .args([
            "--repo",
            &repo_url,
            "backup",
            source.to_str().expect("source path"),
        ])
        .assert()
        .code(4)
        .stdout("")
        .stderr(predicates::str::contains(
            "repository could not be unlocked",
        ));
}
