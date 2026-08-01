use anyhow::Result;
use std::path::Path;
use tracing::{error, info};
use crate::paths::{secrets_host_prefix, secrets_role_prefix};
use crate::traits::SecretsOps;

/// Fetches secrets from AWS Secrets Manager and writes them to `secrets_dir`.
///
/// Role-level secrets are fetched first; host-level secrets overwrite them on name collision.
pub async fn fetch_secrets(
    secrets: &dyn SecretsOps,
    role: &str,
    hostname: &str,
    secrets_dir: &Path,
) -> Result<()> {
    let role_prefix = secrets_role_prefix(role);
    let host_prefix = secrets_host_prefix(role, hostname);

    let role_names = match secrets.list_secrets(&role_prefix).await {
        Ok(names) => names,
        Err(e) => {
            error!("failed to list role-level secrets: {}", e);
            return Ok(());
        }
    };

    let host_names = match secrets.list_secrets(&host_prefix).await {
        Ok(names) => names,
        Err(e) => {
            error!("failed to list host-level secrets: {}", e);
            vec![]
        }
    };

    // Fetch role-level first, then host-level (overwrites on same short name)
    for full_name in role_names.iter().chain(host_names.iter()) {
        let short_name = extract_secret_name(full_name, &role_prefix, &host_prefix);
        match secrets.get_secret_value(full_name).await {
            Ok(value) => {
                let dest = secrets_dir.join(&short_name);
                if let Err(e) = std::fs::write(&dest, value.as_bytes()) {
                    error!("failed to write secret {}: {}", short_name, e);
                }
            }
            Err(e) => {
                error!("failed to fetch secret {}: {}", full_name, e);
            }
        }
    }

    info!("secrets fetched: {} role, {} host", role_names.len(), host_names.len());
    Ok(())
}

/// Strips the prefix from a secret full name to get its short (file) name.
fn extract_secret_name(full_name: &str, role_prefix: &str, host_prefix: &str) -> String {
    if full_name.starts_with(host_prefix) {
        full_name[host_prefix.len()..].to_string()
    } else if full_name.starts_with(role_prefix) {
        full_name[role_prefix.len()..].to_string()
    } else {
        // Fallback: use last path component
        full_name.split('/').last().unwrap_or(full_name).to_string()
    }
}

#[cfg(test)]
mod tests {
    use crate::paths::{secrets_host_prefix, secrets_role_prefix};

    #[test]
    fn test_7_1_role_level_prefix() {
        assert_eq!(secrets_role_prefix("home/production/app"), "NixOps/home/production/app/shared/");
    }

    #[test]
    fn test_7_2_host_level_prefix() {
        assert_eq!(
            secrets_host_prefix("home/production/app", "app-01.example.internal"),
            "NixOps/home/production/app/app-01.example.internal/"
        );
    }

    #[test]
    fn test_7_3_local_path_for_secret() {
        let dir = std::path::Path::new("/run/nixops3/secrets");
        let path = dir.join("example-api-key");
        assert_eq!(path.to_str().unwrap(), "/run/nixops3/secrets/example-api-key");
    }
}
