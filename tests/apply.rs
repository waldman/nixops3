mod common;
use common::{TestContext, TEST_HOST, TEST_ROLE, TEST_SHA, TEST_SHA_2};
use nixops3d::types::{CycleOutcome, InventoryItem};

// Helper: minimal S3 setup — pointer + a role main.nix inside the commit tree.
fn baseline(builder: common::TestContextBuilder) -> common::TestContextBuilder {
    builder
        .s3_pointer(TEST_SHA)
        .commit_file(TEST_SHA, "profiles/base.nix", "{ ... }: {}")
        .commit_file(
            TEST_SHA,
            format!("roles/{TEST_ROLE}/main.nix"),
            "{ ... }: {}",
        )
}

// ─── 12.1 Happy path — first run ─────────────────────────────────────────────

#[tokio::test]
async fn test_12_1_happy_path_first_run() {
    let ctx = baseline(TestContext::builder().inventory_enabled()).build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
    assert_eq!(ctx.symlink_target().as_deref(), Some(TEST_SHA));
    assert_eq!(ctx.dynamo_last_status(), "ok");
    assert_eq!(ctx.dynamo_last_applied_sha(), TEST_SHA);
    assert_eq!(ctx.dynamo_last_target_sha(), TEST_SHA);
    assert!(
        ctx.nixos_config_file().unwrap().contains(TEST_ROLE),
        "configuration.nix written and references role"
    );
}

// ─── 12.2 No-op — symlink already at target ─────────────────────────────────

#[tokio::test]
async fn test_12_2_symlink_already_at_target() {
    let ctx = baseline(TestContext::builder().inventory_enabled())
        .seed_symlink(TEST_SHA)
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::NoOp);
    assert!(!ctx.rebuild_was_called());
    assert_eq!(ctx.symlink_target().as_deref(), Some(TEST_SHA));
    assert_eq!(ctx.dynamo_last_status(), "ok");
    assert_eq!(ctx.dynamo_last_applied_sha(), TEST_SHA);
    assert_eq!(ctx.dynamo_last_target_sha(), TEST_SHA);
}

// ─── 12.3 Apply triggered — pointer flipped ─────────────────────────────────

#[tokio::test]
async fn test_12_3_pointer_flipped() {
    // Symlink at TEST_SHA_2, pointer at TEST_SHA — should apply TEST_SHA
    let ctx = baseline(TestContext::builder().inventory_enabled())
        .seed_symlink(TEST_SHA_2)
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
    assert_eq!(ctx.symlink_target().as_deref(), Some(TEST_SHA));
    assert_eq!(ctx.dynamo_last_applied_sha(), TEST_SHA);
}

// ─── 12.4 Canary skip ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_12_4_canary_skip() {
    let ctx = baseline(TestContext::builder().inventory_enabled())
        .seed_symlink(TEST_SHA_2)
        .commit_file(
            TEST_SHA,
            format!("roles/{TEST_ROLE}/canary.txt"),
            "some-other-host.example.com\n",
        )
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::CanarySkip);
    assert!(!ctx.rebuild_was_called());
    assert_eq!(ctx.symlink_target().as_deref(), Some(TEST_SHA_2));
    assert_eq!(ctx.dynamo_last_status(), "canary_skip");
    assert_eq!(ctx.dynamo_last_applied_sha(), TEST_SHA_2);
    assert_eq!(ctx.dynamo_last_target_sha(), TEST_SHA);
}

// ─── 12.4b Canary listed — apply ─────────────────────────────────────────────

#[tokio::test]
async fn test_12_4b_canary_hostname_listed() {
    let ctx = baseline(TestContext::builder().inventory_enabled())
        .seed_symlink(TEST_SHA_2)
        .commit_file(
            TEST_SHA,
            format!("roles/{TEST_ROLE}/canary.txt"),
            format!("{TEST_HOST}\n"),
        )
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
    assert_eq!(ctx.symlink_target().as_deref(), Some(TEST_SHA));
}

// ─── 12.5 Rebuild failure — no symlink advance ──────────────────────────────

#[tokio::test]
async fn test_12_5_rebuild_failure_symlink_unchanged() {
    let ctx = baseline(TestContext::builder().inventory_enabled())
        .seed_symlink(TEST_SHA_2)
        .rebuild_exit_code(1)
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::RebuildFailed);
    assert!(ctx.rebuild_was_called());
    assert_eq!(
        ctx.symlink_target().as_deref(),
        Some(TEST_SHA_2),
        "symlink must stay at prior sha"
    );
    assert_eq!(ctx.dynamo_last_status(), "failed");
    assert_eq!(ctx.dynamo_last_applied_sha(), TEST_SHA_2);
    assert_eq!(ctx.dynamo_last_target_sha(), TEST_SHA);
}

// ─── 12.6 S3 pointer fetch failure ──────────────────────────────────────────

#[tokio::test]
async fn test_12_6_s3_pointer_failure() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_get_error()
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::S3Error);
    assert!(!ctx.rebuild_was_called());
    assert_eq!(ctx.symlink_target(), None);
    assert_eq!(ctx.dynamo_last_status(), "failed");
    assert_eq!(ctx.dynamo_last_target_sha(), "");
}

// ─── 12.7 Malformed pointer ──────────────────────────────────────────────────

#[tokio::test]
async fn test_12_7_malformed_pointer() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("current", "not-a-sha")
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::S3Error);
    assert!(!ctx.rebuild_was_called());
    assert_eq!(ctx.dynamo_last_status(), "failed");
    assert_eq!(ctx.dynamo_last_target_sha(), "");
}

// ─── 12.9 Host main.nix absent — apply succeeds ─────────────────────────────

#[tokio::test]
async fn test_12_9_no_host_main_nix() {
    let ctx = baseline(TestContext::builder().inventory_enabled()).build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    let config_nix = ctx.nixos_config_file().unwrap();
    assert!(
        !config_nix.contains(&format!("{TEST_HOST}/main.nix")),
        "host import must not appear when no host main.nix in tree"
    );
}

// ─── 12.9b Host main.nix present — imports it ───────────────────────────────

#[tokio::test]
async fn test_12_9b_host_main_nix_present() {
    let ctx = baseline(TestContext::builder().inventory_enabled())
        .commit_file(
            TEST_SHA,
            format!("roles/{TEST_ROLE}/{TEST_HOST}/main.nix"),
            "{ ... }: {}",
        )
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    let config_nix = ctx.nixos_config_file().unwrap();
    assert!(config_nix.contains(&format!("{TEST_HOST}/main.nix")));
}

// ─── 12.10 Inventory disabled — no DynamoDB writes ──────────────────────────

#[tokio::test]
async fn test_12_10_inventory_disabled() {
    let ctx = baseline(TestContext::builder()).build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert_eq!(ctx.dynamo_put_count(), 0);
}

// ─── 12.11 Inventory write failure — apply continues ────────────────────────

#[tokio::test]
async fn test_12_11_dynamo_failure_apply_continues() {
    let ctx = baseline(TestContext::builder().inventory_enabled())
        .dynamo_error()
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
    assert_eq!(ctx.symlink_target().as_deref(), Some(TEST_SHA));
}

// ─── 12.12 Secrets fetched, host wins over role ─────────────────────────────

#[tokio::test]
async fn test_12_12_secrets_host_wins() {
    let role_secret = format!("NixOps/{TEST_ROLE}/shared/api-key");
    let host_secret = format!("NixOps/{TEST_ROLE}/{TEST_HOST}/api-key");

    let ctx = baseline(TestContext::builder().inventory_enabled())
        .secret(role_secret, "role-value")
        .secret(host_secret, "host-value")
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    let secret = ctx.secret_file("api-key").expect("api-key secret should exist");
    assert_eq!(secret, "host-value");
}

// ─── 12.13 Query results written before rebuild ─────────────────────────────

#[tokio::test]
async fn test_12_13_inventory_json_written() {
    let queries_toml = r#"
[[query]]
name        = "zk_nodes"
role_prefix = "home/production/zookeeper"
"#;

    let zk_item = InventoryItem {
        hostname: "zk-01.waldman.internal".to_string(),
        ip: "192.168.1.10".to_string(),
        role: "home/production/zookeeper".to_string(),
        last_run_status: "ok".to_string(),
    };

    let ctx = baseline(TestContext::builder().inventory_enabled())
        .commit_file(TEST_SHA, format!("roles/{TEST_ROLE}/queries.toml"), queries_toml)
        .inventory_items(vec![zk_item])
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
    let inv_path = ctx.var_dir.path().join("inventory.json");
    assert!(inv_path.exists(), "inventory.json should be written");
    let content = std::fs::read_to_string(&inv_path).unwrap();
    assert!(content.contains("zk_nodes"));
    assert!(content.contains("zk-01.waldman.internal"));
}

// ─── 12.14 NIX_PATH — `-I nixops3=...` points at local tree ─────────────────

#[tokio::test]
async fn test_12_14_nix_path_resolution() {
    let ctx = baseline(TestContext::builder().inventory_enabled()).build();

    ctx.run_cycle().await;

    let calls = ctx.executor.calls.lock().unwrap();
    let rebuild_call = calls
        .iter()
        .find(|c| c.first().map(|s| s == "nixos-rebuild").unwrap_or(false))
        .expect("nixos-rebuild called");
    let joined = rebuild_call.join(" ");
    assert!(
        joined.contains(&format!(
            "nixops3={}",
            ctx.var_dir.path().join("commits").join(TEST_SHA).display()
        )),
        "-I nixops3= should point at the local commit tree; got: {joined}"
    );
    // Must contain -I nixos-config= pointing at the generated file.
    // Required because systemd strips NIX_PATH (see daemon.rs step 9 comment).
    assert!(
        joined.contains(&format!(
            "nixos-config={}",
            ctx.nixos_dir.path().join("configuration.nix").display()
        )),
        "-I nixos-config= must be passed explicitly; got: {joined}"
    );
}
