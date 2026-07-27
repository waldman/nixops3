use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
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
    // Initialise tracing — try journald, fall back to stderr
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let config = Config::from_file(CONFIG_PATH)?;

    let hostname = get_fqdn()?;
    info!("starting nixops3d, hostname={}, role={}", hostname, config.role);

    let work_dir = Path::new(WORK_DIR);
    let hash_path = Path::new(HASH_PATH);
    let secrets_dir = Path::new(SECRETS_DIR);

    std::fs::create_dir_all(work_dir)?;

    // Build AWS SDK config
    let sdk_config = build_aws_config(&config).await;

    let s3 = AwsS3Client {
        client: aws_sdk_s3::Client::new(&sdk_config),
        bucket: config.bucket.clone(),
    };
    let dynamo_client = AwsDynamoClient {
        client: aws_sdk_dynamodb::Client::new(&sdk_config),
    };
    let secrets_client = AwsSecretsClient {
        client: aws_sdk_secretsmanager::Client::new(&sdk_config),
    };
    let executor = ProcessExecutor;

    let dynamo: Option<&dyn nixops3d::traits::DynamoOps> = if config.inventory.enabled {
        Some(&dynamo_client)
    } else {
        None
    };

    // First boot: skip sleep if no hash file exists
    let first_boot = !hash_path.exists();

    loop {
        if !first_boot {
            let duration = sleep_duration(config.poll_interval_secs);
            info!("sleeping {:?}", duration);
            sleep(duration).await;
        }

        // Re-read config each cycle so changes take effect without restart
        let cycle_config = match Config::from_file(CONFIG_PATH) {
            Ok(c) => c,
            Err(e) => {
                error!("failed to reload config: {}", e);
                config.clone()
            }
        };

        run_cycle(
            &cycle_config,
            &hostname,
            &s3,
            dynamo,
            &secrets_client,
            &executor,
            work_dir,
            hash_path,
            secrets_dir,
        )
        .await;
    }
}

async fn build_aws_config(config: &Config) -> aws_config::SdkConfig {
    let region = aws_config::Region::new(config.region.clone());
    let builder = aws_config::defaults(BehaviorVersion::latest())
        .region(region);

    if let Some(creds) = &config.aws {
        let static_creds = Credentials::new(
            &creds.access_key_id,
            &creds.secret_access_key,
            None,
            None,
            "nixops3-config",
        );
        builder.credentials_provider(static_creds).load().await
    } else {
        builder.load().await
    }
}

fn get_fqdn() -> Result<String> {
    let output = Command::new("hostname")
        .arg("--fqdn")
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
