// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

use std::io::Cursor;

use crate::cli::test_support::unique_test_directory;

use super::*;

#[test]
fn bounded_reader_rejects_bytes_past_the_limit() {
    assert!(matches!(
        read_bounded(Cursor::new(b"abcd"), 3),
        Err(ReadError::TooLarge)
    ));
    assert_eq!(read_bounded(Cursor::new(b"abc"), 3).unwrap(), b"abc");
}

#[test]
fn bounded_reader_retries_interrupted_and_short_reads_without_losing_the_limit() {
    struct InterruptedReader {
        bytes: Cursor<&'static [u8]>,
        interrupt: bool,
    }

    impl Read for InterruptedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let count = buffer.len().min(1);
            self.bytes.read(&mut buffer[..count])
        }
    }

    let reader = || InterruptedReader {
        bytes: Cursor::new(b"abcd"),
        interrupt: false,
    };
    assert_eq!(read_bounded(reader(), 4).unwrap(), b"abcd");
    assert!(matches!(
        read_bounded(reader(), 3),
        Err(ReadError::TooLarge)
    ));
}

#[test]
fn bounded_reader_preserves_the_io_failure() {
    struct FailedReader;

    impl Read for FailedReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "synthetic read failure",
            ))
        }
    }

    let ReadError::Io(error) = read_bounded(FailedReader, 10).unwrap_err() else {
        panic!("expected the original I/O error");
    };
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "synthetic read failure");
}

#[test]
fn output_commit_replaces_an_existing_file() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"old").unwrap();

    write_file(&output, b"new").unwrap();
    assert_eq!(fs::read(&output).unwrap(), b"new");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn guarded_commit_refuses_to_replace_a_changed_input_snapshot() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"newer source").unwrap();

    assert!(!write_file_if_unchanged(&output, b"old source", b"optimized").unwrap());
    assert_eq!(fs::read(&output).unwrap(), b"newer source");
    assert!(write_file_if_unchanged(&output, b"newer source", b"optimized").unwrap());
    assert_eq!(fs::read(&output).unwrap(), b"optimized");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn staging_skips_a_candidate_equal_to_the_requested_output() {
    let directory = unique_test_directory();
    let sequence = 12_345;
    let name = format!(".columbo-{}-{sequence}.tmp", std::process::id());
    let output = directory.join(&name);

    assert_eq!(
        temporary_output_candidate(&output, &directory, sequence),
        None
    );
    assert_eq!(
        temporary_output_candidate(Path::new(&name), Path::new("."), sequence),
        None
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn no_clobber_commit_creates_only_a_missing_destination() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");

    assert!(write_new_file(&output, b"first").unwrap());
    assert_eq!(fs::read(&output).unwrap(), b"first");
    assert!(!write_new_file(&output, b"second").unwrap());
    assert_eq!(fs::read(&output).unwrap(), b"first");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn staged_output_is_private_and_commit_preserves_destination_mode() {
    use std::os::unix::fs::PermissionsExt;

    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"old").unwrap();
    fs::set_permissions(&output, fs::Permissions::from_mode(0o751)).unwrap();

    let temporary = stage_private_output(&output, b"new").unwrap();
    assert_eq!(fs::read(&temporary.location.path).unwrap(), b"new");
    assert_eq!(
        fs::metadata(&temporary.location.path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&temporary.location.directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(fs::read(&output).unwrap(), b"old");

    publish_output(&output, temporary, None, CommitMode::Replace).unwrap();
    assert_eq!(fs::read(&output).unwrap(), b"new");
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o751
    );
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn output_commit_replaces_a_symlink_without_following_it() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let directory = unique_test_directory();
    let protected = directory.join("protected.bin");
    let output = directory.join("output.bin");
    fs::write(&protected, b"protected").unwrap();
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o644)).unwrap();
    symlink(&protected, &output).unwrap();

    write_file(&output, b"optimized").unwrap();
    assert_eq!(fs::read(&protected).unwrap(), b"protected");
    assert_eq!(fs::read(&output).unwrap(), b"optimized");
    assert!(!fs::symlink_metadata(&output)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn partial_write_failure_preserves_input_and_removes_staging() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"original").unwrap();

    let result = stage_output_with(&output, |file| {
        file.write_all(b"partial")?;
        Err(io::Error::new(
            io::ErrorKind::Other,
            "synthetic disk failure",
        ))
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&output).unwrap(), b"original");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn panic_during_staging_preserves_input_and_closes_before_cleanup() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"original").unwrap();

    let result = std::panic::catch_unwind(|| {
        let _ = stage_output_with(&output, |file| {
            file.write_all(b"partial")?;
            panic!("synthetic interruption");
        });
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&output).unwrap(), b"original");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn failed_publication_does_not_replace_a_directory_or_leave_staging() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("protected"), b"original").unwrap();

    assert!(write_file(&output, b"replacement").is_err());
    assert_eq!(fs::read(output.join("protected")).unwrap(), b"original");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn snapshot_check_happens_after_staging() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"original").unwrap();
    let temporary = stage_private_output(&output, b"optimized").unwrap();
    fs::write(&output, b"edited during staging").unwrap();

    assert!(!publish_output(&output, temporary, Some(b"original"), CommitMode::Replace).unwrap());
    assert_eq!(fs::read(&output).unwrap(), b"edited during staging");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn concurrent_creators_publish_exactly_one_complete_file() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    let barrier = std::sync::Barrier::new(8);
    let winners = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8_u8)
            .map(|byte| {
                let output = &output;
                let barrier = &barrier;
                scope.spawn(move || {
                    let bytes = vec![byte; 32 * 1024];
                    barrier.wait();
                    (byte, write_new_file(output, &bytes).unwrap())
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    let winning_bytes: Vec<_> = winners
        .into_iter()
        .filter_map(|(byte, won)| won.then_some(byte))
        .collect();
    assert_eq!(winning_bytes.len(), 1);
    assert_eq!(
        fs::read(&output).unwrap(),
        vec![winning_bytes[0]; 32 * 1024]
    );
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn non_regular_inputs_are_rejected() {
    assert!(matches!(
        read_file(Path::new("/dev/null"), 1024),
        Err(ReadError::NotRegular)
    ));
    let directory = unique_test_directory();
    assert!(matches!(
        read_file(&directory, 1024),
        Err(ReadError::NotRegular)
    ));
    fs::remove_dir_all(directory).unwrap();
}

/// Test hook called after publication, before the directory sync.
/// `COLUMBO_TEST_WRITE_PHASE` is set only in the `interrupted_writer_child` process.
pub(super) fn after_install() -> io::Result<()> {
    match std::env::var("COLUMBO_TEST_WRITE_PHASE").as_deref() {
        Ok("installed") => wait_to_be_killed(),
        Ok("sync-error") => Err(io::Error::new(
            io::ErrorKind::Other,
            "synthetic directory sync failure",
        )),
        _ => Ok(()),
    }
}

fn wait_to_be_killed() -> ! {
    let directory = PathBuf::from(std::env::var_os("COLUMBO_TEST_WRITE_DIRECTORY").unwrap());
    fs::write(directory.join("ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn interrupted_writer_child() {
    let Ok(phase) = std::env::var("COLUMBO_TEST_WRITE_PHASE") else {
        return;
    };
    let directory = PathBuf::from(std::env::var_os("COLUMBO_TEST_WRITE_DIRECTORY").unwrap());
    let output = directory.join("output.bin");
    let bytes = vec![0x5a; 64 * 1024];
    match phase.as_str() {
        "partial" => {
            let _ = stage_output_with(&output, |file| {
                file.write_all(&bytes[..1024])?;
                wait_to_be_killed()
            });
        }
        "staged" => {
            let _temporary = stage_private_output(&output, &bytes).unwrap();
            wait_to_be_killed();
        }
        "installed" => write_file(&output, &bytes).unwrap(),
        "sync-error" => {
            let error = write_file(&output, &bytes).unwrap_err();
            assert!(error.to_string().contains("output was installed"));
            assert!(error.to_string().contains("durability is uncertain"));
            assert_eq!(fs::read(&output).unwrap(), bytes);
        }
        _ => panic!("unknown child phase"),
    }
}

struct TestChild(std::process::Child);

impl Drop for TestChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_writer_child(directory: &Path, phase: &str) -> TestChild {
    TestChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cli::files::tests::interrupted_writer_child",
                "--nocapture",
            ])
            .env("COLUMBO_TEST_WRITE_PHASE", phase)
            .env("COLUMBO_TEST_WRITE_DIRECTORY", directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn killing_a_writer_keeps_complete_original_or_replacement_bytes() {
    for phase in ["partial", "staged", "installed"] {
        let directory = unique_test_directory();
        let output = directory.join("output.bin");
        let original = vec![0x31; 64 * 1024];
        fs::write(&output, &original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output, fs::Permissions::from_mode(0o751)).unwrap();
        }
        let mut child = start_writer_child(&directory, phase);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !directory.join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "writer exited before {phase}"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "writer stalled before {phase}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
        assert_eq!(
            fs::read(&output).unwrap(),
            if phase == "installed" {
                vec![0x5a; 64 * 1024]
            } else {
                original
            }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o751
            );
            for entry in fs::read_dir(&directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    assert_eq!(
                        entry.metadata().unwrap().permissions().mode() & 0o777,
                        0o700
                    );
                }
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn post_commit_sync_failure_reports_that_output_is_already_installed() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"original").unwrap();
    let mut child = start_writer_child(&directory, "sync-error");
    assert!(child.0.wait().unwrap().success());
    assert_eq!(fs::read(&output).unwrap(), vec![0x5a; 64 * 1024]);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}

thread_local! {
    static REPLACE_AFTER_COMPARISON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn after_snapshot_comparison(path: &Path) {
    if REPLACE_AFTER_COMPARISON.with(|replace| replace.replace(false)) {
        let replacement = path.with_extension("concurrent");
        fs::write(&replacement, b"new edit").unwrap();
        fs::rename(replacement, path).unwrap();
    }
}

#[test]
fn replacement_during_snapshot_comparison_is_not_overwritten() {
    let directory = unique_test_directory();
    let output = directory.join("output.bin");
    fs::write(&output, b"original").unwrap();
    REPLACE_AFTER_COMPARISON.with(|replace| replace.set(true));

    assert!(!write_file_if_unchanged(&output, b"original", b"optimized").unwrap());
    assert_eq!(fs::read(&output).unwrap(), b"new edit");
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}
