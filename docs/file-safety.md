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
   is `0700`; the staged file starts with mode `0600`.
2. Write the complete output and sync the file. Prepare the destination's
   ordinary Unix permission bits inside the private staging directory, then
   sync that metadata before publication.
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
| The process is killed immediately after publication | The destination contains the complete replacement, with ordinary Unix access bits already applied. |
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
inode. Ordinary Unix access bits are preserved for regular destinations;
set-user-ID, set-group-ID, and sticky bits are not copied. This portable
implementation does not preserve ownership, ACLs, extended attributes, or
Windows security descriptors; staging on Windows inherits directory security
settings. When that metadata must be kept, use an explicit output path and a
native installation step.

## Validation

Synthetic unit tests exercise partial write errors, panic unwinding, failed
publication, concurrent no-clobber writers, and input replacement during the
final comparison. Dedicated child processes are forcibly killed during partial
writing, after staging, and immediately after rename. These tests check exact
file contents and Unix permission bits without using the private corpus.
A simulated post-commit sync failure checks that the diagnostic describes the
already-installed output and that no rollback occurs. These tests do not
simulate a machine power failure or validate every filesystem implementation.
