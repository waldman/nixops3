use anyhow::Result;
use fs2::FileExt;
use std::path::Path;
use tracing::{debug, error, info};

use crate::canary::check_canary;
use crate::config::Config;
use crate::inventory::{run_queries, write_heartbeat, write_inventory, PinInfo};
use crate::main_yaml::MainYaml;
use crate::nixgen::generate_configuration_nix;
use crate::paths::POINTER_KEY;
use crate::pin::{self, nixpkgs_arg_url, ChannelResolver, PinResolution};
use crate::pointer;
use crate::queries::Query;
use crate::secrets::fetch_secrets;
use crate::symlink;
use crate::traits::{DynamoOps, Executor, S3Ops, SecretsOps};
use crate::tree;
use crate::types::{CycleOutcome, LastRunStatus};

#[allow(clippy::too_many_arguments)]
pub async fn run_cycle(
    config: &Config,
    hostname: &str,
    s3: &dyn S3Ops,
    dynamo: Option<&dyn DynamoOps>,
    secrets: &dyn SecretsOps,
    executor: &dyn Executor,
    resolver: &dyn ChannelResolver,
    var_dir: &Path,
    current_path: &Path,
    nixos_dir: &Path,
    secrets_dir: &Path,
) -> CycleOutcome {
    info!("poll cycle started for hostname={}", hostname);

    let commits_dir = var_dir.join("commits");
    if let Err(e) = std::fs::create_dir_all(&commits_dir) {
        error!("failed to create commits dir: {}", e);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, "", "", PinInfo::none(), executor).await;
        return CycleOutcome::S3Error;
    }

    let hw_path = nixos_dir.join("hardware-configuration.nix");
    let nixos_config_path = nixos_dir.join("configuration.nix");
    let lock_path = var_dir.join(".lock");

    let _lock = match acquire_lock(&lock_path) {
        Ok(l) => l,
        Err(e) => {
            error!("failed to acquire lock: {}", e);
            return CycleOutcome::S3Error;
        }
    };

    tree::cleanup_tmp_dirs(&commits_dir);

    let applied = symlink::read_current(current_path).unwrap_or_default();

    // Step 1: Resolve pointer
    let target = match resolve_pointer(s3).await {
        Ok(t) => t,
        Err(e) => {
            error!("failed to resolve pointer: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, "", PinInfo::none(), executor).await;
            return CycleOutcome::S3Error;
        }
    };

    // Step 2: No-op if symlink already at target
    if applied == target {
        debug!("target sha unchanged ({}) — no-op", target);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Ok, &applied, &target, PinInfo::none(), executor).await;
        return CycleOutcome::NoOp;
    }

    // Step 3: Canary gate — role-scoped, per-commit
    match check_canary(s3, &target, &config.role, hostname).await {
        Ok(true) => {}
        Ok(false) => {
            info!("canary active, hostname not listed — skipping");
            write_heartbeat(config, hostname, dynamo, LastRunStatus::CanarySkip, &applied, &target, PinInfo::none(), executor).await;
            return CycleOutcome::CanarySkip;
        }
        Err(e) => {
            error!("canary check failed: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, PinInfo::none(), executor).await;
            return CycleOutcome::S3Error;
        }
    }

    // Step 4: Ensure the commit tree is local
    if let Err(e) = tree::ensure_local(s3, &commits_dir, &target).await {
        error!("s3 tree fetch failed: {}", e);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, PinInfo::none(), executor).await;
        return CycleOutcome::S3Error;
    }
    let tree_dir = commits_dir.join(&target);

    // Step 5: Load main.yaml (merged role + host per spec 08)
    let meta = match load_main_yaml(&tree_dir, &config.role, hostname) {
        Ok(m) => m,
        Err(e) => {
            error!("main.yaml load error: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, PinInfo::none(), executor).await;
            return CycleOutcome::S3Error;
        }
    };

    // Step 6: Resolve pin per spec 09
    let pin_res = match pin::resolve(meta.pin.as_ref(), &config.pins, resolver).await {
        Ok(p) => p,
        Err(e) => {
            error!("pin resolve error: {}", e);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, PinInfo::none(), executor).await;
            return CycleOutcome::S3Error;
        }
    };

    // Nixpkgs source for -I nixpkgs=: URL (Pinned/Floating) or local
    // discovered path (Loose). Nix handles the download itself.
    let nixpkgs_source: Option<String> = nixpkgs_arg_url(&pin_res).or_else(find_nixpkgs);

    // Step 7: Ensure hardware config exists
    if !hw_path.exists() {
        info!("hardware-configuration.nix absent — running nixos-generate-config");
        match executor.run("nixos-generate-config", &[]) {
            Ok((0, _)) => info!("nixos-generate-config succeeded"),
            Ok((code, out)) => error!("nixos-generate-config failed (exit {}): {}", code, out),
            Err(e) => error!("nixos-generate-config failed to launch: {}", e),
        }
    }

    // Step 8: Inventory queries (from meta.queries)
    if config.inventory.enabled {
        if let (Some(dynamo_ref), Some(table), Some(qs)) =
            (dynamo, &config.inventory.table, meta.queries.as_ref())
        {
            let queries: Vec<Query> = qs
                .iter()
                .map(|(name, q)| Query {
                    name: name.clone(),
                    role_prefix: q.role_prefix.clone(),
                })
                .collect();
            let query_results = run_queries(&queries, dynamo_ref, table).await;
            let inv_path = var_dir.join("inventory.json");
            if let Err(e) = write_inventory(&query_results, &inv_path) {
                error!("failed to write inventory.json: {}", e);
            }
        }
    }

    // Step 9: Fetch secrets
    if let Err(e) = fetch_secrets(secrets, &config.role, hostname, secrets_dir).await {
        error!("secrets fetch error: {}", e);
    }

    // Step 10: Generate configuration.nix at the standard NixOS location
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
        let pi = pin_info(&pin_res);
        write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, pi, executor).await;
        return CycleOutcome::S3Error;
    }

    // Step 11: Run nixos-rebuild. -I nixos-config= is passed explicitly
    // (systemd strips NIX_PATH). -I nixpkgs= comes from the pin resolution
    // (Floating/Pinned) or from channel discovery (Loose).
    let nixos_config_arg = format!("nixos-config={}", nixos_config_path.display());
    let nixops3_arg = format!("nixops3={}", tree_dir.display());
    let mut args: Vec<&str> = vec!["switch", "-I", &nixos_config_arg, "-I", &nixops3_arg];
    let nixpkgs_arg_string;
    if let Some(p) = &nixpkgs_source {
        nixpkgs_arg_string = format!("nixpkgs={p}");
        args.push("-I");
        args.push(&nixpkgs_arg_string);
    }
    info!("running nixos-rebuild switch");

    match executor.run("nixos-rebuild", &args) {
        Ok((0, _)) => {
            info!("apply succeeded");
            if let Err(e) = symlink::advance(current_path, &target) {
                error!("failed to advance symlink: {}", e);
                let pi = pin_info(&pin_res);
                write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, pi, executor).await;
                return CycleOutcome::RebuildFailed;
            }
            info!("symlink advanced: current -> commits/{}", target);
            tree::prune(&commits_dir, config.trees_retain, &target);
            let pi = pin_info(&pin_res);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Ok, &target, &target, pi, executor).await;
            CycleOutcome::Applied
        }
        Ok((code, output)) => {
            error!("apply failed (exit {}): {}", code, output);
            let pi = pin_info(&pin_res);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, pi, executor).await;
            CycleOutcome::RebuildFailed
        }
        Err(e) => {
            error!("apply failed to launch: {}", e);
            let pi = pin_info(&pin_res);
            write_heartbeat(config, hostname, dynamo, LastRunStatus::Failed, &applied, &target, pi, executor).await;
            CycleOutcome::RebuildFailed
        }
    }
}

fn pin_info(res: &PinResolution) -> PinInfo<'_> {
    PinInfo {
        mode: res.mode(),
        channel: res.channel().unwrap_or(""),
        rev: res.rev().unwrap_or(""),
    }
}

/// Load role's main.yaml and host's main.yaml from the local tree, then merge.
fn load_main_yaml(tree_dir: &Path, role: &str, hostname: &str) -> Result<MainYaml> {
    let role_dir = tree_dir.join(format!("roles/{role}"));
    let host_dir = role_dir.join(hostname);
    let role_yaml = MainYaml::read_optional(&role_dir)?;
    let host_yaml = MainYaml::read_optional(&host_dir)?;
    Ok(MainYaml::merge(role_yaml, host_yaml))
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

/// Fallback nixpkgs discovery for the Loose tier (no pin).
fn find_nixpkgs() -> Option<String> {
    if let Ok(nix_path) = std::env::var("NIX_PATH") {
        for part in nix_path.split(':') {
            if let Some(path) = part.strip_prefix("nixpkgs=") {
                if Path::new(path).exists() {
                    return Some(path.to_string());
                }
            }
        }
    }
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
