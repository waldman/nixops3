use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Merged role+host metadata from `main.yaml`.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainYaml {
    pub pin: Option<Pin>,
    pub queries: Option<BTreeMap<String, Query>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub nixpkgs: NixpkgsPin,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NixpkgsPin {
    pub channel: String,
    pub rev: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub role_prefix: String,
}

impl MainYaml {
    /// Parse a `main.yaml` string. Empty input yields `MainYaml::default()`.
    /// Validates that `rev`, if present, is 40 lowercase hex chars.
    pub fn from_str(s: &str) -> Result<Self> {
        if s.trim().is_empty() {
            return Ok(Self::default());
        }
        let parsed: MainYaml = serde_yaml::from_str(s)
            .map_err(|e| anyhow!("main.yaml parse error: {e}"))?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Read main.yaml from `<dir>/main.yaml`, returning default if missing.
    pub fn read_optional(dir: &Path) -> Result<Self> {
        let path = dir.join("main.yaml");
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::from_str(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow!("failed to read {}: {}", path.display(), e)),
        }
    }

    /// Merge two layers per spec 08: per-top-level-key, most-specific-wins.
    /// `overlay`'s block entirely replaces `base`'s when set (no deep merge).
    pub fn merge(base: MainYaml, overlay: MainYaml) -> MainYaml {
        MainYaml {
            pin: overlay.pin.or(base.pin),
            queries: overlay.queries.or(base.queries),
        }
    }

    /// Merge a chain of layers, most-general first. Later layers override
    /// earlier ones per top-level key. `layer_all([fleet, role, host])`
    /// produces the effective config for one host.
    pub fn layer_all(layers: Vec<MainYaml>) -> MainYaml {
        layers.into_iter().fold(MainYaml::default(), Self::merge)
    }

    fn validate(&self) -> Result<()> {
        if let Some(pin) = &self.pin {
            if pin.nixpkgs.channel.is_empty() {
                return Err(anyhow!("pin.nixpkgs.channel must not be empty"));
            }
            if let Some(rev) = &pin.nixpkgs.rev {
                if rev.len() != 40 || !rev.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
                    return Err(anyhow!(
                        "pin.nixpkgs.rev must be 40 lowercase hex chars (got {:?})",
                        rev
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REV: &str = "abcdef1234567890abcdef1234567890abcdef12";

    #[test]
    fn test_6_1_valid_full_config() {
        let yaml = format!(
            r#"
pin:
  nixpkgs:
    channel: nixos-25.05
    rev: {REV}
queries:
  zk_nodes:
    role_prefix: home/production/zookeeper
"#
        );
        let m = MainYaml::from_str(&yaml).unwrap();
        assert!(m.pin.is_some());
        let p = m.pin.unwrap();
        assert_eq!(p.nixpkgs.channel, "nixos-25.05");
        assert_eq!(p.nixpkgs.rev.as_deref(), Some(REV));
        let q = m.queries.unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q["zk_nodes"].role_prefix, "home/production/zookeeper");
    }

    #[test]
    fn test_6_2_empty_file() {
        let m = MainYaml::from_str("").unwrap();
        assert!(m.pin.is_none());
        assert!(m.queries.is_none());
        let m2 = MainYaml::from_str("{}").unwrap();
        assert!(m2.pin.is_none());
    }

    #[test]
    fn test_6_3_pin_without_channel() {
        let yaml = format!("pin:\n  nixpkgs:\n    rev: {REV}\n");
        assert!(MainYaml::from_str(&yaml).is_err());
    }

    #[test]
    fn test_6_4_pin_channel_only() {
        let m = MainYaml::from_str("pin:\n  nixpkgs:\n    channel: nixos-25.05\n").unwrap();
        assert_eq!(m.pin.unwrap().nixpkgs.rev, None);
    }

    #[test]
    fn test_6_6_rev_must_be_40_hex() {
        assert!(MainYaml::from_str("pin:\n  nixpkgs:\n    channel: c\n    rev: abc\n").is_err());
        let bad_upper = "ABCDEF1234567890ABCDEF1234567890ABCDEF12";
        let yaml = format!("pin:\n  nixpkgs:\n    channel: c\n    rev: {bad_upper}\n");
        assert!(MainYaml::from_str(&yaml).is_err());
    }

    #[test]
    fn test_6_7_queries_as_map() {
        let yaml = r#"
queries:
  zk_nodes: { role_prefix: home/production/zookeeper }
"#;
        let m = MainYaml::from_str(yaml).unwrap();
        assert_eq!(m.queries.unwrap()["zk_nodes"].role_prefix, "home/production/zookeeper");
    }

    #[test]
    fn test_6_8_queries_missing_role_prefix() {
        let yaml = "queries:\n  zk_nodes: {}\n";
        assert!(MainYaml::from_str(yaml).is_err());
    }

    #[test]
    fn test_6_9_merge_host_replaces_pin() {
        let role = MainYaml::from_str(&format!(
            "pin: {{ nixpkgs: {{ channel: A, rev: {REV} }} }}"
        ))
        .unwrap();
        let host = MainYaml::from_str("pin: { nixpkgs: { channel: B } }").unwrap();
        let merged = MainYaml::merge(role, host);
        let p = merged.pin.unwrap();
        assert_eq!(p.nixpkgs.channel, "B");
        assert_eq!(p.nixpkgs.rev, None, "role's rev not inherited");
    }

    #[test]
    fn test_6_10_merge_host_omits_pin() {
        let role = MainYaml::from_str(&format!(
            "pin: {{ nixpkgs: {{ channel: A, rev: {REV} }} }}"
        ))
        .unwrap();
        let host = MainYaml::default();
        let merged = MainYaml::merge(role, host);
        assert_eq!(merged.pin.unwrap().nixpkgs.channel, "A");
    }

    #[test]
    fn test_6_11_merge_host_replaces_queries() {
        let role = MainYaml::from_str(
            "queries:\n  a: { role_prefix: pa }\n  b: { role_prefix: pb }\n",
        )
        .unwrap();
        let host = MainYaml::from_str("queries:\n  c: { role_prefix: pc }\n").unwrap();
        let merged = MainYaml::merge(role, host);
        let q = merged.queries.unwrap();
        assert_eq!(q.len(), 1);
        assert!(q.contains_key("c"));
    }

    #[test]
    fn test_6_13_all_absent() {
        let merged = MainYaml::layer_all(vec![
            MainYaml::default(),
            MainYaml::default(),
            MainYaml::default(),
        ]);
        assert!(merged.pin.is_none());
        assert!(merged.queries.is_none());
    }

    #[test]
    fn test_6_14_fleet_only_applies() {
        let fleet = MainYaml::from_str(&format!(
            "pin: {{ nixpkgs: {{ channel: F, rev: {REV} }} }}"
        ))
        .unwrap();
        let merged = MainYaml::layer_all(vec![fleet, MainYaml::default(), MainYaml::default()]);
        let p = merged.pin.unwrap();
        assert_eq!(p.nixpkgs.channel, "F");
        assert_eq!(p.nixpkgs.rev.as_deref(), Some(REV));
    }

    #[test]
    fn test_6_15_role_overrides_fleet() {
        let fleet = MainYaml::from_str("pin: { nixpkgs: { channel: F } }").unwrap();
        let role = MainYaml::from_str("pin: { nixpkgs: { channel: R } }").unwrap();
        let merged = MainYaml::layer_all(vec![fleet, role, MainYaml::default()]);
        assert_eq!(merged.pin.unwrap().nixpkgs.channel, "R");
    }

    #[test]
    fn test_6_16_host_overrides_both() {
        let fleet = MainYaml::from_str("pin: { nixpkgs: { channel: F } }").unwrap();
        let role = MainYaml::from_str("pin: { nixpkgs: { channel: R } }").unwrap();
        let host = MainYaml::from_str("pin: { nixpkgs: { channel: H } }").unwrap();
        let merged = MainYaml::layer_all(vec![fleet, role, host]);
        assert_eq!(merged.pin.unwrap().nixpkgs.channel, "H");
    }

    #[test]
    fn test_6_17_independent_keys() {
        let fleet = MainYaml::from_str("pin: { nixpkgs: { channel: F } }").unwrap();
        let role = MainYaml::from_str("queries:\n  q1: { role_prefix: p }\n").unwrap();
        let merged = MainYaml::layer_all(vec![fleet, role, MainYaml::default()]);
        assert_eq!(merged.pin.unwrap().nixpkgs.channel, "F");
        assert_eq!(merged.queries.unwrap()["q1"].role_prefix, "p");
    }
}
