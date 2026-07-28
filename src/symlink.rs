use anyhow::{anyhow, Result};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

/// Read the current symlink and return the sha (basename of the target).
/// Returns None if the symlink doesn't exist.
pub fn read_current(current_path: &Path) -> Option<String> {
    let target = std::fs::read_link(current_path).ok()?;
    let name = target.file_name()?.to_str()?;
    Some(name.to_string())
}

/// Atomically advance the current symlink to point at `commits/<sha>`.
/// Uses tmp-symlink + rename(2), atomic on the same filesystem.
pub fn advance(current_path: &Path, sha: &str) -> Result<()> {
    let parent = current_path
        .parent()
        .ok_or_else(|| anyhow!("symlink path {} has no parent", current_path.display()))?;
    let tmp = parent.join(format!(".current.tmp.{}", std::process::id()));

    // Clean any leftover from a crashed prior invocation
    let _ = std::fs::remove_file(&tmp);

    let target = PathBuf::from(format!("commits/{sha}"));
    symlink(&target, &tmp)
        .map_err(|e| anyhow!("failed to create tmp symlink at {}: {}", tmp.display(), e))?;

    std::fs::rename(&tmp, current_path).map_err(|e| {
        // Best-effort cleanup on failure
        let _ = std::fs::remove_file(&tmp);
        anyhow!(
            "failed to rename {} -> {}: {}",
            tmp.display(),
            current_path.display(),
            e
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_10_1_first_time_creation() {
        let dir = TempDir::new().unwrap();
        let current = dir.path().join("current");
        assert!(read_current(&current).is_none());
        advance(&current, "abc1234").unwrap();
        assert_eq!(read_current(&current).unwrap(), "abc1234");
    }

    #[test]
    fn test_10_2_replacement() {
        let dir = TempDir::new().unwrap();
        let current = dir.path().join("current");
        advance(&current, "old").unwrap();
        advance(&current, "new").unwrap();
        assert_eq!(read_current(&current).unwrap(), "new");
    }

    #[test]
    fn test_read_missing() {
        let dir = TempDir::new().unwrap();
        assert!(read_current(&dir.path().join("nonexistent")).is_none());
    }
}
