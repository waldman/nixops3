use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Query {
    pub name: String,
    pub role_prefix: String,
}

#[derive(Debug, Deserialize)]
struct QueriesFile {
    #[serde(default)]
    query: Vec<Query>,
}

/// Parses a queries.toml file content. Returns empty vec on empty input.
pub fn parse_queries(toml_str: &str) -> Result<Vec<Query>> {
    if toml_str.trim().is_empty() {
        return Ok(vec![]);
    }
    let qf: QueriesFile = toml::from_str(toml_str)?;
    Ok(qf.query)
}

/// Merges multiple query lists. Later entries win on duplicate names.
pub fn merge_queries(lists: &[Vec<Query>]) -> Vec<Query> {
    let mut map: HashMap<String, Query> = HashMap::new();
    for list in lists {
        for q in list {
            map.insert(q.name.clone(), q.clone());
        }
    }
    let mut result: Vec<Query> = map.into_values().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_6_1_valid_single_query() {
        let toml = r#"
[[query]]
name        = "zk_nodes"
role_prefix = "home/production/zookeeper"
"#;
        let queries = parse_queries(toml).unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].name, "zk_nodes");
        assert_eq!(queries[0].role_prefix, "home/production/zookeeper");
    }

    #[test]
    fn test_6_2_valid_multiple_queries() {
        let toml = r#"
[[query]]
name        = "zk_nodes"
role_prefix = "home/production/zookeeper"

[[query]]
name        = "kafka_nodes"
role_prefix = "home/production/kafka"
"#;
        let queries = parse_queries(toml).unwrap();
        assert_eq!(queries.len(), 2);
    }

    #[test]
    fn test_6_3_empty_file() {
        let queries = parse_queries("").unwrap();
        assert!(queries.is_empty());
    }

    #[test]
    fn test_6_4_missing_name_field() {
        let toml = "[[query]]\nrole_prefix = \"home/production/zookeeper\"";
        assert!(parse_queries(toml).is_err());
    }

    #[test]
    fn test_6_5_missing_role_prefix_field() {
        let toml = "[[query]]\nname = \"zk_nodes\"";
        assert!(parse_queries(toml).is_err());
    }

    #[test]
    fn test_6_6_merge_no_duplicates() {
        let a = vec![Query { name: "zk_nodes".into(), role_prefix: "home/production/zookeeper".into() }];
        let b = vec![Query { name: "kafka_nodes".into(), role_prefix: "home/production/kafka".into() }];
        let merged = merge_queries(&[a, b]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_6_7_merge_duplicate_host_wins() {
        let role_queries = vec![Query {
            name: "zk_nodes".into(),
            role_prefix: "home/production/zookeeper".into(),
        }];
        let host_queries = vec![Query {
            name: "zk_nodes".into(),
            role_prefix: "home/staging/zookeeper".into(),
        }];
        let merged = merge_queries(&[role_queries, host_queries]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].role_prefix, "home/staging/zookeeper");
    }
}
