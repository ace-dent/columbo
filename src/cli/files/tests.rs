// SPDX-License-Identifier: MIT

use std::io::Cursor;

use crate::cli::test_support::unique_test_directory;

use super::*;

#[test]
fn bounded_reader_rejects_bytes_past_the_limit() {
    assert_eq!(
        read_bounded(Cursor::new(b"abcd"), 3),
        Err(ReadError::TooLarge)
    );
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
    assert_eq!(read_bounded(reader(), 3), Err(ReadError::TooLarge));
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

    let (mut temporary, file) = stage_private_output(&output, b"new").unwrap();
    assert_eq!(fs::read(&temporary.path).unwrap(), b"new");
    assert_eq!(
        fs::metadata(&temporary.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read(&output).unwrap(), b"old");

    commit_temporary_output(&output, &mut temporary, file).unwrap();
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
