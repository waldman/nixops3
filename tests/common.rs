use anyhow::Result;
use async_trait::async_trait;
use nixops3d::config::Config;
use nixops3d::daemon::run_cycle;
use nixops3d::traits::{DynamoOps, Executor, S3Ops, SecretsOps};
use nixops3d::types::{CycleOutcome, DynVal, InventoryItem};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

// Test constants — deterministic 40-char hex shas
pub const TEST_SHA: &str = "abcdef1234567890abcdef1234567890abcdef12";
pub const TEST_SHA_2: &str = "1234567890abcdef1234567890abcdef12345678";
pub const TEST_ROLE: &str = "home/production/webserver";
pub const TEST_HOST: &str = "web-01.waldman.internal";

// ─── MockS3 ──────────────────────────────────────────────────────────────────

pub struct MockS3 {
    pub files: HashMap<String, Vec<u8>>,
    pub fail_get: bool,
    pub fail_get_key: Option<String>,
}

impl MockS3 {
    pub fn new(files: HashMap<String, Vec<u8>>) -> Self {
        Self { files, fail_get: false, fail_get_key: None }
    }
}

#[async_trait]
impl S3Ops for MockS3 {
    async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if self.fail_get {
            anyhow::bail!("mock S3 get error");
        }
        if let Some(ref k) = self.fail_get_key {
            if key == k {
                anyhow::bail!("mock S3 get error for key {}", key);
            }
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
    pub var_dir: TempDir,
    pub nixos_dir: TempDir,
    pub secrets_dir: TempDir,
}

impl TestContext {
    pub fn builder() -> TestContextBuilder {
        TestContextBuilder::default()
    }

    pub async fn run_cycle(&self) -> CycleOutcome {
        let current_path = self.var_dir.path().join("current");
        // Pre-create hardware-configuration.nix stub in nixos_dir so the
        // daemon doesn't try to invoke nixos-generate-config in tests.
        let hw = self.nixos_dir.path().join("hardware-configuration.nix");
        if !hw.exists() {
            std::fs::write(&hw, "{ }").unwrap();
        }
        run_cycle(
            &self.config,
            &self.hostname,
            self.s3.as_ref(),
            Some(self.dynamo.as_ref()),
            self.secrets.as_ref(),
            self.executor.as_ref(),
            self.var_dir.path(),
            &current_path,
            self.nixos_dir.path(),
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

    pub fn symlink_target(&self) -> Option<String> {
        let current = self.var_dir.path().join("current");
        let target = std::fs::read_link(&current).ok()?;
        target.file_name()?.to_str().map(|s| s.to_string())
    }

    pub fn dynamo_last_status(&self) -> String {
        self.dynamo_field("last_run_status")
    }

    pub fn dynamo_last_applied_sha(&self) -> String {
        self.dynamo_field("applied_sha")
    }

    pub fn dynamo_last_target_sha(&self) -> String {
        self.dynamo_field("target_sha")
    }

    fn dynamo_field(&self, name: &str) -> String {
        let calls = self.dynamo.calls.lock().unwrap();
        calls
            .last()
            .and_then(|m| m.get(name))
            .and_then(|v| if let DynVal::S(s) = v { Some(s.clone()) } else { None })
            .unwrap_or_default()
    }

    pub fn dynamo_put_count(&self) -> usize {
        self.dynamo.calls.lock().unwrap().len()
    }

    pub fn nixos_config_file(&self) -> Option<String> {
        std::fs::read_to_string(self.nixos_dir.path().join("configuration.nix")).ok()
    }

    pub fn secret_file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.secrets_dir.path().join(name)).ok()
    }
}

// ─── TestContextBuilder ──────────────────────────────────────────────────────

#[derive(Default)]
pub struct TestContextBuilder {
    s3_pointer: Option<String>,
    s3_files: HashMap<String, Vec<u8>>,
    fail_get_key: Option<String>,
    seed_symlink: Option<String>,
    rebuild_exit_code: i32,
    dynamo_error: bool,
    secrets: HashMap<String, String>,
    inventory_enabled: bool,
    inventory_items: Vec<InventoryItem>,
    hostname: String,
    s3_get_error: bool,
}

impl TestContextBuilder {
    pub fn s3_pointer(mut self, sha: impl Into<String>) -> Self {
        self.s3_pointer = Some(sha.into());
        self
    }

    pub fn s3_file(mut self, key: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        self.s3_files.insert(key.into(), content.into());
        self
    }

    pub fn commit_file(
        self,
        sha: &str,
        key: impl AsRef<str>,
        content: impl Into<Vec<u8>>,
    ) -> Self {
        let full = format!("commits/{}/{}", sha, key.as_ref());
        self.s3_file(full, content)
    }

    pub fn seed_symlink(mut self, sha: impl Into<String>) -> Self {
        self.seed_symlink = Some(sha.into());
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

    #[allow(dead_code)]
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

    pub fn s3_get_error(mut self) -> Self {
        self.s3_get_error = true;
        self
    }

    #[allow(dead_code)]
    pub fn fail_get_key(mut self, key: impl Into<String>) -> Self {
        self.fail_get_key = Some(key.into());
        self
    }

    pub fn build(mut self) -> TestContext {
        let hostname = if self.hostname.is_empty() {
            TEST_HOST.to_string()
        } else {
            self.hostname
        };

        if let Some(sha) = &self.s3_pointer {
            self.s3_files.insert("current".to_string(), sha.as_bytes().to_vec());
        }

        let config_toml = format!(
            r#"bucket = "test-bucket"
region = "us-east-1"
role = "{TEST_ROLE}"
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

        let s3 = Arc::new({
            let mut m = MockS3::new(self.s3_files);
            if self.s3_get_error {
                m.fail_get = true;
            }
            m.fail_get_key = self.fail_get_key;
            m
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

        let var_dir = TempDir::new().unwrap();
        let nixos_dir = TempDir::new().unwrap();
        let secrets_dir = TempDir::new().unwrap();

        std::fs::create_dir_all(var_dir.path().join("commits")).unwrap();

        if let Some(sha) = self.seed_symlink {
            let commits_dir = var_dir.path().join("commits");
            std::fs::create_dir_all(commits_dir.join(&sha)).unwrap();
            std::os::unix::fs::symlink(
                format!("commits/{sha}"),
                var_dir.path().join("current"),
            )
            .unwrap();
        }

        TestContext {
            config,
            hostname,
            s3,
            dynamo,
            secrets,
            executor,
            var_dir,
            nixos_dir,
            secrets_dir,
        }
    }
}
