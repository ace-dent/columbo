// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Bounded input reads and atomic output replacement.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const READ_BUFFER_BYTES: usize = 64 * 1024;
const TEMP_FILE_ATTEMPTS: usize = 128;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) enum ReadError {
    Io(io::Error),
    TooLarge,
    Allocation,
    NotRegular,
    Changed,
}

pub(super) fn read_file(path: &Path, maximum_size: u64) -> Result<Vec<u8>, ReadError> {
    // Reject devices, sockets, and FIFOs before opening, and recheck the
    // handle in case the path changed.
    if !fs::metadata(path).map_err(ReadError::Io)?.is_file() {
        return Err(ReadError::NotRegular);
    }
    let file = File::open(path).map_err(ReadError::Io)?;
    let metadata = file.metadata().map_err(ReadError::Io)?;
    if !metadata.is_file() {
        return Err(ReadError::NotRegular);
    }
    if metadata.len() > maximum_size {
        return Err(ReadError::TooLarge);
    }

    let bytes = read_bounded(&file, maximum_size)?;
    if !same_file_version(&metadata, &file.metadata().map_err(ReadError::Io)?) {
        return Err(ReadError::Changed);
    }
    Ok(bytes)
}

/// Unlike `Read::read_to_end`, growth uses fallible reservations, so memory
/// exhaustion is an error rather than an abort.
fn read_bounded(reader: impl Read, maximum_size: u64) -> Result<Vec<u8>, ReadError> {
    let mut bytes = Vec::new();
    let mut reader = reader.take(maximum_size.saturating_add(1));
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let count = read_retrying_interrupts(&mut reader, &mut buffer).map_err(ReadError::Io)?;
        if count == 0 {
            break;
        }
        let new_length = u64::try_from(bytes.len())
            .ok()
            .and_then(|length| length.checked_add(count as u64))
            .ok_or(ReadError::TooLarge)?;
        if new_length > maximum_size {
            return Err(ReadError::TooLarge);
        }
        bytes
            .try_reserve(count)
            .map_err(|_| ReadError::Allocation)?;
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(bytes)
}

fn read_retrying_interrupts(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

/// Publish complete output with one same-filesystem rename; earlier errors
/// leave any existing destination untouched.
pub(super) fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = stage_private_output(path, bytes)?;
    publish_output(path, temporary, None, CommitMode::Replace).map(|_| ())
}

/// Like [`write_file`], but return `Ok(false)` without replacing `path` if it
/// no longer holds `expected`. The check runs just before the rename and is not
/// an atomic compare-and-swap.
pub(super) fn write_file_if_unchanged(
    path: &Path,
    expected: &[u8],
    bytes: &[u8],
) -> io::Result<bool> {
    let temporary = stage_private_output(path, bytes)?;
    publish_output(path, temporary, Some(expected), CommitMode::Replace)
}

/// Publish a complete file only if the destination entry is still absent;
/// return `Ok(false)` if it exists. A hard link gives an atomic no-clobber
/// commit; without hard links this fails without exposing partial data.
pub(super) fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let temporary = stage_private_output(path, bytes)?;
    publish_output(path, temporary, None, CommitMode::CreateNew)
}

fn file_contents_equal(path: &Path, expected: &[u8]) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != expected.len() as u64 {
        return Ok(false);
    }

    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    let mut position = 0_usize;
    while position < expected.len() {
        let count = read_retrying_interrupts(&mut file, &mut buffer)?;
        if count == 0 || expected[position..].get(..count) != Some(&buffer[..count]) {
            return Ok(false);
        }
        position += count;
    }
    if read_retrying_interrupts(&mut file, &mut buffer[..1])? != 0 {
        return Ok(false);
    }
    #[cfg(test)]
    tests::after_snapshot_comparison(path);
    // Also catch edits to the file, or replacement of its directory entry,
    // during the comparison.
    let current = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(same_file_version(&metadata, &file.metadata()?) && same_file_version(&metadata, &current))
}

/// Compare the strongest file-version metadata the standard library exposes on
/// each platform. This supplements byte comparison; it does not lock a file.
fn same_file_version(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    if !left.is_file() || !right.is_file() || left.len() != right.len() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.mtime() == right.mtime()
            && left.mtime_nsec() == right.mtime_nsec()
            && left.ctime() == right.ctime()
            && left.ctime_nsec() == right.ctime_nsec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        // File IDs are not exposed by the project's minimum Rust version.
        left.creation_time() == right.creation_time()
            && left.last_write_time() == right.last_write_time()
            && left.file_attributes() == right.file_attributes()
    }
    #[cfg(not(any(unix, windows)))]
    {
        matches!((left.modified(), right.modified()), (Ok(left), Ok(right)) if left == right)
    }
}

pub(super) fn output_entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Reject an unusable explicit destination before any input is read. The probe
/// writes no data; publication still reports its own errors.
pub(super) fn validate_output_destination(path: &Path) -> io::Result<()> {
    let name = path.as_os_str().to_string_lossy();
    if path.file_name().is_none()
        || matches!(
            name.rsplit(std::path::is_separator).next(),
            Some("" | "." | "..")
        )
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output must name a file",
        ));
    }
    match fs::symlink_metadata(path) {
        // Publication replaces the link entry without modifying its target.
        Ok(metadata) if metadata.file_type().is_symlink() => {}
        Ok(metadata) if metadata.is_file() => {
            if metadata.permissions().readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "output is read-only",
                ));
            }
            // Let the OS check effective access, including ACLs; opening
            // without create or truncate leaves the bytes intact.
            let file = OpenOptions::new().write(true).open(path)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "output changed to a non-regular file",
                ));
            }
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output is not a regular file or symbolic link",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let _directory = ParentDirectory::open(output_parent(path))?;
    let location = TemporaryLocation::create(path)?;
    drop(create_temporary_output(&location.path)?);
    // Close the probe first (required on Windows); remove it explicitly to
    // report cleanup errors.
    fs::remove_file(&location.path)?;
    fs::remove_dir(&location.directory)?;
    Ok(())
}

/// Stage output in a new sibling directory whose Unix mode stays `0700`, so
/// the staged file stays private while its permissions and data are finalized.
fn stage_private_output(path: &Path, bytes: &[u8]) -> io::Result<TemporaryOutput> {
    stage_output_with(path, |file| file.write_all(bytes))
}

fn stage_output_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<TemporaryOutput> {
    let location = TemporaryLocation::create(path)?;
    let mut temporary = TemporaryOutput {
        file: create_temporary_output(&location.path)?,
        location,
    };
    write(&mut temporary.file)?;
    temporary.file.sync_all()?;
    Ok(temporary)
}

fn output_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn temporary_output_candidate(path: &Path, parent: &Path, sequence: u64) -> Option<PathBuf> {
    let candidate = parent.join(format!(".columbo-{}-{sequence}.tmp", std::process::id()));
    (candidate.file_name() != path.file_name()).then_some(candidate)
}

pub(super) fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if let (Ok(left), Ok(right)) = (fs::metadata(left), fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }

    matches!(
        (fs::canonicalize(left), fs::canonicalize(right)),
        (Ok(left), Ok(right)) if left == right
    )
}

#[derive(Clone, Copy)]
enum CommitMode {
    Replace,
    CreateNew,
}

/// Finish every fallible preparation step before the snapshot check and
/// publication. Never roll back or delete the destination after publication:
/// another process may already be using or replacing the new file.
fn publish_output(
    path: &Path,
    temporary: TemporaryOutput,
    expected: Option<&[u8]>,
    mode: CommitMode,
) -> io::Result<bool> {
    let directory = ParentDirectory::open(output_parent(path))?;
    if matches!(mode, CommitMode::Replace) {
        prepare_output_permissions(path, &temporary.file)?;
    }
    temporary.file.sync_all()?;
    // Discover unsupported directory syncs before changing the destination.
    directory.sync()?;

    let TemporaryOutput { file, location } = temporary;
    // Windows requires the handle to be closed before rename or cleanup.
    drop(file);
    if let Some(expected) = expected {
        if !file_contents_equal(path, expected)? {
            return Ok(false);
        }
    }

    match mode {
        CommitMode::Replace => fs::rename(&location.path, path)?,
        CommitMode::CreateNew => match fs::hard_link(&location.path, path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
            Err(error) => return Err(error),
        },
    }

    // File sync alone does not persist the directory entry on Unix; a failure
    // here is post-commit, so report it without undoing the output.
    directory.sync_after_install().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "output was installed, but directory sync failed; durability is uncertain: {error}"
            ),
        )
    })?;
    // Dropping `location` removes the staging entry on a best-effort basis; a
    // cleanup failure must not fail a durable publication.
    Ok(true)
}

/// Reject a non-regular destination and, on Unix, copy its access bits to the
/// staged file.
fn prepare_output_permissions(path: &Path, file: &File) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output is not a regular file or symbolic link",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Copy the rwx bits only; drop setuid, setgid, and sticky bits.
        file.set_permissions(fs::Permissions::from_mode(
            metadata.permissions().mode() & 0o777,
        ))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

#[cfg(unix)]
fn create_temporary_output(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_temporary_output(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Keep a directory handle across publication so the final sync refers to the
/// same directory that was checked during preparation.
struct ParentDirectory {
    #[cfg(unix)]
    file: File,
}

impl ParentDirectory {
    fn open(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                file: File::open(path)?,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(Self {})
        }
    }

    fn sync(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.file.sync_all()
        }
        #[cfg(not(unix))]
        {
            // The standard library has no directory flush on Windows, so the
            // rename's durability after power loss depends on the filesystem.
            Ok(())
        }
    }

    fn sync_after_install(&self) -> io::Result<()> {
        #[cfg(test)]
        tests::after_install()?;
        self.sync()
    }
}

/// Fields drop in order, so the file closes before its location guard
/// unlinks it, including after write errors and panics (needed on Windows).
struct TemporaryOutput {
    file: File,
    location: TemporaryLocation,
}

struct TemporaryLocation {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryLocation {
    fn create(output: &Path) -> io::Result<Self> {
        if output.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output has no file name",
            ));
        }
        for _ in 0..TEMP_FILE_ATTEMPTS {
            let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let Some(directory) =
                temporary_output_candidate(output, output_parent(output), sequence)
            else {
                continue;
            };
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let mut builder = builder;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&directory) {
                Ok(()) => {
                    let path = directory.join("output");
                    return Ok(Self { directory, path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique temporary output directory",
        ))
    }
}

impl Drop for TemporaryLocation {
    fn drop(&mut self) {
        // Remove only our own file and empty directory; never delete
        // recursively or sweep another process's staging data.
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.directory);
    }
}

#[cfg(test)]
mod tests;
