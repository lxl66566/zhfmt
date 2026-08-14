//! End-to-end CLI tests, exercising the real binary.

use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use tempfile::TempDir;

fn zhfmt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zhfmt"))
}

fn write_file(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn write_mode_formats_in_place() {
    let tmp = TempDir::new().unwrap();
    let md = write_file(tmp.path(), "a.md", "中文test混排");
    let out = zhfmt().arg(&md).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&md).unwrap(), "中文 test 混排");
}

#[test]
fn check_mode_reports_and_keeps_file() {
    let tmp = TempDir::new().unwrap();
    let md = write_file(tmp.path(), "a.md", "中文test混排");
    let out = zhfmt().arg("--check").arg(&md).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("would reformat"));
    assert_eq!(fs::read_to_string(&md).unwrap(), "中文test混排");

    // Clean file: exit 0.
    let md2 = write_file(tmp.path(), "b.md", "已经 format 好的");
    let out2 = zhfmt().arg("--check").arg(&md2).output().unwrap();
    assert_eq!(out2.status.code(), Some(0));
}

#[test]
fn diff_mode_prints_unified_diff() {
    let tmp = TempDir::new().unwrap();
    let md = write_file(tmp.path(), "a.md", "中文test\n");
    let out = zhfmt().arg("--diff").arg(&md).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("-中文test"), "stdout: {stdout}");
    assert!(stdout.contains("+中文 test"), "stdout: {stdout}");
    assert_eq!(fs::read_to_string(&md).unwrap(), "中文test\n");
}

#[test]
fn stdin_pipeline_mode() {
    let mut child = zhfmt()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("中文test".as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "中文 test");
}

#[test]
fn walk_filters_by_extension_and_scans_hidden_files() {
    let tmp = TempDir::new().unwrap();
    let md = write_file(tmp.path(), "a.md", "中文test");
    let rs = write_file(tmp.path(), "b.rs", "中文test");
    let hidden = write_file(tmp.path(), ".hidden.md", "中文test");
    let out = zhfmt().arg(tmp.path()).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&md).unwrap(), "中文 test");
    assert_eq!(fs::read_to_string(&rs).unwrap(), "中文test");
    assert_eq!(fs::read_to_string(&hidden).unwrap(), "中文 test");
}

#[test]
fn walk_respects_gitignore() {
    let tmp = TempDir::new().unwrap();
    // Pretend to be a git repo so .gitignore applies.
    fs::create_dir(tmp.path().join(".git")).unwrap();
    write_file(tmp.path(), ".gitignore", "ignored.md");
    let ignored = write_file(tmp.path(), "ignored.md", "中文test");
    let kept = write_file(tmp.path(), "kept.md", "中文test");
    let out = zhfmt().arg(tmp.path()).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&ignored).unwrap(), "中文test");
    assert_eq!(fs::read_to_string(&kept).unwrap(), "中文 test");
}

#[test]
fn explicit_file_bypasses_extension_filter() {
    let tmp = TempDir::new().unwrap();
    let rs = write_file(tmp.path(), "b.rs", "中文test");
    let out = zhfmt().arg(&rs).output().unwrap();
    assert!(out.status.success());
    assert_eq!(fs::read_to_string(&rs).unwrap(), "中文 test");
}

#[test]
fn config_file_exclude() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "zhfmt.json", r#"{ "exclude": ["skip/**"] }"#);
    let sub = tmp.path().join("skip");
    fs::create_dir(&sub).unwrap();
    let skipped = write_file(&sub, "a.md", "中文test");
    let kept = write_file(tmp.path(), "kept.md", "中文test");
    let out = zhfmt().current_dir(tmp.path()).arg(".").output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&skipped).unwrap(), "中文test");
    assert_eq!(fs::read_to_string(&kept).unwrap(), "中文 test");
}

#[test]
fn nonexistent_path_reports_error() {
    let tmp = TempDir::new().unwrap();
    let out = zhfmt()
        .arg(tmp.path().join("does-not-exist.md"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error"), "stderr: {stderr}");
    assert!(stderr.contains("1 errors"), "stderr: {stderr}");
}

#[test]
fn config_discovered_from_parent_directory() {
    let tmp = TempDir::new().unwrap();
    // Config in the parent dir customizes extensions; run from a child dir.
    write_file(tmp.path(), "zhfmt.json", r#"{ "extensions": ["zz"] }"#);
    let child = tmp.path().join("child");
    fs::create_dir(&child).unwrap();
    let zz = write_file(&child, "a.zz", "中文test");
    let md = write_file(&child, "b.md", "中文test");
    let out = zhfmt().current_dir(&child).arg(".").output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&zz).unwrap(), "中文 test");
    assert_eq!(fs::read_to_string(&md).unwrap(), "中文test");
}

#[test]
fn invalid_config_exits_with_code_2() {
    let tmp = TempDir::new().unwrap();
    let cfg = write_file(tmp.path(), "bad.json", "{ not json");
    let out = zhfmt()
        .arg("--config")
        .arg(&cfg)
        .arg(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn unchanged_file_not_rewritten() {
    let tmp = TempDir::new().unwrap();
    let md = write_file(tmp.path(), "a.md", "已经 format 好的\n");
    let before = fs::metadata(&md).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let out = zhfmt().arg(&md).output().unwrap();
    assert!(out.status.success());
    let after = fs::metadata(&md).unwrap().modified().unwrap();
    assert_eq!(before, after, "unchanged file must not be rewritten");
}

#[test]
fn large_file_mmap_write_and_diff() {
    // Larger than SMALL_FILE_THRESHOLD (256 KiB) so the mmap path is used.
    // Windows cannot rename over a file with a live mapping, so this
    // regression-tests that the mapping is dropped before writing.
    let content = "中文test".repeat(70_000); // 420 KiB
    let tmp = TempDir::new().unwrap();
    let big = write_file(tmp.path(), "big.md", &content);

    // --diff over the mmap path keeps the file untouched.
    let out = zhfmt().arg("--diff").arg(&big).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&big).unwrap(),
        content,
        "diff keeps file"
    );

    // Write over the mmap path.
    let out = zhfmt().arg(&big).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&big).unwrap(),
        "中文 test ".repeat(70_000).trim_end()
    );
}
