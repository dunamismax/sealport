use assert_cmd::Command;
use fileferry_storage::{ObjectKey, ObjectKeyPrefix, ObjectStore, S3Store, S3StoreConfig};
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
        "FILEFERRY_S3_ENDPOINT",
        "FILEFERRY_S3_REGION",
        "FILEFERRY_S3_ACCESS_KEY_ID",
        "FILEFERRY_S3_SECRET_ACCESS_KEY",
        "FILEFERRY_S3_DISABLE_CONDITIONAL_CREATE",
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
    backup_source_with_tags(repo_url, passphrase, source, &["cli"])
}

fn backup_source_with_tags(
    repo_url: &str,
    passphrase: &str,
    source: &std::path::Path,
    tags: &[&str],
) -> Value {
    let mut args = vec!["--repo", repo_url, "--json", "backup"];
    for tag in tags {
        args.push("--tag");
        args.push(tag);
    }
    args.push(source.to_str().expect("source path"));

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(args)
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&output).expect("backup json")
}

fn file_count_under(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }

    let mut pending = vec![path.to_path_buf()];
    let mut count = 0;
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).expect("read directory") {
            let entry = entry.expect("directory entry");
            let file_type = entry.file_type().expect("entry type");
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                count += 1;
            }
        }
    }

    count
}

fn set_modified_time(path: &Path, modified: SystemTime) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open file for timestamp update");
    file.set_times(fs::FileTimes::new().set_modified(modified))
        .expect("set file modified time");
}

fn patterned_bytes(seed: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| ((index * 29 + seed * 11 + index / 3) % 251) as u8)
        .collect()
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
fn forget_dry_run_reports_plan_without_writing_markers() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let first_source = temp.path().join("first-source");
    let second_source = temp.path().join("second-source");
    fs::create_dir(&first_source).expect("create first source");
    fs::create_dir(&second_source).expect("create second source");
    fs::write(first_source.join("first.txt"), b"first").expect("write first");
    fs::write(second_source.join("second.txt"), b"second").expect("write second");
    backup_source_with_tags(&repo_url, passphrase, &first_source, &["old"]);
    backup_source_with_tags(&repo_url, passphrase, &second_source, &["new"]);

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "forget",
            "--dry-run",
            "--keep-last",
            "1",
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let forget: Value = serde_json::from_slice(&output).expect("forget json");
    assert_eq!(forget["command"], "forget");
    assert_eq!(forget["status"], "success");
    assert_eq!(forget["data"]["dry_run"], true);
    assert_eq!(forget["data"]["snapshots_matched"], 2);
    assert_eq!(forget["data"]["snapshots_forgotten"], 1);
    assert_eq!(forget["data"]["retained_snapshots"], 1);
    assert_eq!(forget["data"]["object_deletion"], false);
    assert_eq!(forget["data"]["marker_objects_written"], 0);
    assert_eq!(
        forget["data"]["candidate_snapshots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        forget["data"]["kept_snapshots"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        forget["data"]["forgotten_snapshots"][0]["reasons"],
        serde_json::json!(["not-matched-by-keep-rule"])
    );
    assert_eq!(file_count_under(&repo.join("forgets")), 0);

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
    assert_eq!(snapshots["data"]["snapshots"].as_array().unwrap().len(), 2);
}

#[test]
fn forget_keep_tag_writes_marker_and_does_not_delete_repository_objects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let keep_source = temp.path().join("keep-source");
    let drop_source = temp.path().join("drop-source");
    fs::create_dir(&keep_source).expect("create keep source");
    fs::create_dir(&drop_source).expect("create drop source");
    fs::write(keep_source.join("keep.txt"), b"keep").expect("write keep");
    fs::write(drop_source.join("drop.txt"), b"drop").expect("write drop");
    backup_source_with_tags(&repo_url, passphrase, &keep_source, &["keep"]);
    backup_source_with_tags(&repo_url, passphrase, &drop_source, &["drop"]);
    let commits_before = file_count_under(&repo.join("commits"));
    let manifests_before = file_count_under(&repo.join("objects").join("manifest"));
    let indexes_before = file_count_under(&repo.join("objects").join("index"));
    let chunks_before = file_count_under(&repo.join("objects").join("chunk"));

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "forget",
            "--keep-tag",
            "keep",
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let forget: Value = serde_json::from_slice(&output).expect("forget json");
    assert_eq!(forget["data"]["dry_run"], false);
    assert_eq!(forget["data"]["snapshots_forgotten"], 1);
    assert_eq!(forget["data"]["retained_snapshots"], 1);
    assert_eq!(forget["data"]["marker_objects_written"], 1);
    assert_eq!(
        forget["data"]["kept_snapshots"][0]["reasons"],
        serde_json::json!(["keep-tag:keep"])
    );
    assert!(
        forget["data"]["forgotten_snapshots"][0]["marker_object"]
            .as_str()
            .expect("marker object")
            .starts_with("forgets/")
    );
    assert_eq!(file_count_under(&repo.join("forgets")), 1);
    assert_eq!(file_count_under(&repo.join("commits")), commits_before);
    assert_eq!(
        file_count_under(&repo.join("objects").join("manifest")),
        manifests_before
    );
    assert_eq!(
        file_count_under(&repo.join("objects").join("index")),
        indexes_before
    );
    assert_eq!(
        file_count_under(&repo.join("objects").join("chunk")),
        chunks_before
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
    assert_eq!(snapshots["data"]["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(
        snapshots["data"]["snapshots"][0]["tags"],
        serde_json::json!(["keep"])
    );
}

#[test]
fn forget_jsonl_reports_progress_and_completion_envelope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let first_source = temp.path().join("first-source");
    let second_source = temp.path().join("second-source");
    fs::create_dir(&first_source).expect("create first source");
    fs::create_dir(&second_source).expect("create second source");
    fs::write(first_source.join("first.txt"), b"first").expect("write first");
    fs::write(second_source.join("second.txt"), b"second").expect("write second");
    backup_source_with_tags(&repo_url, passphrase, &first_source, &["first"]);
    backup_source_with_tags(&repo_url, passphrase, &second_source, &["second"]);

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--jsonl", "forget", "--keep-last", "1"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let events = String::from_utf8(output)
        .expect("jsonl utf8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("jsonl event"))
        .collect::<Vec<_>>();

    assert_eq!(events.first().unwrap()["event"], "command_started");
    assert_eq!(events.last().unwrap()["event"], "command_completed");
    assert_eq!(events.last().unwrap()["command"], "forget");
    assert_eq!(events.last().unwrap()["data"]["snapshots_forgotten"], 1);
    assert!(
        events
            .iter()
            .any(|event| event["data"]["phase"] == "write_forget_state")
    );
}

#[test]
fn forget_no_match_and_invalid_policy_have_stable_exit_codes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("sample.txt"), b"sample").expect("write sample");
    backup_source(&repo_url, passphrase, &source);

    let no_match_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "forget", "--keep-last", "1"])
        .assert()
        .code(7)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let no_match: Value = serde_json::from_slice(&no_match_output).expect("no-match json");
    assert_eq!(no_match["status"], "failure");
    assert_eq!(no_match["data"]["code"], "forget_no_snapshots_matched");
    assert_eq!(no_match["data"]["exit_code"], 7);

    let invalid_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args(["--repo", &repo_url, "--json", "forget", "--keep-last", "0"])
        .assert()
        .code(2)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let invalid: Value = serde_json::from_slice(&invalid_output).expect("invalid policy json");
    assert_eq!(invalid["status"], "failure");
    assert_eq!(invalid["data"]["code"], "retention_policy_count_invalid");
    assert_eq!(invalid["data"]["exit_code"], 2);

    let invalid_tag_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "forget",
            "--keep-tag",
            "bad,tag",
        ])
        .assert()
        .code(2)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let invalid_tag: Value = serde_json::from_slice(&invalid_tag_output).expect("invalid tag json");
    assert_eq!(invalid_tag["status"], "failure");
    assert_eq!(invalid_tag["data"]["code"], "retention_policy_tag_invalid");
    assert_eq!(invalid_tag["data"]["exit_code"], 2);
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
#[cfg(unix)]
fn restore_path_scoped_symlink_creates_missing_parent_directory() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir_all(source.join("links")).expect("create source links dir");
    fs::write(source.join("target.txt"), b"target").expect("write target");
    symlink("../target.txt", source.join("links/target.link")).expect("create symlink");
    let backup = backup_source(&repo_url, passphrase, &source);

    let destination = temp.path().join("restore");
    let restore_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "restore",
            "--path",
            "links/target.link",
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
    assert_eq!(restore["data"]["entries_selected"], 1);
    assert_eq!(restore["data"]["directories_written"], 0);
    assert_eq!(restore["data"]["files_written"], 0);
    assert_eq!(restore["data"]["symlinks_written"], 1);
    assert!(destination.join("links").is_dir());
    assert_eq!(
        fs::read_link(destination.join("links/target.link")).expect("restored symlink"),
        std::path::PathBuf::from("../target.txt")
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
fn restore_json_failure_preflights_destination_conflicts_before_writes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::create_dir(source.join("early")).expect("create early directory");
    fs::write(source.join("conflict.txt"), b"new").expect("write source conflict");
    backup_source(&repo_url, passphrase, &source);

    let destination = temp.path().join("restore");
    fs::create_dir(&destination).expect("create destination");
    fs::write(destination.join("conflict.txt"), b"old").expect("write destination conflict");

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "restore",
            destination.to_str().expect("destination path"),
        ])
        .assert()
        .code(2)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let failure: Value = serde_json::from_slice(&output).expect("restore failure json");

    assert_eq!(failure["command"], "restore");
    assert_eq!(failure["status"], "failure");
    assert_eq!(failure["data"]["code"], "restore_destination_exists");
    assert_eq!(failure["data"]["exit_code"], 2);
    assert!(
        failure["data"]["path"]
            .as_str()
            .expect("failure path")
            .ends_with("conflict.txt")
    );
    assert!(!destination.join("early").exists());
    assert_eq!(
        fs::read(destination.join("conflict.txt")).expect("existing destination file"),
        b"old"
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
fn check_read_data_subset_reports_count_and_percent_modes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("a.bin"), patterned_bytes(1, 700_000)).expect("write a");
    fs::write(source.join("b.bin"), patterned_bytes(2, 800_000)).expect("write b");
    fs::write(source.join("c.bin"), patterned_bytes(3, 900_000)).expect("write c");
    backup_source(&repo_url, passphrase, &source);

    let count_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "check",
            "--read-data-subset",
            "1",
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let count: Value = serde_json::from_slice(&count_output).expect("count subset json");
    assert_eq!(count["data"]["read_data_mode"], "subset");
    assert_eq!(count["data"]["read_data_subset"], "1");
    assert_eq!(count["data"]["chunk_objects_checked"], 1);

    let percent_output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--jsonl",
            "check",
            "--read-data-subset",
            "50%",
        ])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let lines: Vec<_> = percent_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    let completed: Value = serde_json::from_slice(lines.last().expect("completed event"))
        .expect("percent completed jsonl event");
    assert_eq!(completed["event"], "command_completed");
    assert_eq!(completed["data"]["read_data_mode"], "subset");
    assert_eq!(completed["data"]["read_data_subset"], "50%");
    assert!(
        completed["data"]["chunk_objects_checked"]
            .as_u64()
            .expect("checked chunks")
            >= 1
    );
}

#[test]
fn check_read_data_subset_rejects_invalid_arguments() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();

    for subset in ["0", "0%", "101%", "abc"] {
        fileferry()
            .args(["--repo", &repo_url, "check", "--read-data-subset", subset])
            .assert()
            .code(2);
    }
}

#[test]
fn check_read_data_subset_integrity_failure_exits_six() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo = temp.path().join("repo");
    let repo_url = repo.display().to_string();
    let passphrase = "test-passphrase";
    init_repo(&repo_url, passphrase);

    let source = temp.path().join("source");
    fs::create_dir(&source).expect("create source");
    fs::write(source.join("a.bin"), patterned_bytes(4, 700_000)).expect("write a");
    fs::write(source.join("b.bin"), patterned_bytes(5, 800_000)).expect("write b");
    backup_source(&repo_url, passphrase, &source);

    let chunk_path = find_first_file(repo.join("objects/chunk"));
    let mut bytes = fs::read(&chunk_path).expect("chunk bytes");
    bytes[0] ^= 0x01;
    fs::write(&chunk_path, bytes).expect("tamper selected chunk");

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .args([
            "--repo",
            &repo_url,
            "--json",
            "check",
            "--read-data-subset",
            "1",
        ])
        .assert()
        .code(6)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let failure: Value = serde_json::from_slice(&output).expect("check failure json");
    assert_eq!(failure["command"], "check");
    assert_eq!(failure["status"], "failure");
    assert_eq!(failure["data"]["exit_code"], 6);
    assert!(
        failure["data"]["object_key"]
            .as_str()
            .expect("object key")
            .starts_with("objects/chunk/")
    );
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
    assert!(unsupported_text.contains("s3://<redacted>"));
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

#[test]
fn init_s3_requires_environment_and_redacts_repository_url() {
    let output = fileferry()
        .env("FILEFERRY_PASSWORD", "test-passphrase")
        .args(["--repo", "s3://test-bucket/team/repo", "--json", "init"])
        .assert()
        .code(2)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output.clone()).expect("s3 init failure utf8");
    let failure: Value = serde_json::from_slice(&output).expect("s3 init failure json");

    assert_eq!(failure["command"], "init");
    assert_eq!(failure["status"], "failure");
    assert_eq!(failure["data"]["code"], "repository_s3_environment_missing");
    assert_eq!(failure["data"]["exit_code"], 2);
    assert!(text.contains("FILEFERRY_S3_ENDPOINT"));
    assert!(!text.contains("test-bucket"));
    assert!(!text.contains("team/repo"));
    assert!(!text.contains("test-passphrase"));

    let secret_output = fileferry()
        .env("FILEFERRY_PASSWORD", "test-passphrase")
        .args([
            "--repo",
            "s3://access:secret@example.com/bucket?token=sensitive",
            "--jsonl",
            "init",
        ])
        .assert()
        .code(2)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let secret_text = String::from_utf8(secret_output.clone()).expect("secret failure utf8");
    let lines: Vec<_> = secret_output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 2);
    let failed: Value = serde_json::from_slice(lines[1]).expect("secret failure event");
    assert_eq!(failed["event"], "command_failed");
    assert_eq!(failed["data"]["code"], "repository_s3_url_invalid");
    assert!(secret_text.contains("s3://<redacted>"));
    assert!(!secret_text.contains("secret"));
    assert!(!secret_text.contains("sensitive"));
}

#[test]
fn init_s3_live_integration_when_env_is_enabled() {
    if std::env::var("FILEFERRY_S3_INIT_INTEGRATION")
        .ok()
        .as_deref()
        != Some("1")
    {
        return;
    }

    let bucket = required_env("FILEFERRY_S3_BUCKET");
    let endpoint = required_env("FILEFERRY_S3_ENDPOINT");
    let region = required_env("FILEFERRY_S3_REGION");
    let access_key_id = required_env("FILEFERRY_S3_ACCESS_KEY_ID");
    let secret_access_key = required_env("FILEFERRY_S3_SECRET_ACCESS_KEY");
    let test_prefix = required_env("FILEFERRY_S3_TEST_PREFIX");
    let repo_prefix = format!("{test_prefix}/cli-init-{}", unique_test_id());
    let repo_url = format!("s3://{bucket}/{repo_prefix}");
    let passphrase = "s3-init-test-passphrase";

    let output = fileferry()
        .env("FILEFERRY_PASSWORD", passphrase)
        .env("FILEFERRY_S3_ENDPOINT", &endpoint)
        .env("FILEFERRY_S3_REGION", &region)
        .env("FILEFERRY_S3_ACCESS_KEY_ID", &access_key_id)
        .env("FILEFERRY_S3_SECRET_ACCESS_KEY", &secret_access_key)
        .env("FILEFERRY_S3_DISABLE_CONDITIONAL_CREATE", "1")
        .args(["--repo", &repo_url, "--json", "init"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output.clone()).expect("s3 init output utf8");
    let init: Value = serde_json::from_slice(&output).expect("s3 init json");

    assert_eq!(init["command"], "init");
    assert_eq!(init["status"], "success");
    assert_eq!(init["data"]["backend"], "s3_compatible");
    assert_eq!(init["data"]["created"], true);
    assert_eq!(init["data"]["repository_url"], "s3://<redacted>");
    assert!(!text.contains(&bucket));
    assert!(!text.contains(&repo_prefix));
    assert!(!text.contains(&access_key_id));
    assert!(!text.contains(&secret_access_key));

    let cleanup_config = S3StoreConfig::new(
        bucket,
        region,
        endpoint,
        access_key_id,
        secret_access_key,
        ObjectKeyPrefix::new(repo_prefix).expect("test prefix"),
    )
    .expect("cleanup s3 config")
    .with_conditional_create(false);
    let cleanup_store = S3Store::new(cleanup_config).expect("cleanup s3 store");
    let runtime = tokio::runtime::Runtime::new().expect("cleanup runtime");
    runtime
        .block_on(cleanup_store.delete(&ObjectKey::new("bootstrap").expect("bootstrap key")))
        .expect("cleanup bootstrap");
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for S3 init integration"))
}

fn unique_test_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
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
