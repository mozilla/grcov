use std::path::Path;

#[cfg(unix)]
pub(crate) use std::os::unix::fs::symlink as symlink_file;

/// Creates a symbolic link to a file.
///
/// On Windows, creating a symbolic link to a file requires Administrator
/// rights. Fall back to copying if creating the symbolic link fails.
#[cfg(windows)]
pub(crate) fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(
    original: P,
    link: Q,
) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

    static HAVE_PRINTED_WARNING: AtomicBool = AtomicBool::new(false);

    std::os::windows::fs::symlink_file(&original, &link)
        .or_else(|_| {
            let _len: u64 = std::fs::copy(&original, &link)?;

            // Print a warning about symbolic links, but only once per grcov run.
            if HAVE_PRINTED_WARNING.compare_exchange(false, true, SeqCst, SeqCst).is_ok() {
                eprintln!(
                    "Failed to create a symlink, but successfully copied file (as fallback).\n\
                     This is less efficient. You can enable symlinks without elevating to Administrator.\n\
                     See instructions at https://github.com/mozilla/grcov/blob/master/README.md#enabling-symlinks-on-windows");
            }

            Ok(())
        }
    )
}
