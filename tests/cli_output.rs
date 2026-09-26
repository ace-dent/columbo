// SPDX-License-Identifier: MIT

//! Report streams and explicit output validation through the public CLI.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        for _ in 0..128 {
            let sequence = DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "columbo-cli-output-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create test directory: {error}"),
            }
        }
        panic!("could not create a unique test directory");
    }

    fn run(&self, destination: &Path, input: &[u8], extra: &[&str]) -> Output {
        fs::write(self.0.join("input.deflate"), input).unwrap();
        Command::new(env!("CARGO_BIN_EXE_columbo"))
            .current_dir(&self.0)
            .args(["--raw", "--verbose", "--out"])
            .arg(destination)
            .args(extra)
            .arg("input.deflate")
            .output()
            .unwrap()
    }

    fn run_report(&self, extra: &[&str], inputs: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_columbo"))
            .current_dir(&self.0)
            .args(["--raw", "--dry-run"])
            .args(extra)
            .args(inputs)
            .output()
            .unwrap()
    }

    fn assert_entries(&self, expected: &[&str]) {
        let mut entries: Vec<_> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        entries.sort();
        let mut expected: Vec<_> = expected.iter().map(std::ffi::OsString::from).collect();
        expected.sort();
        assert_eq!(entries, expected);
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct RestorePermissions {
    path: PathBuf,
    permissions: fs::Permissions,
}

impl RestorePermissions {
    fn capture(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            permissions: fs::metadata(path).unwrap().permissions(),
        }
    }
}

impl Drop for RestorePermissions {
    fn drop(&mut self) {
        fs::set_permissions(&self.path, self.permissions.clone()).unwrap();
    }
}

fn assert_destination_error(result: &Output) {
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.starts_with("cannot use --out destination "),
        "{stderr}"
    );
    assert!(!stderr.contains("could not optimize"), "{stderr}");
    assert!(result.stdout.is_empty());
}

fn assert_optimization_error(result: &Output) {
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("could not optimize"), "{stderr}");
    assert!(!stderr.contains("cannot use --out destination"), "{stderr}");
}

#[test]
fn redirected_reports_use_stdout_in_every_mode() {
    let directory = TestDirectory::new();
    fs::write(directory.0.join("input.deflate"), [0x03, 0x00]).unwrap();
    for flags in [&[][..], &["--verbose"][..], &["--visual"][..]] {
        let result = directory.run_report(flags, &["input.deflate"]);
        assert_eq!(result.status.code(), Some(0), "{result:?}");
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("\"input.deflate\""), "{stdout}");
        assert!(stdout.contains("dry run"), "{stdout}");
        if flags == ["--visual"] {
            assert!(stdout.contains("Result\n"), "{stdout}");
            assert!(!stdout.contains("Stream 01"), "{stdout}");
            assert_eq!(
                String::from_utf8_lossy(&result.stderr),
                "visual mode needs an interactive stdout terminal; continuing without stream maps\n"
            );
        } else {
            assert!(result.stderr.is_empty(), "{result:?}");
        }
        assert!(!result.stdout.contains(&0x1b), "{result:?}");
        assert!(!result.stderr.contains(&0x1b), "{result:?}");
        assert_eq!(
            fs::read(directory.0.join("input.deflate")).unwrap(),
            [0x03, 0x00]
        );
        directory.assert_entries(&["input.deflate"]);
    }
}

#[test]
fn relaxed_mode_warns_once_on_stderr_in_every_mode() {
    let directory = TestDirectory::new();
    for name in ["first.deflate", "second.deflate"] {
        fs::write(directory.0.join(name), [0x03, 0x00]).unwrap();
    }
    for mode in [None, Some("--verbose"), Some("--visual")] {
        let mut flags = vec!["--strict", "0"];
        flags.extend(mode);
        let result = directory.run_report(&flags, &["first.deflate", "second.deflate"]);
        assert_eq!(result.status.code(), Some(0), "{result:?}");
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stdout.contains("\"first.deflate\""), "{stdout}");
        assert!(stdout.contains("\"second.deflate\""), "{stdout}");
        assert!(!stdout.contains("Caution:"), "{stdout}");
        assert_eq!(stderr.matches("Caution: strict mode disabled;").count(), 1);
        assert!(!result.stdout.contains(&0x1b), "{result:?}");
        assert!(!result.stderr.contains(&0x1b), "{result:?}");
    }
}

#[test]
fn batch_errors_stay_on_stderr_and_later_reports_use_stdout() {
    let directory = TestDirectory::new();
    fs::write(directory.0.join("bad.deflate"), [0xff]).unwrap();
    fs::write(directory.0.join("good.deflate"), [0x03, 0x00]).unwrap();
    for flags in [&[][..], &["--verbose"][..], &["--visual"][..]] {
        let result = directory.run_report(flags, &["bad.deflate", "good.deflate"]);
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stdout.contains("\"good.deflate\""), "{stdout}");
        assert!(!stdout.contains("could not optimize"), "{stdout}");
        assert!(
            stderr.contains("could not optimize \"bad.deflate\":"),
            "{stderr}"
        );
        assert!(!stderr.contains("good.deflate"), "{stderr}");
        assert!(!result.stdout.contains(&0x1b), "{result:?}");
        assert!(!result.stderr.contains(&0x1b), "{result:?}");
    }
}

#[test]
fn help_uses_stdout_and_argument_errors_use_stderr() {
    let directory = TestDirectory::new();
    let help = directory.run_report(&["--help"], &[]);
    assert_eq!(help.status.code(), Some(0));
    assert!(help.stderr.is_empty(), "{help:?}");
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(
        stdout.contains("Reports go to stdout; warnings, errors, and the spinner go to stderr.")
    );

    let error = directory.run_report(&["--unknown"], &[]);
    assert_eq!(error.status.code(), Some(2));
    assert!(error.stdout.is_empty(), "{error:?}");
    assert!(String::from_utf8_lossy(&error.stderr).contains("unknown option: --unknown"));
}

#[test]
fn missing_parent_fails_before_optimization() {
    let directory = TestDirectory::new();
    // A reserved Deflate block type would fail optimization if reached.
    let result = directory.run(Path::new("missing/output.deflate"), &[0xff], &[]);
    assert_destination_error(&result);
    assert_eq!(fs::read(directory.0.join("input.deflate")).unwrap(), [0xff]);
    directory.assert_entries(&["input.deflate"]);
}

#[test]
fn directory_destination_fails_without_changing_its_contents() {
    let directory = TestDirectory::new();
    let output = directory.0.join("output");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep"), b"existing data").unwrap();
    let result = directory.run(&output, &[0xff], &[]);
    assert_destination_error(&result);
    assert_eq!(fs::read(output.join("keep")).unwrap(), b"existing data");
    directory.assert_entries(&["input.deflate", "output"]);
}

#[test]
fn file_used_as_parent_fails_before_optimization() {
    let directory = TestDirectory::new();
    fs::write(directory.0.join("parent"), b"existing data").unwrap();
    let result = directory.run(Path::new("parent/output.deflate"), &[0xff], &[]);
    assert_destination_error(&result);
    assert_eq!(
        fs::read(directory.0.join("parent")).unwrap(),
        b"existing data"
    );
    directory.assert_entries(&["input.deflate", "parent"]);
}

#[test]
fn output_without_a_final_file_component_fails_before_optimization() {
    let directory = TestDirectory::new();
    for destination in ["missing/", "missing/.", ".", ".."] {
        let result = directory.run(Path::new(destination), &[0xff], &[]);
        assert_destination_error(&result);
        directory.assert_entries(&["input.deflate"]);
    }
}

#[test]
fn read_only_output_fails_without_changing_the_file() {
    let directory = TestDirectory::new();
    let output = directory.0.join("output.deflate");
    fs::write(&output, b"existing data").unwrap();
    let restore = RestorePermissions::capture(&output);
    let mut permissions = restore.permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&output, permissions).unwrap();

    let result = directory.run(&output, &[0xff], &[]);
    assert_destination_error(&result);
    assert_eq!(fs::read(&output).unwrap(), b"existing data");
    assert!(fs::metadata(&output).unwrap().permissions().readonly());
    directory.assert_entries(&["input.deflate", "output.deflate"]);
}

#[cfg(unix)]
#[test]
fn unwritable_parent_fails_for_both_new_and_existing_output() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let directory = TestDirectory::new();
    if fs::metadata(&directory.0).unwrap().uid() == 0 {
        // Root can bypass ordinary Unix directory permissions.
        return;
    }
    let parent = directory.0.join("parent");
    fs::create_dir(&parent).unwrap();
    fs::write(parent.join("existing.deflate"), b"existing data").unwrap();
    let _restore = RestorePermissions::capture(&parent);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).unwrap();

    for name in ["missing.deflate", "existing.deflate"] {
        let result = directory.run(&parent.join(name), &[0xff], &[]);
        assert_destination_error(&result);
    }
    assert_eq!(
        fs::read(parent.join("existing.deflate")).unwrap(),
        b"existing data"
    );
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
    directory.assert_entries(&["input.deflate", "parent"]);
}

#[test]
fn successful_probe_leaves_existing_output_intact_and_cleans_up() {
    let directory = TestDirectory::new();
    let output = directory.0.join("output.deflate");
    fs::write(&output, b"existing data").unwrap();
    let before = fs::metadata(&output).unwrap();
    let result = directory.run(&output, &[0xff], &[]);
    assert_optimization_error(&result);
    assert_eq!(fs::read(&output).unwrap(), b"existing data");
    assert_eq!(
        fs::metadata(&output).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        assert_eq!(fs::metadata(&output).unwrap().ino(), before.ino());
    }
    directory.assert_entries(&["input.deflate", "output.deflate"]);
}

#[test]
fn successful_probe_does_not_create_the_final_destination() {
    let directory = TestDirectory::new();
    let result = directory.run(Path::new("output.deflate"), &[0xff], &[]);
    assert_optimization_error(&result);
    directory.assert_entries(&["input.deflate"]);
}

#[test]
fn dry_run_ignores_an_unusable_output_destination() {
    let directory = TestDirectory::new();
    let result = directory.run(
        Path::new("missing/output.deflate"),
        &[0x03, 0x00],
        &["--dry-run"],
    );
    assert!(result.status.success(), "{:?}", result);
    assert_eq!(
        fs::read(directory.0.join("input.deflate")).unwrap(),
        [0x03, 0x00]
    );
    directory.assert_entries(&["input.deflate"]);
}

#[cfg(unix)]
#[test]
fn symlink_probe_leaves_the_link_and_its_target_untouched() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let target = directory.0.join("target");
    let output = directory.0.join("output.deflate");
    fs::write(&target, b"target data").unwrap();
    symlink(&target, &output).unwrap();
    let result = directory.run(&output, &[0xff], &[]);
    assert_optimization_error(&result);
    assert_eq!(fs::read_link(&output).unwrap(), target);
    assert_eq!(fs::read(&target).unwrap(), b"target data");

    fs::remove_file(&target).unwrap();
    let result = directory.run(&output, &[0xff], &[]);
    assert_optimization_error(&result);
    assert_eq!(fs::read_link(&output).unwrap(), target);
    directory.assert_entries(&["input.deflate", "output.deflate"]);
}
