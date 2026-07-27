use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use crate::types::{DynVal, InventoryItem};

#[async_trait]
pub trait S3Ops: Send + Sync {
    async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn list_prefix(&self, prefix: &str) -> Result<Vec<String>>;
}

#[async_trait]
pub trait DynamoOps: Send + Sync {
    async fn put_item(&self, table: &str, item: HashMap<String, DynVal>) -> Result<()>;
    async fn scan_role_prefix(&self, table: &str, prefix: &str) -> Result<Vec<InventoryItem>>;
}

#[async_trait]
pub trait SecretsOps: Send + Sync {
    async fn list_secrets(&self, path_prefix: &str) -> Result<Vec<String>>;
    async fn get_secret_value(&self, name: &str) -> Result<String>;
}

pub trait Executor: Send + Sync {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<(i32, String)>;
}
