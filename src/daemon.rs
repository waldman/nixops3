use anyhow::Result;
use fs2::FileExt;
use std::path::Path;
use tracing::{debug, error, info};

use crate::canary::check_canary;
use crate::config::Config;
use crate::inventory::{run_queries, write_heartbeat, write_inventory};
use crate::nixgen::generate_configuration_nix;
use crate::paths::POINTER_KEY;
use crate::pointer;
use crate::queries::{merge_queries, parse_queries};
use crate::secrets::fetch_secrets;
use crate::symlink;
use crate::traits::{DynamoOps, Executor, S3Ops, SecretsOps};
use crate::tree;
use crate::types::{CycleOutcome, LastRunStatus};

pub async fn run_cycle(
    config: &Config,
    hostname: &str,
    s3: &dyn S3Ops,
    dynamo: Option<&dyn DynamoOps>,
    secrets: &dyn SecretsOps,
    executor: &dyn Executor,
    var_dir: &Path,
    current_path: &Path,
    nixos_dir: &Path,
    secrets_dir: &Path,
) -> CycleOutcome {
    info!("poll cycle started for hostname={}", hostname);

    let commits_dir = var_dir.join("commits");
    if let Err(e) = std::fs::create_dir_all(&commits_dir) {
        error!("failed to create commits dir: {}", e);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, "", "", executor).await;
        return CycleOutcome::S3Error;
    }

    let hw_path = nixos_dir.join("hardware-configuration.nix");
    let nixos_config_path = nixos_dir.join("configuration.nix");
    let lock_path = var_dir.join(".lock");

    // Cross-process serialization: prevents daemon + single-shot from racing.
    let _lock = match acquire_lock(&lock_path) {
        Ok(l) => l,
        Err(e) => {
            error!("failed to acquire lock: {}", e);
            return CycleOutcome::S3Error;
        }
    };

    // Clean up any orphaned .tmp-* dirs from a crashed prior run.
    tree::cleanup_tmp_dirs(&commits_dir);

    let applied = symlink::read_current(current_path).unwrap_or_default();

    // Step 1: Resolve pointer
    let target = match resolve_pointer(s3).await {
        Ok(t) => t,
        Err(e) => {
            error!("failed to resolve pointer: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, "", executor).await;
            return CycleOutcome::S3Error;
        }
    };

    // Step 2: No-op if symlink already at target
    if applied == target {
        debug!("target sha unchanged ({}) — no-op", target);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Ok, &applied, &target, executor).await;
        return CycleOutcome::NoOp;
    }

    // Step 3: Canary gate — role-scoped, per-commit
    match check_canary(s3, &target, &config.role, hostname).await {
        Ok(true) => {}
        Ok(false) => {
            info!("canary active, hostname not listed — skipping");
            write_heartbeat(config, hostname, dynamo, LastRunStatus::CanarySkip, &applied, &target, executor).await;
            return CycleOutcome::CanarySkip;
        }
        Err(e) => {
            error!("canary check failed: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, executor).await;
            return CycleOutcome::S3Error;
        }
    }

    // Step 4: Ensure the commit tree is local
    if let Err(e) = tree::ensure_local(s3, &commits_dir, &target).await {
        error!("s3 tree fetch failed: {}", e);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, executor).await;
        return CycleOutcome::S3Error;
    }
    let tree_dir = commits_dir.join(&target);

    // Step 5: Ensure hardware config exists
    if !hw_path.exists() {
        info!("hardware-configuration.nix absent — running nixos-generate-config");
        match executor.run("nixos-generate-config", &[]) {
            Ok((0, _)) => info!("nixos-generate-config succeeded"),
            Ok((code, out)) => error!("nixos-generate-config failed (exit {}): {}", code, out),
            Err(e) => error!("nixos-generate-config failed to launch: {}", e),
        }
    }

    // Step 6: Inventory queries (if enabled)
    if config.inventory.enabled {
        if let (Some(dynamo_ref), Some(table)) = (dynamo, &config.inventory.table) {
            let query_contents = collect_queries_from_tree(&tree_dir, &config.role, hostname);
            let query_lists: Vec<_> = query_contents
                .iter()
                .filter_map(|content| parse_queries(content).ok())
                .collect();
            let queries = merge_queries(&query_lists);
            let query_results = run_queries(&queries, dynamo_ref, table).await;
            let inv_path = var_dir.join("inventory.json");
            if let Err(e) = write_inventory(&query_results, &inv_path) {
                error!("failed to write inventory.json: {}", e);
            }
        }
    }

    // Step 7: Fetch secrets
    if let Err(e) = fetch_secrets(secrets, &config.role, hostname, secrets_dir).await {
        error!("secrets fetch error: {}", e);
    }

    // Step 8: Generate configuration.nix at the standard NixOS location
    let has_hw_config = hw_path.exists();
    let host_arg = if tree_dir
        .join(format!("roles/{}/{}/main.nix", config.role, hostname))
        .exists()
    {
        Some(hostname)
    } else {
        None
    };
    let config_nix = generate_configuration_nix(&tree_dir, &config.role, host_arg, has_hw_config);
    if let Some(parent) = nixos_config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&nixos_config_path, &config_nix) {
        error!("failed to write {}: {}", nixos_config_path.display(), e);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, executor).await;
        return CycleOutcome::S3Error;
    }

    // Step 9: Run nixos-rebuild. We write configuration.nix to the standard
    // NixOS location AND pass it explicitly via -I nixos-config=. The explicit
    // flag is required because systemd strips NIX_PATH — without it,
    // nixos-rebuild fails with "file 'nixos-config' was not found in the Nix
    // search path". The standard location still helps manual debugging: a
    // user can `sudo nixos-rebuild switch` from a shell (where NIX_PATH is
    // set) without knowing the file location.
    let nixos_config_arg = format!("nixos-config={}", nixos_config_path.display());
    let nixops3_arg = format!("nixops3={}", tree_dir.display());
    let mut args: Vec<&str> = vec!["switch", "-I", &nixos_config_arg, "-I", &nixops3_arg];
    let nixpkgs_arg;
    if let Some(nixpkgs) = find_nixpkgs() {
        nixpkgs_arg = format!("nixpkgs={nixpkgs}");
        args.push("-I");
        args.push(&nixpkgs_arg);
    }
    info!("running nixos-rebuild switch");

    match executor.run("nixos-rebuild", &args) {
        Ok((0, _)) => {
            info!("apply succeeded");
            // Step 10: advance the symlink atomically
            if let Err(e) = symlink::advance(current_path, &target) {
                error!("failed to advance symlink: {}", e);
                write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, executor).await;
                return CycleOutcome::RebuildFailed;
            }
            info!("symlink advanced: current -> commits/{}", target);
            tree::prune(&commits_dir, config.trees_retain, &target);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Ok, &target, &target, executor).await;
            CycleOutcome::Applied
        }
        Ok((code, output)) => {
            error!("apply failed (exit {}): {}", code, output);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, executor).await;
            CycleOutcome::RebuildFailed
        }
        Err(e) => {
            error!("apply failed to launch: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, executor).await;
            CycleOutcome::RebuildFailed
        }
    }
}

/// GET `current` from S3, validate as 40-char hex sha.
async fn resolve_pointer(s3: &dyn S3Ops) -> Result<String> {
    let bytes = s3
        .get_object(POINTER_KEY)
        .await?
        .ok_or_else(|| anyhow::anyhow!("`current` pointer not found in bucket"))?;
    let raw = String::from_utf8(bytes)?;
    pointer::parse(&raw)
}

/// Read queries.toml files from the local commit tree.
fn collect_queries_from_tree(tree_dir: &Path, role: &str, hostname: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Role-level (rebuild path relative to tree_dir root)
    let role_qt = tree_dir.join(commit_role_queries_relpath(role));
    if let Ok(s) = std::fs::read_to_string(&role_qt) {
        out.push(s);
    }
    // Host-level
    let host_qt = tree_dir.join(commit_host_queries_relpath(role, hostname));
    if let Ok(s) = std::fs::read_to_string(&host_qt) {
        out.push(s);
    }
    out
}

fn commit_role_queries_relpath(role: &str) -> String {
    format!("roles/{role}/queries.toml")
}

fn commit_host_queries_relpath(role: &str, hostname: &str) -> String {
    format!("roles/{role}/{hostname}/queries.toml")
}

/// Acquire the process-serialization lock. Blocking (blocks until acquired).
fn acquire_lock(lock_path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    f.lock_exclusive()?;
    Ok(f)
}

/// Finds the nixpkgs path so it can be passed as `-I nixpkgs=...` to nixos-rebuild.
/// This is necessary because sudo strips NIX_PATH and root's nix channels may not
/// be set up on freshly provisioned machines.
fn find_nixpkgs() -> Option<String> {
    // 1. Current environment — fastest path when NIX_PATH is already set
    if let Ok(nix_path) = std::env::var("NIX_PATH") {
        for part in nix_path.split(':') {
            if let Some(path) = part.strip_prefix("nixpkgs=") {
                if Path::new(path).exists() {
                    return Some(path.to_string());
                }
            }
        }
    }

    // 2. /etc/set-environment — generated by NixOS activation, present even without a login shell
    if let Ok(content) = std::fs::read_to_string("/etc/set-environment") {
        for line in content.lines() {
            let line = line.trim_start_matches("export ");
            if let Some(val) = line.strip_prefix("NIX_PATH=") {
                for part in val.split(':') {
                    if let Some(path) = part.strip_prefix("nixpkgs=") {
                        if Path::new(path).exists() {
                            return Some(path.to_string());
                        }
                    }
                }
            }
        }
    }

    // 3. Standard channel paths for root
    for path in &[
        "/nix/var/nix/profiles/per-user/root/channels/nixos",
        "/nix/var/nix/profiles/per-user/root/channels/nixpkgs",
    ] {
        if Path::new(path).exists() {
            return Some(path.to_string());
        }
    }

    None
}
