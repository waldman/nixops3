use anyhow::{anyhow, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tokio::time::sleep;
use tracing::{error, info};

use nixops3d::aws::{AwsDynamoClient, AwsS3Client, AwsSecretsClient};
use nixops3d::config::Config;
use nixops3d::daemon::run_cycle;
use nixops3d::executor::ProcessExecutor;
use nixops3d::timer::sleep_duration;

const CONFIG_PATH: &str = "/etc/nixops3/nixops3.toml";
const WORK_DIR: &str = "/var/lib/nixops3/current";
const HASH_PATH: &str = "/run/nixops3/last-hash";
const SECRETS_DIR: &str = "/run/nixops3/secrets";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("bootstrap") => run_bootstrap(&args[2..]).await,
        Some(other) => Err(anyhow!("unknown subcommand: {}", other)),
        None => run_daemon().await,
    }
}

// ── bootstrap ────────────────────────────────────────────────────────────────

async fn run_bootstrap(args: &[String]) -> Result<()> {
    let flags = parse_flags(args);

    let bucket     = flags.get("bucket").cloned().ok_or_else(|| anyhow!("--bucket is required"))?;
    let region     = flags.get("region").cloned().ok_or_else(|| anyhow!("--region is required"))?;
    let role       = flags.get("role").cloned().ok_or_else(|| anyhow!("--role is required"))?;
    let hostname   = flags.get("hostname").cloned();
    let table      = flags.get("table").cloned();
    let ttl_days: Option<u64> = flags.get("ttl-days").and_then(|v| v.parse().ok());
    let access_key = flags.get("access-key-id").cloned();
    let secret_key = flags.get("secret-key").cloned();

    match (&access_key, &secret_key) {
        (Some(_), None) | (None, Some(_)) =>
            return Err(anyhow!("--access-key-id and --secret-key must be provided together")),
        _ => {}
    }

    // Optionally set the system hostname
    if let Some(ref h) = hostname {
        Command::new("hostnamectl")
            .args(["set-hostname", h])
            .status()
            .map_err(|e| anyhow!("hostnamectl failed: {}", e))?;
        info!("hostname set to {}", h);
    }

    // Write config file — nixops3d owns this from here on
    write_toml(&bucket, &region, &role, table.as_deref(), ttl_days,
               access_key.as_deref(), secret_key.as_deref())?;

    let config = Config::from_file(CONFIG_PATH)?;

    let hostname = hostname
        .map(Ok)
        .unwrap_or_else(get_fqdn)?;

    info!("bootstrap: hostname={}, role={}", hostname, config.role);

    // Create runtime dirs (systemd RuntimeDirectory handles this in daemon mode)
    let work_dir    = Path::new(WORK_DIR);
    let hash_path   = Path::new(HASH_PATH);
    let secrets_dir = Path::new(SECRETS_DIR);
    std::fs::create_dir_all(work_dir)?;
    std::fs::create_dir_all(secrets_dir)?;
    std::fs::set_permissions(secrets_dir, std::fs::Permissions::from_mode(0o700))?;

    let (s3, dynamo_client, secrets_client) = build_clients(&config).await;
    let executor = ProcessExecutor;
    let dynamo: Option<&dyn nixops3d::traits::DynamoOps> =
        if config.inventory.enabled { Some(&dynamo_client) } else { None };

    run_cycle(&config, &hostname, &s3, dynamo, &secrets_client,
              &executor, work_dir, hash_path, secrets_dir).await;

    info!("bootstrap complete — nixops3d service will take over");
    Ok(())
}

fn write_toml(
    bucket: &str, region: &str, role: &str,
    table: Option<&str>, ttl_days: Option<u64>,
    access_key: Option<&str>, secret_key: Option<&str>,
) -> Result<()> {
    let dir = Path::new("/etc/nixops3");
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;

    let mut s = format!("bucket = \"{bucket}\"\nregion = \"{region}\"\nrole   = \"{role}\"\n");

    if let (Some(ak), Some(sk)) = (access_key, secret_key) {
        s.push_str(&format!(
            "\n[aws]\naccess_key_id     = \"{ak}\"\nsecret_access_key = \"{sk}\"\n"
        ));
    }

    if let Some(t) = table {
        s.push_str(&format!("\n[inventory]\nenabled = true\ntable   = \"{t}\"\n"));
        if let Some(days) = ttl_days {
            s.push_str(&format!("ttl_secs = {}\n", days * 86400));
        }
    }

    let path = dir.join("nixops3.toml");
    std::fs::write(&path, s)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    info!("config written: {}", path.display());
    Ok(())
}

fn parse_flags(args: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(key) = args[i].strip_prefix("--") {
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                map.insert(key.to_string(), args[i + 1].clone());
                i += 2;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    map
}

// ── daemon ───────────────────────────────────────────────────────────────────

async fn run_daemon() -> Result<()> {
    let config = Config::from_file(CONFIG_PATH)?;
    let hostname = get_fqdn()?;
    info!("starting nixops3d, hostname={}, role={}", hostname, config.role);

    let work_dir    = Path::new(WORK_DIR);
    let hash_path   = Path::new(HASH_PATH);
    let secrets_dir = Path::new(SECRETS_DIR);
    std::fs::create_dir_all(work_dir)?;

    let (s3, dynamo_client, secrets_client) = build_clients(&config).await;
    let executor = ProcessExecutor;
    let dynamo: Option<&dyn nixops3d::traits::DynamoOps> =
        if config.inventory.enabled { Some(&dynamo_client) } else { None };

    let mut first_boot = !hash_path.exists();

    loop {
        if !first_boot {
            let duration = sleep_duration(config.poll_interval_secs);
            info!("sleeping {:?}", duration);
            sleep(duration).await;
        }
        first_boot = false;

        let cycle_config = match Config::from_file(CONFIG_PATH) {
            Ok(c) => c,
            Err(e) => { error!("failed to reload config: {}", e); config.clone() }
        };

        run_cycle(&cycle_config, &hostname, &s3, dynamo, &secrets_client,
                  &executor, work_dir, hash_path, secrets_dir).await;
    }
}

// ── shared helpers ───────────────────────────────────────────────────────────

async fn build_clients(config: &Config)
    -> (AwsS3Client, AwsDynamoClient, AwsSecretsClient)
{
    let region = aws_config::Region::new(config.region.clone());
    let builder = aws_config::defaults(BehaviorVersion::latest()).region(region);

    let sdk_config = if let Some(creds) = &config.aws {
        let static_creds = Credentials::new(
            &creds.access_key_id, &creds.secret_access_key,
            None, None, "nixops3-config",
        );
        builder.credentials_provider(static_creds).load().await
    } else {
        builder.load().await
    };

    (
        AwsS3Client { client: aws_sdk_s3::Client::new(&sdk_config), bucket: config.bucket.clone() },
        AwsDynamoClient { client: aws_sdk_dynamodb::Client::new(&sdk_config) },
        AwsSecretsClient { client: aws_sdk_secretsmanager::Client::new(&sdk_config) },
    )
}

fn get_fqdn() -> Result<String> {
    let output = Command::new("hostname").arg("--fqdn").output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
