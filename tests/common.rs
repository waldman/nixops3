use anyhow::Result;
use async_trait::async_trait;
use nixops3d::config::Config;
use nixops3d::daemon::run_cycle;
use nixops3d::hash::compute_hash;
use nixops3d::traits::{DynamoOps, Executor, S3Ops, SecretsOps};
use nixops3d::types::{CycleOutcome, DynVal, InventoryItem};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

// ─── MockS3 ──────────────────────────────────────────────────────────────────

pub struct MockS3 {
    pub files: HashMap<String, Vec<u8>>,
    pub fail_get: bool,
}

impl MockS3 {
    pub fn new(files: HashMap<String, Vec<u8>>) -> Self {
        Self { files, fail_get: false }
    }

    pub fn with_get_error(mut self) -> Self {
        self.fail_get = true;
        self
    }
}

#[async_trait]
impl S3Ops for MockS3 {
    async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if self.fail_get {
            anyhow::bail!("mock S3 get error");
        }
        Ok(self.files.get(key).cloned())
    }

    async fn list_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self.files.keys().filter(|k| k.starts_with(prefix)).cloned().collect())
    }
}

// ─── MockDynamo ──────────────────────────────────────────────────────────────

pub struct MockDynamo {
    pub calls: Arc<Mutex<Vec<HashMap<String, DynVal>>>>,
    pub error: bool,
    pub scan_items: Vec<InventoryItem>,
}

#[async_trait]
impl DynamoOps for MockDynamo {
    async fn put_item(&self, _table: &str, item: HashMap<String, DynVal>) -> Result<()> {
        if self.error {
            anyhow::bail!("mock DynamoDB error");
        }
        self.calls.lock().unwrap().push(item);
        Ok(())
    }

    async fn scan_role_prefix(&self, _table: &str, _prefix: &str) -> Result<Vec<InventoryItem>> {
        if self.error {
            anyhow::bail!("mock DynamoDB error");
        }
        Ok(self.scan_items.clone())
    }
}

// ─── MockSecrets ─────────────────────────────────────────────────────────────

pub struct MockSecrets {
    pub secrets: HashMap<String, String>,
}

#[async_trait]
impl SecretsOps for MockSecrets {
    async fn list_secrets(&self, path_prefix: &str) -> Result<Vec<String>> {
        Ok(self.secrets.keys().filter(|k| k.starts_with(path_prefix)).cloned().collect())
    }

    async fn get_secret_value(&self, name: &str) -> Result<String> {
        self.secrets
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("secret not found: {}", name))
    }
}

// ─── MockExecutor ────────────────────────────────────────────────────────────

pub struct MockExecutor {
    pub calls: Arc<Mutex<Vec<Vec<String>>>>,
    pub exit_code: i32,
}

impl Executor for MockExecutor {
    fn run(&self, cmd: &str, args: &[&str]) -> Result<(i32, String)> {
        let mut call = vec![cmd.to_string()];
        call.extend(args.iter().map(|s| s.to_string()));
        self.calls.lock().unwrap().push(call);
        Ok((self.exit_code, String::new()))
    }
}

// ─── TestContext ─────────────────────────────────────────────────────────────

pub struct TestContext {
    pub config: Config,
    pub hostname: String,
    pub s3: Arc<MockS3>,
    pub dynamo: Arc<MockDynamo>,
    pub secrets: Arc<MockSecrets>,
    pub executor: Arc<MockExecutor>,
    pub work_dir: TempDir,
    pub hash_dir: TempDir,
    pub secrets_dir: TempDir,
}

impl TestContext {
    pub fn builder() -> TestContextBuilder {
        TestContextBuilder::default()
    }

    pub async fn run_cycle(&self) -> CycleOutcome {
        let hash_path = self.hash_dir.path().join("last-hash");
        run_cycle(
            &self.config,
            &self.hostname,
            self.s3.as_ref(),
            Some(self.dynamo.as_ref()),
            self.secrets.as_ref(),
            self.executor.as_ref(),
            self.work_dir.path(),
            &hash_path,
            self.secrets_dir.path(),
        )
        .await
    }

    pub fn rebuild_was_called(&self) -> bool {
        self.rebuild_call_count() > 0
    }

    pub fn rebuild_call_count(&self) -> usize {
        self.executor
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.first().map(|s| s == "nixos-rebuild").unwrap_or(false))
            .count()
    }

    pub fn last_hash(&self) -> String {
        let path = self.hash_dir.path().join("last-hash");
        std::fs::read_to_string(path).unwrap_or_default()
    }

    pub fn computed_hash(&self) -> String {
        let mut files: Vec<(String, Vec<u8>)> = self
            .s3
            .files
            .iter()
            .filter(|(k, _)| k.ends_with(".nix"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        compute_hash(&files)
    }

    pub fn dynamo_status(&self) -> String {
        let calls = self.dynamo.calls.lock().unwrap();
        calls
            .last()
            .and_then(|m| m.get("last_run_status"))
            .and_then(|v| if let DynVal::S(s) = v { Some(s.clone()) } else { None })
            .unwrap_or_default()
    }

    pub fn dynamo_put_count(&self) -> usize {
        self.dynamo.calls.lock().unwrap().len()
    }

    pub fn work_dir_file(&self, rel_path: &str) -> Option<String> {
        let path = self.work_dir.path().join(rel_path);
        std::fs::read_to_string(path).ok()
    }

    pub fn secret_file(&self, name: &str) -> Option<String> {
        let path = self.secrets_dir.path().join(name);
        std::fs::read_to_string(path).ok()
    }
}

// ─── TestContextBuilder ──────────────────────────────────────────────────────

#[derive(Default)]
pub struct TestContextBuilder {
    s3_files: HashMap<String, Vec<u8>>,
    last_hash: Option<String>,
    rebuild_exit_code: i32,
    dynamo_error: bool,
    secrets: HashMap<String, String>,
    inventory_enabled: bool,
    inventory_items: Vec<InventoryItem>,
    hostname: String,
    s3_get_error: bool,
}

impl TestContextBuilder {
    pub fn s3_file(mut self, key: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        self.s3_files.insert(key.into(), content.into());
        self
    }

    pub fn last_hash(mut self, h: impl Into<String>) -> Self {
        self.last_hash = Some(h.into());
        self
    }

    pub fn rebuild_exit_code(mut self, code: i32) -> Self {
        self.rebuild_exit_code = code;
        self
    }

    pub fn dynamo_error(mut self) -> Self {
        self.dynamo_error = true;
        self
    }

    pub fn secret(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.secrets.insert(name.into(), value.into());
        self
    }

    pub fn hostname(mut self, h: impl Into<String>) -> Self {
        self.hostname = h.into();
        self
    }

    pub fn inventory_enabled(mut self) -> Self {
        self.inventory_enabled = true;
        self
    }

    pub fn inventory_items(mut self, items: Vec<InventoryItem>) -> Self {
        self.inventory_items = items;
        self
    }

    /// Makes all S3 get_object calls return an error (simulates S3 outage)
    pub fn s3_get_error(mut self) -> Self {
        self.s3_get_error = true;
        self
    }

    pub fn build(self) -> TestContext {
        let hostname = if self.hostname.is_empty() {
            "ada-01.waldman.internal".to_string()
        } else {
            self.hostname
        };

        let config_toml = format!(
            r#"bucket = "test-bucket"
region = "us-east-1"
role = "home/production/ada"
poll_interval_secs = 600
{}
"#,
            if self.inventory_enabled {
                "[inventory]\nenabled = true\ntable = \"test-table\""
            } else {
                ""
            }
        );
        let config = Config::from_toml(&config_toml).unwrap();

        let s3 = Arc::new(if self.s3_get_error {
            MockS3::new(self.s3_files).with_get_error()
        } else {
            MockS3::new(self.s3_files)
        });

        let dynamo = Arc::new(MockDynamo {
            calls: Arc::new(Mutex::new(vec![])),
            error: self.dynamo_error,
            scan_items: self.inventory_items,
        });

        let secrets = Arc::new(MockSecrets { secrets: self.secrets });

        let executor = Arc::new(MockExecutor {
            calls: Arc::new(Mutex::new(vec![])),
            exit_code: self.rebuild_exit_code,
        });

        let work_dir = TempDir::new().unwrap();
        let hash_dir = TempDir::new().unwrap();
        let secrets_dir = TempDir::new().unwrap();

        if let Some(h) = self.last_hash {
            std::fs::write(hash_dir.path().join("last-hash"), h).unwrap();
        }

        TestContext { config, hostname, s3, dynamo, secrets, executor, work_dir, hash_dir, secrets_dir }
    }
}
