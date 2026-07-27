use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use tracing::error;
use crate::config::Config;
use crate::queries::Query;
use crate::traits::{DynamoOps, Executor};
use crate::types::{DynVal, LastRunStatus};

/// Runs DynamoDB scan queries and returns a map of query name → items.
pub async fn run_queries(
    queries: &[Query],
    dynamo: &dyn DynamoOps,
    table: &str,
) -> HashMap<String, Value> {
    let mut result = HashMap::new();
    for q in queries {
        match dynamo.scan_role_prefix(table, &q.role_prefix).await {
            Ok(items) => {
                let arr: Vec<Value> = items
                    .iter()
                    .map(|i| {
                        json!({
                            "hostname": i.hostname,
                            "ip": i.ip,
                            "role": i.role,
                            "last_run_status": i.last_run_status,
                        })
                    })
                    .collect();
                result.insert(q.name.clone(), Value::Array(arr));
            }
            Err(e) => {
                error!("DynamoDB scan failed for query '{}': {}", q.name, e);
            }
        }
    }
    result
}

/// Writes inventory.json to the given path.
pub fn write_inventory(queries_result: &HashMap<String, Value>, path: &Path) -> Result<()> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let doc = json!({
        "generated_at": now,
        "queries": queries_result,
    });
    std::fs::write(path, serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}

/// Writes a heartbeat item to DynamoDB.
pub async fn write_heartbeat(
    config: &Config,
    hostname: &str,
    dynamo: Option<&dyn DynamoOps>,
    status: LastRunStatus,
    executor: &dyn Executor,
) {
    let dynamo = match dynamo {
        Some(d) => d,
        None => return,
    };
    if !config.inventory.enabled {
        return;
    }
    let table = match &config.inventory.table {
        Some(t) => t.clone(),
        None => return,
    };

    let machine_id = std::fs::read_to_string("/etc/machine-id")
        .unwrap_or_default()
        .trim()
        .to_string();

    let (iface, ip) = detect_network(executor);
    let network = "unknown".to_string();

    let now = Utc::now();
    let last_seen = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let ttl_duration = config.inventory.ttl_secs.unwrap_or(2 * config.poll_interval_secs) as i64;
    let ttl = now.timestamp() + ttl_duration;

    let mut item: HashMap<String, DynVal> = HashMap::new();
    item.insert("hostname".into(), DynVal::S(hostname.to_string()));
    item.insert("machine_id".into(), DynVal::S(machine_id));
    item.insert("role".into(), DynVal::S(config.role.clone()));
    item.insert("iface".into(), DynVal::S(iface));
    item.insert("ip".into(), DynVal::S(ip));
    item.insert("network".into(), DynVal::S(network));
    item.insert("last_run_status".into(), DynVal::S(status.as_str().to_string()));
    item.insert("last_seen".into(), DynVal::S(last_seen));
    item.insert("ttl".into(), DynVal::N(ttl.to_string()));

    if let Err(e) = dynamo.put_item(&table, item).await {
        error!("DynamoDB heartbeat write failed: {}", e);
    }
}

/// Runs `ip route get 1.1.1.1` and parses iface + ip.
fn detect_network(executor: &dyn Executor) -> (String, String) {
    match executor.run("ip", &["route", "get", "1.1.1.1"]) {
        Ok((_, output)) => parse_ip_route(&output),
        Err(_) => ("unknown".to_string(), "unknown".to_string()),
    }
}

/// Parses output like: `1.1.1.1 via 192.168.1.1 dev eth0 src 192.168.1.100 uid 0`
fn parse_ip_route(output: &str) -> (String, String) {
    let mut iface = "unknown".to_string();
    let mut ip = "unknown".to_string();
    let tokens: Vec<&str> = output.split_whitespace().collect();
    for i in 0..tokens.len().saturating_sub(1) {
        if tokens[i] == "dev" {
            iface = tokens[i + 1].to_string();
        }
        if tokens[i] == "src" {
            ip = tokens[i + 1].to_string();
        }
    }
    (iface, ip)
}
