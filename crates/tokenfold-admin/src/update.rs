//! Local install/rollback primitives: swap the current binary for a new one, keeping a
//! backup copy so [`rollback`] can restore it. See the crate root doc comment for why this
//! operates on local files rather than a live update server.

use std::io;
use std::path::{Path, PathBuf};

/// The sibling backup path for `current_binary_path`, formed by appending `backup_suffix` to
/// the path's string form (e.g. `<path>.bak`).
fn backup_path(current_binary_path: &Path, backup_suffix: &str) -> PathBuf {
    let mut backup = current_binary_path.as_os_str().to_os_string();
    backup.push(backup_suffix);
    PathBuf::from(backup)
}

/// Backs up `current_binary_path`'s current contents to `<path><backup_suffix>`, then
/// overwrites `current_binary_path` with `new_binary_bytes`.
pub fn install_update(
    current_binary_path: &Path,
    new_binary_bytes: &[u8],
    backup_suffix: &str,
) -> io::Result<()> {
    let current_bytes = std::fs::read(current_binary_path)?;
    std::fs::write(
        backup_path(current_binary_path, backup_suffix),
        current_bytes,
    )?;
    std::fs::write(current_binary_path, new_binary_bytes)?;
    Ok(())
}

/// Restores `current_binary_path` from its `<path><backup_suffix>` backup file.
pub fn rollback(current_binary_path: &Path, backup_suffix: &str) -> io::Result<()> {
    let backup_bytes = std::fs::read(backup_path(current_binary_path, backup_suffix))?;
    std::fs::write(current_binary_path, backup_bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_rollback_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "tokenfold_admin_update_test_{}.bin",
            std::process::id()
        ));
        let backup_suffix = ".bak";
        let backup = backup_path(&path, backup_suffix);

        std::fs::write(&path, b"old bytes").unwrap();

        install_update(&path, b"new bytes", backup_suffix).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new bytes");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old bytes");

        rollback(&path, backup_suffix).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old bytes");

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&backup).ok();
    }
}
