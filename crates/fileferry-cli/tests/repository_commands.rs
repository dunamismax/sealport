use assert_cmd::Command;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

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

fn set_modified_time(path: &Path, modified: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open file for timestamp update");
    file.set_times(fs::FileTimes::new().set_modified(modified))
        .expect("set file modified time");
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
    let keep_modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_modified_time(&source.join("nested").join("keep.txt"), keep_modified);
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
    assert_eq!(restore["data"]["metadata_planned"], 1);
    assert_eq!(restore["data"]["metadata_applied"], 1);
    assert_eq!(restore["data"]["metadata_warnings"], serde_json::json!([]));
    assert_eq!(restore["data"]["bytes_written"], 4);
    assert_eq!(restore["data"]["verified_files"], 1);
    assert_eq!(
        fs::read(destination.join("nested").join("keep.txt")).expect("restored nested file"),
        b"keep"
    );
    assert_eq!(
        fs::metadata(destination.join("nested").join("keep.txt"))
            .expect("restored nested metadata")
            .modified()
            .expect("restored nested modified time"),
        keep_modified
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
    assert_eq!(dry_run["data"]["metadata_planned"], 4);
    assert_eq!(dry_run["data"]["metadata_applied"], 0);
    assert_eq!(dry_run["data"]["metadata_warnings"], serde_json::json!([]));
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

#[cfg(unix)]
#[test]
fn restore_writes_directory_entries_and_symlinks_from_committed_snapshot() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::create_dir_all(source.join("empty/nested")).expect("create empty tree");
    fs::write(source.join("target.txt"), b"target").expect("write target");
    symlink("target.txt", source.join("target.link")).expect("create symlink");
    let backup = backup_source(&repo_url, passphrase, &source);

    let destination = temp.path().join("restore");
    let restore_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "restore",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let restore: Value = serde_json::from_slice(&restore_output).expect("restore json");
    assert_eq!(
        restore["data"]["snapshot_id"],
        backup["data"]["snapshot_id"]
    );
    assert_eq!(restore["data"]["entries_selected"], 5);
    assert_eq!(restore["data"]["directories_written"], 3);
    assert_eq!(restore["data"]["files_written"], 1);
    assert_eq!(restore["data"]["symlinks_written"], 1);
    assert_eq!(restore["data"]["metadata_planned"], 4);
    assert_eq!(restore["data"]["metadata_applied"], 4);
    assert_eq!(restore["data"]["metadata_warnings"], serde_json::json!([]));
    assert!(destination.join("empty/nested").is_dir());
    assert_eq!(
        fs::read(destination.join("target.txt")).expect("restored target"),
        b"target"
    );
    assert_eq!(
        fs::read_link(destination.join("target.link")).expect("restored symlink"),
        std::path::PathBuf::from("target.txt")
    );

    let blocked_destination = temp.path().join("blocked");
    fs::create_dir(&blocked_destination).expect("create blocked destination");
    symlink(temp.path(), blocked_destination.join("target.link"))
        .expect("create destination symlink");
    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "restore",
            "--overwrite",
            "--path",
            "target.link",
            blocked_destination
                .to_str()
                .expect("blocked destination path"),
        ])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicates::str::contains("contains a symlink"));
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
    assert_eq!(completed["data"]["metadata_planned"], 2);
    assert_eq!(completed["data"]["metadata_applied"], 2);
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
fn restore_json_failure_reports_missing_requested_path() {
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
    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "restore",
            "--path",
            "missing.txt",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .code(7)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let failure: Value = serde_json::from_slice(&output).expect("restore failure json");

    assert_eq!(failure["command"], "restore");
    assert_eq!(failure["status"], "failure");
    assert_eq!(failure["data"]["code"], "snapshot_path_not_found");
    assert_eq!(failure["data"]["exit_code"], 7);
    assert_eq!(failure["data"]["path"], serde_json::json!("missing.txt"));
    assert!(!destination.exists());
}

#[test]
fn restore_jsonl_failure_reports_missing_referenced_chunk_without_destination_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);
    let chunk_path = find_first_file(repo.join("objects/chunk"));
    fs::remove_file(&chunk_path).expect("remove chunk");

    let destination = temp.path().join("restore");
    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--jsonl",
            "restore",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let started: Value = serde_json::from_slice(lines[0]).expect("started event");
    assert_eq!(started["event"], "command_started");
    assert_eq!(started["command"], "restore");

    let failed: Value = serde_json::from_slice(lines[1]).expect("failed event");
    assert_eq!(failed["event"], "command_failed");
    assert_eq!(failed["command"], "restore");
    assert_eq!(failed["status"], "failure");
    assert_eq!(
        failed["data"]["code"],
        "repository_referenced_object_missing"
    );
    assert_eq!(failed["data"]["exit_code"], 6);
    assert!(
        failed["data"]["object_key"]
            .as_str()
            .expect("object key")
            .starts_with("objects/chunk/")
    );
    assert!(!destination.exists());
}

#[test]
fn check_verifies_initialized_local_repository() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let check_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "check"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let check: Value = serde_json::from_slice(&check_output).expect("check json");
    assert_eq!(check["command"], "check");
    assert_eq!(check["status"], "success");
    assert_eq!(check["data"]["metadata_objects_checked"], 3);
    assert_eq!(check["data"]["chunk_objects_checked"], 1);
    assert_eq!(check["data"]["read_data_mode"], "full");
    assert_eq!(check["data"]["read_data_subset"], serde_json::Value::Null);
    assert_eq!(check["data"]["errors"], serde_json::json!([]));
    assert_eq!(check["data"]["warnings"], serde_json::json!([]));
    assert!(check["data"]["bytes_read"].as_u64().expect("bytes read") > 0);

    let check_jsonl_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--jsonl", "check"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = check_jsonl_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 7);
    let progress: Vec<Value> = lines[1..6]
        .iter()
        .map(|line| serde_json::from_slice(line).expect("progress event"))
        .collect();
    assert_eq!(progress[0]["data"]["phase"], "load_commits");
    assert_eq!(progress[4]["data"]["phase"], "complete");
    let completed: Value = serde_json::from_slice(lines[6]).expect("completed event");
    assert_eq!(completed["event"], "command_completed");
    assert_eq!(completed["data"]["read_data_mode"], "full");
}

#[test]
fn check_requires_initialized_repository_correct_password_and_authentic_chunks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";

    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "check"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(predicates::str::contains("repository is not initialized"));

    init_repo(&repo_url, passphrase);
    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    fileferry()
        .env("FILEFERRY_PASSWORD", "wrong-passphrase")
        .args(["--repo", &repo_url, "check"])
        .assert()
        .code(4)
        .stdout("")
        .stderr(predicates::str::contains(
            "repository could not be unlocked",
        ));

    let chunk_path = find_first_file(repo.join("objects/chunk"));
    let mut bytes = fs::read(&chunk_path).expect("chunk bytes");
    bytes[0] ^= 0x01;
    fs::write(&chunk_path, bytes).expect("tamper chunk");
    fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "check"])
        .assert()
        .code(6)
        .stdout("")
        .stderr(predicates::str::contains("framing could not be decoded"));
}

#[test]
fn check_json_failure_reports_missing_chunk_as_machine_readable_finding() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let chunk_path = find_first_file(repo.join("objects/chunk"));
    fs::remove_file(&chunk_path).expect("delete chunk");

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "check"])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let failure: Value = serde_json::from_slice(&output).expect("check failure json");

    assert_eq!(failure["command"], "check");
    assert_eq!(failure["status"], "failure");
    assert_eq!(failure["data"]["code"], "repository_check_missing_object");
    assert_eq!(failure["data"]["exit_code"], 6);
    assert_eq!(failure["data"]["retryable"], false);
    assert!(
        failure["data"]["object_key"]
            .as_str()
            .expect("object key")
            .starts_with("objects/chunk/")
    );
    assert_eq!(
        failure["data"]["finding"]["code"],
        "repository_check_missing_object"
    );
    assert_eq!(failure["data"]["finding"]["severity"], "error");
    assert_eq!(
        failure["data"]["finding"]["object_key"],
        failure["data"]["object_key"]
    );
}

#[test]
fn check_jsonl_failure_reports_tampered_chunk_without_stderr() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let chunk_path = find_first_file(repo.join("objects/chunk"));
    let mut bytes = fs::read(&chunk_path).expect("chunk bytes");
    bytes[0] ^= 0x01;
    fs::write(&chunk_path, bytes).expect("tamper chunk");

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--jsonl", "check"])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let started: Value = serde_json::from_slice(lines[0]).expect("started event");
    assert_eq!(started["event"], "command_started");
    assert_eq!(started["command"], "check");

    let failed: Value = serde_json::from_slice(lines[1]).expect("failed event");
    assert_eq!(failed["event"], "command_failed");
    assert_eq!(failed["command"], "check");
    assert_eq!(failed["status"], "failure");
    assert_eq!(failed["data"]["code"], "repository_object_decode_failed");
    assert_eq!(failed["data"]["exit_code"], 6);
    assert!(
        failed["data"]["object_key"]
            .as_str()
            .expect("object key")
            .starts_with("objects/chunk/")
    );
    assert_eq!(
        failed["data"]["finding"]["code"],
        "repository_object_decode_failed"
    );
}

#[test]
fn repository_open_failures_are_structured_and_redacted_in_machine_modes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";

    let uninitialized_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "check"])
        .assert()
        .code(3)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let uninitialized: Value =
        serde_json::from_slice(&uninitialized_output).expect("uninitialized json");
    assert_eq!(uninitialized["command"], "check");
    assert_eq!(uninitialized["status"], "failure");
    assert_eq!(uninitialized["data"]["code"], "repository_not_initialized");
    assert_eq!(uninitialized["data"]["exit_code"], 3);

    let secret_url = "s3://access:secret@example.com/bucket?token=sensitive";
    let unsupported_output = fileferry()
        .args(["--repo", secret_url, "--json", "check"])
        .assert()
        .code(9)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let unsupported_text = String::from_utf8(unsupported_output.clone()).expect("unsupported utf8");
    let unsupported: Value = serde_json::from_slice(&unsupported_output).expect("unsupported json");
    assert_eq!(unsupported["data"]["code"], "repository_url_unsupported");
    assert_eq!(unsupported["data"]["exit_code"], 9);
    assert!(unsupported_text.contains("s3://<redacted>@example.com/bucket?<redacted>"));
    assert!(!unsupported_text.contains("secret"));
    assert!(!unsupported_text.contains("sensitive"));

    init_repo(&repo_url, passphrase);
    let wrong_password_output = fileferry()
        .env("FILEFERRY_PASSWORD", "wrong-passphrase")
        .args(["--repo", &repo_url, "--json", "snapshots"])
        .assert()
        .code(4)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let wrong_password_text =
        String::from_utf8(wrong_password_output.clone()).expect("wrong password utf8");
    let wrong_password: Value =
        serde_json::from_slice(&wrong_password_output).expect("wrong password json");
    assert_eq!(wrong_password["data"]["code"], "repository_unlock_failed");
    assert_eq!(wrong_password["data"]["exit_code"], 4);
    assert!(!wrong_password_text.contains("wrong-passphrase"));

    fs::write(repo.join("bootstrap"), b"not-json").expect("corrupt bootstrap");
    let bootstrap_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--jsonl", "snapshots"])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = bootstrap_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let failed: Value = serde_json::from_slice(lines[1]).expect("bootstrap failed event");
    assert_eq!(failed["event"], "command_failed");
    assert_eq!(failed["data"]["code"], "repository_bootstrap_decode_failed");
    assert_eq!(failed["data"]["exit_code"], 6);
}

#[test]
fn repository_open_reports_unsupported_bootstrap_version_and_features_as_incompatible() {
    let temp = tempfile::tempdir().expect("tempdir");
    let passphrase = "test-passphrase";

    let version_repo = temp.path().join("version-repo");
    let version_repo_url = version_repo.display().to_string();
    init_repo(&version_repo_url, passphrase);
    let mut bootstrap: Value =
        serde_json::from_slice(&fs::read(version_repo.join("bootstrap")).expect("bootstrap bytes"))
            .expect("bootstrap json");
    bootstrap["format_version"] = serde_json::json!(999);
    fs::write(
        version_repo.join("bootstrap"),
        serde_json::to_vec(&bootstrap).expect("unsupported version json"),
    )
    .expect("write unsupported version");
    let version_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &version_repo_url, "--json", "snapshots"])
        .assert()
        .code(3)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let version_failure: Value =
        serde_json::from_slice(&version_output).expect("version failure json");
    assert_eq!(version_failure["command"], "snapshots");
    assert_eq!(version_failure["status"], "failure");
    assert_eq!(
        version_failure["data"]["code"],
        "repository_format_unsupported"
    );
    assert_eq!(version_failure["data"]["exit_code"], 3);

    let feature_repo = temp.path().join("feature-repo");
    let feature_repo_url = feature_repo.display().to_string();
    init_repo(&feature_repo_url, passphrase);
    let mut bootstrap: Value =
        serde_json::from_slice(&fs::read(feature_repo.join("bootstrap")).expect("bootstrap bytes"))
            .expect("bootstrap json");
    bootstrap["features"] = serde_json::json!(["future-feature"]);
    fs::write(
        feature_repo.join("bootstrap"),
        serde_json::to_vec(&bootstrap).expect("unsupported feature json"),
    )
    .expect("write unsupported feature");
    let feature_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &feature_repo_url, "--jsonl", "check"])
        .assert()
        .code(3)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = feature_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let failed: Value = serde_json::from_slice(lines[1]).expect("feature failed event");
    assert_eq!(failed["event"], "command_failed");
    assert_eq!(failed["data"]["code"], "repository_features_unsupported");
    assert_eq!(failed["data"]["exit_code"], 3);
}

#[test]
fn snapshots_json_failure_reports_missing_referenced_manifest_as_integrity_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let manifest_path = find_first_file(repo.join("objects/manifest"));
    fs::remove_file(&manifest_path).expect("delete manifest");

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "snapshots"])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let failure: Value = serde_json::from_slice(&output).expect("snapshots failure json");

    assert_eq!(failure["command"], "snapshots");
    assert_eq!(failure["status"], "failure");
    assert_eq!(
        failure["data"]["code"],
        "repository_referenced_object_missing"
    );
    assert_eq!(failure["data"]["exit_code"], 6);
    assert!(
        failure["data"]["object_key"]
            .as_str()
            .expect("object key")
            .starts_with("objects/manifest/")
    );
}

#[test]
fn check_machine_failures_report_malformed_commits_and_corrupted_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let manifest_path = find_first_file(repo.join("objects/manifest"));
    let mut manifest_frame: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest frame"))
            .expect("manifest frame json");
    let first_ciphertext_byte = manifest_frame["ciphertext"]
        .as_array_mut()
        .expect("ciphertext array")
        .first_mut()
        .expect("ciphertext byte");
    let byte = first_ciphertext_byte
        .as_u64()
        .expect("ciphertext byte value");
    *first_ciphertext_byte = serde_json::json!(byte ^ 0x01);
    fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest_frame).expect("tampered manifest json"),
    )
    .expect("tamper manifest");

    let metadata_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "check"])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let metadata_failure: Value =
        serde_json::from_slice(&metadata_output).expect("metadata failure json");
    assert_eq!(
        metadata_failure["data"]["code"],
        "repository_object_authentication_failed"
    );
    assert!(
        metadata_failure["data"]["object_key"]
            .as_str()
            .expect("object key")
            .starts_with("objects/manifest/")
    );
    assert_eq!(
        metadata_failure["data"]["finding"]["object_key"],
        metadata_failure["data"]["object_key"]
    );

    init_repo(&repo_url, passphrase);
    let commit_path = find_first_file(repo.join("commits"));
    fs::write(&commit_path, b"not-json").expect("malform commit");
    let commit_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--jsonl", "check"])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = commit_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let failed: Value = serde_json::from_slice(lines[1]).expect("commit failed event");
    assert_eq!(failed["event"], "command_failed");
    assert_eq!(failed["data"]["code"], "repository_commit_decode_failed");
    assert!(
        failed["data"]["object_key"]
            .as_str()
            .expect("object key")
            .starts_with("commits/")
    );
    assert_eq!(
        failed["data"]["finding"]["code"],
        "repository_commit_decode_failed"
    );
}

fn find_first_file(root: std::path::PathBuf) -> std::path::PathBuf {
    let mut pending = vec![root];
    while let Some(path) = pending.pop() {
        if path.is_file() {
            return path;
        }
        let mut children = fs::read_dir(&path)
            .expect("read dir")
            .map(|entry| entry.expect("dir entry").path())
            .collect::<Vec<_>>();
        children.sort();
        children.reverse();
        pending.extend(children);
    }
    panic!("file not found");
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
        .stderr(predicates::str::contains("repository is not initialized"));

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
