# File safety

The library operates on byte slices and returns an output buffer. The CLI
performs all input and output filesystem operations. It reads input through a
read-only handle and finishes optimization in memory before staging any output
bytes.

An explicit `--out` destination is checked before reading or optimizing input.
Missing parents, non-file destinations, read-only files, and access failures
produce an error and exit status 1. An existing regular output is opened for
writing without creating or truncating it, only to check effective access;
this also applies when `--out` names the input. Symlink destinations are checked
without opening their targets, because publication replaces the link itself.
The CLI creates and removes an empty private staging directory and file beside
the destination to check creation permissions. This probe changes directory
metadata but writes no payload bytes and performs no syncs. Dry runs ignore
`--out` and perform no probe.

The check does not reserve the destination or guarantee future disk space or
permissions; publication still handles errors from later changes and
filesystem-specific restrictions.

## Publishing an output

The CLI uses the following sequence for an output that should replace a file:

1. Create a unique staging directory beside the destination. On Unix its mode
   is `0700`, which keeps the staged file private; the file itself starts with
   the default mode for new files (`0666` less the umask).
2. Write the complete output and sync the file. When the destination is a
   regular file, give the staged file that file's owner and group, as far as
   permitted, and then its ordinary Unix access bits, inside the private
   staging directory. Sync that metadata before publication.
3. Open and sync the destination directory on Unix. Failure at this point
   leaves the destination unchanged.
4. For an in-place replacement, compare the current input bytes with the
   original snapshot. Recheck file version metadata and the directory entry
   after comparison to catch changes made during a long read.
5. Rename the complete staged file over the destination. The CLI never deletes
   or truncates the original as a preliminary step.
6. Sync the destination directory again on Unix, then remove the staging file
   and directory on a best-effort basis.

When copying an unchanged input to a missing `--out` destination, publication
uses a hard link instead of rename. That atomically refuses to overwrite a file
created by another writer. Filesystems without hard-link support return an
error; there is no fallback that exposes a partial destination.

The directory syncs follow the [Unix fsync
requirements](https://man7.org/linux/man-pages/man2/fsync.2.html): syncing a
file does not persist its directory entry. Replacement uses Rust's
[same-filesystem rename](https://doc.rust-lang.org/std/fs/fn.rename.html).

## Interruption and errors

| Event | Expected result on a filesystem supporting atomic replacement |
| --- | --- |
| Optimization fails, panics, or is killed | Input and existing output contents remain unchanged. |
| Staging or pre-commit syncing fails | The existing destination remains unchanged. Cleanup closes the file before removing staging data. |
| The process is killed before publication | The original remains in place. A private staging directory may remain. |
| The process is killed immediately after publication | The destination contains the complete replacement, with its Unix owner, group, and access bits already applied. |
| The final directory sync fails | The error says output was installed but durability is uncertain. The CLI does not attempt a rollback. |
| Staging cleanup fails after publication | The complete output remains installed; leftover staging data does not turn a successful publication into a failed write. |

Optimized builds abort on panic, so a panic also leaves the staging directory
in place, like a kill; debug builds unwind and remove it. A force kill cannot
run destructors or signal cleanup. An abandoned directory
has a name such as `.columbo-1234-0.tmp` beside the output. Remove it only after
confirming its writer has stopped. The program does not sweep old staging names,
which could belong to another running process.

Process termination and power loss are different failure modes. Unix builds
sync file data, supported permission metadata, and the destination directory.
Actual recovery after a system crash still depends on the filesystem and
storage device honoring those requests. Windows builds sync file data before
rename, but Rust's standard API does not provide the same directory-flush step;
this implementation does not promise power-loss durability of Windows renames.

## File access and metadata limits

Inputs must be regular files, or symlinks resolving to regular files. The CLI
checks the path before opening and the opened handle afterwards, and rejects a
snapshot whose size or available version metadata changed while it was read.
The byte comparison before replacement also checks Unix device/inode identity
and timestamps. The minimum supported Rust version exposes fewer identity
fields on Windows, where creation time, last-write time, attributes, and size
supplement the byte comparison.

These checks are not a lock or an atomic compare-and-swap. A concurrent writer
can still change the destination between the final check and rename; another
process can also move or retarget a parent directory. Use a stable directory
and avoid editing the same input concurrently. Network and unusual filesystems
may have different failure and atomicity semantics.

Replacing a symlink replaces the link entry and leaves its target untouched.
Replacing one hard-link name leaves other names referring to the previous
inode.

On Unix, replacing a regular file keeps its owner, group, and ordinary access
bits; set-user-ID, set-group-ID, and sticky bits are not copied. Ownership is
restored before the access bits, as far as the running user may: only root can
give the file to another user, and other users can only choose a group they
belong to. When the owner cannot be restored, the running user owns the
replacement. When the group cannot be restored, the replacement's group gets
only the access the original gave to others, so no one gains access through the
change of group. A new destination, including a replaced symlink, is owned by
the running user and gets the default mode for new files.

This portable implementation does not preserve ACLs, extended attributes, or
Windows security descriptors; staging on Windows inherits directory security
settings. When that metadata must be kept, use an explicit output path and a
native installation step.

## Standard input and output

An input of `-` reads standard input to its end within the input size limit.
A pipe has no file type or version to check, so the regular-file and
changed-input checks do not apply. With `--out`, the output is published as
described above.

`--stdout` bypasses publication. Optimization still finishes in memory before
the first output byte is written, so a failed optimization writes nothing to
standard output. A write error, such as a reader that closes the pipe early,
or a kill during the write leaves the reader with a partial file; exit status
1 is the only signal. There is no staging, sync, or atomic replacement, and
the reader receives nothing until the whole file is optimized.

Shell redirection happens before Columbo starts. On Unix, Columbo refuses to
run when stdout is the input file (the same device and inode, including through
a symlink or hard link), before reading it, so `>>` or `1<>` onto the input
leaves it unchanged. A `>` redirection onto the input, as in `columbo --stdout
image.png > image.png` or `columbo - < image.png > image.png`, has
already emptied it by then; Columbo reports the mistake but cannot undo it.
Windows builds have no such check, because the standard library exposes no
file identity there.

## Validation

Synthetic unit tests exercise partial write errors, panic unwinding, failed
publication, concurrent no-clobber writers, and input replacement during the
final comparison. Dedicated child processes are forcibly killed during partial
writing, after staging, and immediately after rename. These tests check exact
file contents and Unix permission bits without using the private corpus.
A simulated post-commit sync failure checks that the diagnostic describes the
already-installed output and that no rollback occurs. These tests do not
simulate a machine power failure or validate every filesystem implementation.
