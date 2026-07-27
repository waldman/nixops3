use anyhow::{anyhow, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
    bucket: String,
    region: String,
    role: String,
    poll_interval_secs: Option<u64>,
    aws: Option<AwsCredentials>,
    inventory: Option<InventoryConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InventoryConfig {
    pub enabled: Option<bool>,
    pub table: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bucket: String,
    pub region: String,
    pub role: String,
    pub poll_interval_secs: u64,
    pub aws: Option<AwsCredentials>,
    pub inventory: ResolvedInventory,
}

#[derive(Debug, Clone)]
pub struct ResolvedInventory {
    pub enabled: bool,
    pub table: Option<String>,
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(s)?;

        let poll_interval_secs = raw.poll_interval_secs.unwrap_or(600);
        if poll_interval_secs == 0 {
            return Err(anyhow!("poll_interval_secs must be > 0"));
        }

        let inventory = match raw.inventory {
            None => ResolvedInventory { enabled: false, table: None },
            Some(inv) => {
                let enabled = inv.enabled.unwrap_or(false);
                if enabled && inv.table.is_none() {
                    return Err(anyhow!("inventory.table is required when inventory.enabled = true"));
                }
                ResolvedInventory { enabled, table: inv.table }
            }
        };

        Ok(Config {
            bucket: raw.bucket,
            region: raw.region,
            role: raw.role,
            poll_interval_secs,
            aws: raw.aws,
            inventory,
        })
    }

    pub fn from_file(path: &str) -> Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Self::from_toml(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> &'static str {
        r#"
bucket = "nixops3-waldman"
region = "us-east-1"
role   = "home/production/ada"
"#
    }

    #[test]
    fn test_1_1_valid_full_config() {
        let toml = r#"
bucket = "nixops3-waldman"
region = "us-east-1"
role   = "home/production/ada"
poll_interval_secs = 300

[aws]
access_key_id     = "AKIAIOSFODNN7EXAMPLE"
secret_access_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"

[inventory]
enabled = true
table   = "nixops3-inventory"
"#;
        let cfg = Config::from_toml(toml).unwrap();
        assert_eq!(cfg.bucket, "nixops3-waldman");
        assert_eq!(cfg.region, "us-east-1");
        assert_eq!(cfg.role, "home/production/ada");
        assert_eq!(cfg.poll_interval_secs, 300);
        assert!(cfg.aws.is_some());
        assert!(cfg.inventory.enabled);
        assert_eq!(cfg.inventory.table.as_deref(), Some("nixops3-inventory"));
    }

    #[test]
    fn test_1_2_valid_minimal_config() {
        let cfg = Config::from_toml(minimal()).unwrap();
        assert_eq!(cfg.poll_interval_secs, 600);
        assert!(!cfg.inventory.enabled);
        assert!(cfg.aws.is_none());
    }

    #[test]
    fn test_1_3_missing_bucket() {
        let toml = "region = \"us-east-1\"\nrole = \"home/production/ada\"";
        assert!(Config::from_toml(toml).is_err());
    }

    #[test]
    fn test_1_4_missing_region() {
        let toml = "bucket = \"b\"\nrole = \"home/production/ada\"";
        assert!(Config::from_toml(toml).is_err());
    }

    #[test]
    fn test_1_5_missing_role() {
        let toml = "bucket = \"b\"\nregion = \"us-east-1\"";
        assert!(Config::from_toml(toml).is_err());
    }

    #[test]
    fn test_1_6_poll_interval_zero() {
        let toml = format!("{}\npoll_interval_secs = 0", minimal());
        let err = Config::from_toml(&toml).unwrap_err();
        assert!(err.to_string().contains("poll_interval_secs"));
    }

    #[test]
    fn test_1_7_poll_interval_negative() {
        let toml = format!("{}\npoll_interval_secs = -1", minimal());
        assert!(Config::from_toml(&toml).is_err());
    }

    #[test]
    fn test_1_8_inventory_disabled_by_default() {
        let cfg = Config::from_toml(minimal()).unwrap();
        assert!(!cfg.inventory.enabled);
    }

    #[test]
    fn test_1_9_inventory_enabled_explicitly() {
        let toml = format!("{}\n[inventory]\nenabled = true\ntable = \"my-table\"", minimal());
        let cfg = Config::from_toml(&toml).unwrap();
        assert!(cfg.inventory.enabled);
        assert_eq!(cfg.inventory.table.as_deref(), Some("my-table"));
    }

    #[test]
    fn test_1_10_inventory_enabled_without_table() {
        let toml = format!("{}\n[inventory]\nenabled = true", minimal());
        assert!(Config::from_toml(&toml).is_err());
    }

    #[test]
    fn test_1_11_aws_credentials_optional() {
        let cfg = Config::from_toml(minimal()).unwrap();
        assert!(cfg.aws.is_none());
    }
}
