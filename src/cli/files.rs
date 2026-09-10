// SPDX-License-Identifier: MIT

//! Bounded input reads and atomic output replacement.

use std::ffi::OsString;
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
}

pub(super) fn read_file(path: &Path, maximum_size: u64) -> Result<Vec<u8>, ReadError> {
    let file = File::open(path).map_err(ReadError::Io)?;
    if file.metadata().map_err(ReadError::Io)?.len() > maximum_size {
        return Err(ReadError::TooLarge);
    }

    read_bounded(file, maximum_size)
}

/// Read through a fixed stack buffer so allocation failure is recoverable.
///
/// `Read::read_to_end` grows its destination internally. Explicit fallible
/// reservations keep a hostile size-changing or seekless input from turning
/// memory pressure into an allocation panic or abort.
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

/// Interrupted reads consumed no bytes and can be retried without changing
/// the input limit or the snapshot comparison position.
fn read_retrying_interrupts(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

/// Commit output through a new sibling file.
///
/// `create_new` prevents a symlink race on the temporary name, and the final
/// rename replaces the directory entry rather than following a destination
/// symlink. A failed write or pre-commit sync leaves any existing output
/// untouched.
pub(super) fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (mut temporary, file) = stage_private_output(path, bytes)?;
    commit_temporary_output(path, &mut temporary, file)
}

/// Recheck that an input still contains the snapshot that was optimized. The
/// comparison happens after staging, immediately before the atomic rename, so
/// a long optimization cannot silently discard a newer source version. Like
/// any portable path check followed by rename, a very small race remains.
pub(super) fn write_file_if_unchanged(
    path: &Path,
    expected: &[u8],
    bytes: &[u8],
) -> io::Result<bool> {
    let (mut temporary, file) = stage_private_output(path, bytes)?;
    if !file_contents_equal(path, expected)? {
        return Ok(false);
    }
    commit_temporary_output(path, &mut temporary, file)?;
    Ok(true)
}

fn file_contents_equal(path: &Path, expected: &[u8]) -> io::Result<bool> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() != expected.len() as u64 {
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
    Ok(read_retrying_interrupts(&mut file, &mut buffer[..1])? == 0)
}

/// Install a synced sibling only if no directory entry exists at `path`.
///
/// A hard link provides an atomic no-clobber commit: unlike `rename`, it fails
/// when another process creates the destination between the caller's
/// existence check and this commit. The sibling lives on the same filesystem,
/// so a successful link cannot fail because of a cross-device boundary.
pub(super) fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let (mut temporary, file) = stage_private_output(path, bytes)?;
    drop(file);
    match fs::hard_link(&temporary.path, path) {
        Ok(()) => {
            fs::remove_file(&temporary.path)?;
            temporary.committed = true;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn output_entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Write and sync a collision-safe sibling before replacing the destination.
///
/// Unix builds force mode `0600` throughout staging. Keeping this phase
/// separate from the final commit avoids exposing partial data through the
/// destination's ordinary access bits.
/// Platform ACLs and Windows security descriptors are outside Rust's standard
/// file API: a sibling can inherit them from its directory, and atomic rename
/// installs the sibling's metadata instead of preserving the destination ACL.
/// Extended authorization can therefore change when replacing an existing
/// file; ACL-sensitive callers should use a new output path and a native,
/// policy-aware replacement step.
fn stage_private_output(path: &Path, bytes: &[u8]) -> io::Result<(TemporaryOutput, File)> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output has no file name",
        ));
    }

    let mut opened = None;
    for _ in 0..TEMP_FILE_ATTEMPTS {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let Some(temporary_path) = temporary_output_candidate(path, parent, sequence) else {
            continue;
        };
        match create_temporary_output(&temporary_path) {
            Ok(file) => {
                opened = Some((temporary_path, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let (temporary_path, mut file) = opened.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique temporary output",
        )
    })?;
    let temporary = TemporaryOutput::new(temporary_path);
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok((temporary, file))
}

fn temporary_output_candidate(path: &Path, parent: &Path, sequence: u64) -> Option<PathBuf> {
    let temporary_name = OsString::from(format!(".columbo-{}-{sequence}.tmp", std::process::id()));
    let candidate = parent.join(temporary_name);
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

/// Atomically install a synced private sibling.
///
/// Unix permits renaming an open file. Its handle is deliberately retained so
/// a path substitution after the rename cannot redirect the permission update.
#[cfg(unix)]
fn commit_temporary_output(
    path: &Path,
    temporary: &mut TemporaryOutput,
    file: File,
) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let destination_mode = existing_regular_output_mode(path)?;
    fs::rename(&temporary.path, path)?;
    temporary.committed = true;

    if let Some(mode) = destination_mode {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        // The data was synced while private. Sync again after chmod so success
        // also means the restored access bits have reached the filesystem.
        file.sync_all()?;
    }
    Ok(())
}

/// Close before rename on platforms such as Windows, preserving prior
/// behavior where an open handle cannot be atomically moved over the output.
#[cfg(not(unix))]
fn commit_temporary_output(
    path: &Path,
    temporary: &mut TemporaryOutput,
    file: File,
) -> io::Result<()> {
    drop(file);
    fs::rename(&temporary.path, path)?;
    temporary.committed = true;
    Ok(())
}

#[cfg(unix)]
fn create_temporary_output(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    // Do not expose partially written data through the temporary name in a
    // shared directory. Existing destination access bits are restored only
    // after the private file has been renamed; a new output remains private.
    let file = options.open(path)?;
    if let Err(error) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

#[cfg(not(unix))]
fn create_temporary_output(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    options.open(path)
}

/// Preserve ordinary Unix access bits when replacing an existing output.
/// Special mode bits are intentionally not copied to newly written data.
#[cfg(unix)]
fn existing_regular_output_mode(path: &Path) -> io::Result<Option<u32>> {
    use std::os::unix::fs::PermissionsExt;

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            Ok(Some(metadata.permissions().mode() & 0o777))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

struct TemporaryOutput {
    path: PathBuf,
    committed: bool,
}

impl TemporaryOutput {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }
}

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests;
