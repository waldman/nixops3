use anyhow::{anyhow, Result};
use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use std::collections::HashMap;

use crate::traits::{DynamoOps, S3Ops, SecretsOps};
use crate::types::{DynVal, InventoryItem};

// ─── S3 ───────────────────────────────────────────────────────────────────────

pub struct AwsS3Client {
    pub client: aws_sdk_s3::Client,
    pub bucket: String,
}

#[async_trait]
impl S3Ops for AwsS3Client {
    async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let result = self.client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await;

        match result {
            Ok(resp) => {
                let bytes = resp.body.collect().await?.into_bytes().to_vec();
                Ok(Some(bytes))
            }
            Err(SdkError::ServiceError(e)) if matches!(e.err(), GetObjectError::NoSuchKey(_)) => {
                Ok(None)
            }
            Err(e) => Err(anyhow!("S3 GetObject failed for key '{}': {}", key, e)),
        }
    }

    async fn list_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut req = self.client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix);
            if let Some(token) = continuation_token.take() {
                req = req.continuation_token(token);
            }
            let resp = req.send().await
                .map_err(|e| anyhow!("S3 ListObjectsV2 failed: {}", e))?;

            for obj in resp.contents() {
                if let Some(k) = obj.key() {
                    keys.push(k.to_string());
                }
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation_token = resp.next_continuation_token().map(str::to_string);
            } else {
                break;
            }
        }

        Ok(keys)
    }
}

// ─── DynamoDB ─────────────────────────────────────────────────────────────────

pub struct AwsDynamoClient {
    pub client: aws_sdk_dynamodb::Client,
}

fn dynval_to_attr(v: &DynVal) -> AttributeValue {
    match v {
        DynVal::S(s) => AttributeValue::S(s.clone()),
        DynVal::N(n) => AttributeValue::N(n.clone()),
    }
}

fn attr_to_string(v: &AttributeValue) -> String {
    match v {
        AttributeValue::S(s) => s.clone(),
        AttributeValue::N(n) => n.clone(),
        _ => String::new(),
    }
}

#[async_trait]
impl DynamoOps for AwsDynamoClient {
    async fn put_item(&self, table: &str, item: HashMap<String, DynVal>) -> Result<()> {
        let attrs: HashMap<String, AttributeValue> = item
            .iter()
            .map(|(k, v)| (k.clone(), dynval_to_attr(v)))
            .collect();

        self.client
            .put_item()
            .table_name(table)
            .set_item(Some(attrs))
            .send()
            .await
            .map_err(|e| anyhow!("DynamoDB PutItem failed: {}", e))?;
        Ok(())
    }

    async fn scan_role_prefix(&self, table: &str, prefix: &str) -> Result<Vec<InventoryItem>> {
        let resp = self.client
            .scan()
            .table_name(table)
            .filter_expression("begins_with(#r, :prefix)")
            .expression_attribute_names("#r", "role")
            .expression_attribute_values(":prefix", AttributeValue::S(prefix.to_string()))
            .send()
            .await
            .map_err(|e| anyhow!("DynamoDB Scan failed: {}", e))?;

        let now_epoch = chrono::Utc::now().timestamp();
        let mut items = Vec::new();

        for raw in resp.items() {
            // Filter out expired items
            if let Some(ttl_attr) = raw.get("ttl") {
                let ttl: i64 = attr_to_string(ttl_attr).parse().unwrap_or(0);
                if ttl > 0 && ttl <= now_epoch {
                    continue;
                }
            }

            let hostname = raw.get("hostname").map(attr_to_string).unwrap_or_default();
            let ip = raw.get("ip").map(attr_to_string).unwrap_or_default();
            let role = raw.get("role").map(attr_to_string).unwrap_or_default();
            let last_run_status = raw.get("last_run_status").map(attr_to_string).unwrap_or_default();

            items.push(InventoryItem { hostname, ip, role, last_run_status });
        }

        Ok(items)
    }
}

// ─── Secrets Manager ──────────────────────────────────────────────────────────

pub struct AwsSecretsClient {
    pub client: aws_sdk_secretsmanager::Client,
}

#[async_trait]
impl SecretsOps for AwsSecretsClient {
    async fn list_secrets(&self, path_prefix: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut next_token: Option<String> = None;

        loop {
            let mut req = self.client
                .list_secrets()
                .filters(
                    aws_sdk_secretsmanager::types::Filter::builder()
                        .key(aws_sdk_secretsmanager::types::FilterNameStringType::Name)
                        .values(path_prefix)
                        .build(),
                );
            if let Some(token) = next_token.take() {
                req = req.next_token(token);
            }
            let resp = req.send().await
                .map_err(|e| anyhow!("SecretsManager ListSecrets failed: {}", e))?;

            for s in resp.secret_list() {
                if let Some(name) = s.name() {
                    names.push(name.to_string());
                }
            }

            next_token = resp.next_token().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }

        Ok(names)
    }

    async fn get_secret_value(&self, name: &str) -> Result<String> {
        let resp = self.client
            .get_secret_value()
            .secret_id(name)
            .send()
            .await
            .map_err(|e| anyhow!("SecretsManager GetSecretValue failed for '{}': {}", name, e))?;

        resp.secret_string()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("secret '{}' has no string value (binary secrets not supported)", name))
    }
}
