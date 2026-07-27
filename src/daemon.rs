use anyhow::Result;
use std::path::Path;
use tracing::{error, info};

use crate::canary::check_canary;
use crate::config::Config;
use crate::hash::compute_hash;
use crate::inventory::{run_queries, write_heartbeat, write_inventory};
use crate::nixgen::generate_configuration_nix;
use crate::paths::{host_main_nix, host_queries_toml, role_main_nix, role_queries_toml};
use crate::queries::{merge_queries, parse_queries};
use crate::secrets::fetch_secrets;
use crate::traits::{DynamoOps, Executor, S3Ops, SecretsOps};
use crate::types::{CycleOutcome, LastRunStatus};

pub async fn run_cycle(
    config: &Config,
    hostname: &str,
    s3: &dyn S3Ops,
    dynamo: Option<&dyn DynamoOps>,
    secrets: &dyn SecretsOps,
    executor: &dyn Executor,
    work_dir: &Path,
    hash_path: &Path,
    secrets_dir: &Path,
) -> CycleOutcome {
    info!("poll cycle started for hostname={}", hostname);

    // Step 1: Canary check
    match check_canary(s3, hostname).await {
        Ok(true) => {}
        Ok(false) => {
            info!("canary active, hostname not listed — skipping");
            write_heartbeat(config, hostname, dynamo, LastRunStatus::CanarySkip, executor).await;
            return CycleOutcome::CanarySkip;
        }
        Err(e) => {
            error!("canary check failed: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, executor).await;
            return CycleOutcome::S3Error;
        }
    }

    // Step 2: Download config tree (two-pass)
    let download = download_config_tree(s3, config, hostname).await;
    let (files, has_host, query_contents) = match download {
        Ok(v) => v,
        Err(e) => {
            error!("s3 download failed: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, executor).await;
            return CycleOutcome::S3Error;
        }
    };

    // Step 3: Write downloaded files to work_dir
    if let Err(e) = write_files_to_dir(&files, work_dir) {
        error!("failed to write files to work_dir: {}", e);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, executor).await;
        return CycleOutcome::S3Error;
    }

    // Step 4: Compute hash of .nix files
    let new_hash = compute_hash(&files);

    // Step 5: Read existing hash
    let old_hash = std::fs::read_to_string(hash_path).unwrap_or_default();

    // Step 6: If unchanged, skip
    if new_hash == old_hash {
        info!("hash unchanged — skipping rebuild");
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Ok, executor).await;
        return CycleOutcome::HashUnchanged;
    }

    // Step 7: Inventory queries (if enabled)
    if config.inventory.enabled {
        if let (Some(dynamo_ref), Some(table)) = (dynamo, &config.inventory.table) {
            let query_lists: Vec<_> = query_contents
                .iter()
                .filter_map(|content| parse_queries(content).ok())
                .collect();
            let queries = merge_queries(&query_lists);
            let query_results = run_queries(&queries, dynamo_ref, table).await;
            let inv_path = Path::new("/var/lib/nixops3/inventory.json");
            if let Err(e) = write_inventory(&query_results, inv_path) {
                error!("failed to write inventory.json: {}", e);
            }
        }
    }

    // Step 8: Fetch secrets
    if let Err(e) = fetch_secrets(secrets, &config.role, hostname, secrets_dir).await {
        error!("secrets fetch error: {}", e);
    }

    // Step 9: Generate configuration.nix
    let host_arg = if has_host { Some(hostname) } else { None };
    let config_nix = generate_configuration_nix(&config.role, host_arg);
    let config_nix_path = work_dir.join("configuration.nix");
    if let Err(e) = std::fs::write(&config_nix_path, &config_nix) {
        error!("failed to write configuration.nix: {}", e);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, executor).await;
        return CycleOutcome::S3Error;
    }

    // Step 10: Run nixos-rebuild
    // -I nixops3=<work_dir> lets role main.nix use <nixops3/profiles/...> imports
    let nixos_config_arg = format!("nixos-config={}", work_dir.display());
    let nixops3_arg = format!("nixops3={}", work_dir.display());
    let args = &["switch", "-I", &nixos_config_arg, "-I", &nixops3_arg];
    info!("running nixos-rebuild switch");

    match executor.run("nixos-rebuild", args) {
        Ok((0, _)) => {
            info!("apply succeeded");
            if let Err(e) = std::fs::write(hash_path, &new_hash) {
                error!("failed to write hash: {}", e);
            }
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Ok, executor).await;
            CycleOutcome::Applied
        }
        Ok((code, output)) => {
            error!("apply failed (exit {}): {}", code, output);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, executor).await;
            CycleOutcome::RebuildFailed
        }
        Err(e) => {
            error!("apply failed to launch: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, executor).await;
            CycleOutcome::RebuildFailed
        }
    }
}

/// Two-pass download of the config tree.
///
/// Pass 1: fetch role `main.nix` and host `main.nix`. Scan both for `<nixops3/...>`
///         path references to discover which profiles are needed.
/// Pass 2: fetch exactly those profile files.
///
/// Returns `(files, has_host_main_nix, query_toml_contents)`.
async fn download_config_tree(
    s3: &dyn S3Ops,
    config: &Config,
    hostname: &str,
) -> Result<(Vec<(String, Vec<u8>)>, bool, Vec<String>)> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut query_contents: Vec<String> = Vec::new();

    // Pass 1 — role main.nix
    let role_main = role_main_nix(&config.role);
    let role_bytes = s3.get_object(&role_main).await?;

    // Pass 1 — host main.nix (optional)
    let host_main = host_main_nix(&config.role, hostname);
    let host_bytes = s3.get_object(&host_main).await?;
    let has_host = host_bytes.is_some();

    // Scan both for <nixops3/...> imports
    let mut profile_keys: Vec<String> = Vec::new();
    if let Some(ref bytes) = role_bytes {
        for key in extract_nixops3_imports(&String::from_utf8_lossy(bytes)) {
            if !profile_keys.contains(&key) {
                profile_keys.push(key);
            }
        }
    }
    if let Some(ref bytes) = host_bytes {
        for key in extract_nixops3_imports(&String::from_utf8_lossy(bytes)) {
            if !profile_keys.contains(&key) {
                profile_keys.push(key);
            }
        }
    }

    // Pass 2 — fetch referenced profiles
    for key in profile_keys {
        if let Some(bytes) = s3.get_object(&key).await? {
            files.push((key, bytes));
        }
    }

    // Add main.nix files after profiles (order doesn't affect hash — sorted by key)
    if let Some(bytes) = role_bytes {
        files.push((role_main, bytes));
    }
    if let Some(bytes) = host_bytes {
        files.push((host_main, bytes));
    }

    // queries.toml files
    let role_qt = role_queries_toml(&config.role);
    if let Some(bytes) = s3.get_object(&role_qt).await? {
        if let Ok(s) = String::from_utf8(bytes.clone()) {
            query_contents.push(s);
        }
        files.push((role_qt, bytes));
    }

    let host_qt = host_queries_toml(&config.role, hostname);
    if let Some(bytes) = s3.get_object(&host_qt).await? {
        if let Ok(s) = String::from_utf8(bytes.clone()) {
            query_contents.push(s);
        }
        files.push((host_qt, bytes));
    }

    Ok((files, has_host, query_contents))
}

/// Extracts `<nixops3/...>` path references from a Nix source file.
/// Returns S3 keys (the path inside the angle brackets).
fn extract_nixops3_imports(content: &str) -> Vec<String> {
    let prefix = "<nixops3/";
    let mut paths = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find(prefix) {
        rest = &rest[start + prefix.len()..];
        if let Some(end) = rest.find('>') {
            let path = rest[..end].trim().to_string();
            if !path.is_empty() && !paths.contains(&path) {
                paths.push(path);
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    paths
}

fn write_files_to_dir(files: &[(String, Vec<u8>)], dir: &Path) -> Result<()> {
    for (key, bytes) in files {
        let dest = dir.join(key);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_no_imports() {
        assert!(extract_nixops3_imports("{ ... }: {}").is_empty());
    }

    #[test]
    fn test_extract_single_import() {
        let nix = r#"{ ... }: { imports = [ <nixops3/profiles/base.nix> ]; }"#;
        assert_eq!(extract_nixops3_imports(nix), vec!["profiles/base.nix"]);
    }

    #[test]
    fn test_extract_multiple_imports() {
        let nix = r#"
            imports = [
                <nixops3/profiles/base.nix>
                <nixops3/profiles/docker.nix>
            ];
        "#;
        let result = extract_nixops3_imports(nix);
        assert_eq!(result, vec!["profiles/base.nix", "profiles/docker.nix"]);
    }

    #[test]
    fn test_extract_deduplicates() {
        let nix = "<nixops3/profiles/base.nix> <nixops3/profiles/base.nix>";
        let result = extract_nixops3_imports(nix);
        assert_eq!(result, vec!["profiles/base.nix"]);
    }
}
