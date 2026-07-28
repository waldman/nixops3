use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::traits::S3Ops;

/// Ensure the commit tree for `sha` is present at `commits_dir/<sha>/`.
/// If already present, no-op. Otherwise: list S3 prefix, fetch all objects,
/// write to a `.tmp-<sha>-<pid>/` dir, then atomically rename into place.
pub async fn ensure_local(s3: &dyn S3Ops, commits_dir: &Path, sha: &str) -> Result<()> {
    let target = commits_dir.join(sha);
    if target.exists() {
        debug!("tree already local: {}", sha);
        return Ok(());
    }

    let prefix = format!("commits/{sha}/");
    let keys = s3.list_prefix(&prefix).await?;
    if keys.is_empty() {
        return Err(anyhow!("no objects found under {}", prefix));
    }

    info!("fetching {} ({} objects)", prefix, keys.len());

    std::fs::create_dir_all(commits_dir)?;
    let tmp = commits_dir.join(format!(".tmp-{}-{}", sha, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    // Serial fetch. Homelab trees are small; parallelism can come later if needed.
    for key in &keys {
        let bytes = s3
            .get_object(key)
            .await?
            .ok_or_else(|| anyhow!("object listed but not fetchable: {}", key))?;
        let rel = key
            .strip_prefix(&prefix)
            .ok_or_else(|| anyhow!("key {} missing expected prefix {}", key, prefix))?;
        let dest = tmp.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, bytes)?;
    }

    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp);
        anyhow!(
            "failed to rename {} -> {}: {}",
            tmp.display(),
            target.display(),
            e
        )
    })?;

    Ok(())
}

/// Remove any orphaned `.tmp-*` dirs left by a previous crashed cycle.
pub fn cleanup_tmp_dirs(commits_dir: &Path) {
    let read = match std::fs::read_dir(commits_dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(".tmp-") {
            let path = entry.path();
            if let Err(e) = std::fs::remove_dir_all(&path) {
                warn!("failed to remove tmp dir {}: {}", path.display(), e);
            } else {
                debug!("cleaned up stale tmp: {}", path.display());
            }
        }
    }
}

/// Prune old trees, keeping the newest `retain` (by mtime) — never removing
/// `keep_sha` regardless of age.
pub fn prune(commits_dir: &Path, retain: usize, keep_sha: &str) {
    let read = match std::fs::read_dir(commits_dir) {
        Ok(r) => r,
        Err(_) => return,
    };

    let mut trees: Vec<(std::time::SystemTime, PathBuf, String)> = read
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.is_dir() {
                return None;
            }
            let mtime = meta.modified().ok()?;
            Some((mtime, entry.path(), name))
        })
        .collect();

    // Newest first
    trees.sort_by(|a, b| b.0.cmp(&a.0));

    if trees.len() <= retain {
        return;
    }

    for (_, path, name) in trees.into_iter().skip(retain) {
        if name == keep_sha {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(&path) {
            warn!("failed to prune {}: {}", path.display(), e);
        } else {
            debug!("pruned old tree: {}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_11_1_keep_newest_n() {
        let dir = TempDir::new().unwrap();
        for i in 0..8 {
            let p = dir.path().join(format!("sha{i}"));
            fs::create_dir(&p).unwrap();
            // Space out mtimes
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        prune(dir.path(), 5, "no-symlink");
        let count = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(count, 5);
    }

    #[test]
    fn test_11_2_never_delete_symlink_target() {
        let dir = TempDir::new().unwrap();
        // Create sha0 first (oldest), then 1..8
        fs::create_dir(dir.path().join("sha0")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        for i in 1..8 {
            let p = dir.path().join(format!("sha{i}"));
            fs::create_dir(&p).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Symlink points at sha0 (the oldest)
        prune(dir.path(), 5, "sha0");
        // 5 newest kept + sha0 preserved = 6 total
        let count = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(count, 6);
        assert!(dir.path().join("sha0").exists());
    }

    #[test]
    fn test_11_3_noop_when_under_retain() {
        let dir = TempDir::new().unwrap();
        for i in 0..3 {
            fs::create_dir(dir.path().join(format!("sha{i}"))).unwrap();
        }
        prune(dir.path(), 5, "sha0");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn test_11_4_ignores_tmp_dirs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".tmp-abc-1234")).unwrap();
        for i in 0..3 {
            fs::create_dir(dir.path().join(format!("sha{i}"))).unwrap();
        }
        prune(dir.path(), 2, "sha0");
        // .tmp-* should be untouched by pruning (cleanup_tmp_dirs handles it)
        assert!(dir.path().join(".tmp-abc-1234").exists());
    }

    #[test]
    fn test_cleanup_tmp_dirs() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".tmp-abc-1234")).unwrap();
        fs::create_dir(dir.path().join(".tmp-def-5678")).unwrap();
        fs::create_dir(dir.path().join("sha1")).unwrap();
        cleanup_tmp_dirs(dir.path());
        assert!(!dir.path().join(".tmp-abc-1234").exists());
        assert!(!dir.path().join(".tmp-def-5678").exists());
        assert!(dir.path().join("sha1").exists());
    }
}
