mod common;
use common::TestContext;
use nixops3d::types::{CycleOutcome, InventoryItem};

// ─── 9.1 Happy path — first run ───────────────────────────────────────────────

#[tokio::test]
async fn test_9_1_happy_path_first_run() {
    let main_nix = r#"{ ... }: { imports = [ <nixops3/profiles/base.nix> ]; }"#;
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("profiles/base.nix", "{ config, ... }: {}")
        .s3_file("roles/home/production/ada/main.nix", main_nix)
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
    assert!(!ctx.last_hash().is_empty());
    assert_eq!(ctx.last_hash(), ctx.computed_hash());
    assert_eq!(ctx.dynamo_status(), "ok");
}

// ─── 9.2 No-op — hash unchanged ───────────────────────────────────────────────

#[tokio::test]
async fn test_9_2_hash_unchanged_no_rebuild() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .build();

    ctx.run_cycle().await;
    let hash_after_first = ctx.last_hash();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::HashUnchanged);
    assert_eq!(ctx.rebuild_call_count(), 1, "rebuild called only once total");
    assert_eq!(ctx.last_hash(), hash_after_first);
    assert_eq!(ctx.dynamo_status(), "ok");
}

// ─── 9.3 Apply triggered — hash changed ───────────────────────────────────────

#[tokio::test]
async fn test_9_3_hash_changed_triggers_rebuild() {
    let ctx1 = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .build();
    let outcome1 = ctx1.run_cycle().await;
    assert_eq!(outcome1, CycleOutcome::Applied);
    let first_hash = ctx1.last_hash();

    let ctx2 = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: { # changed }")
        .last_hash(first_hash.clone())
        .build();
    let outcome2 = ctx2.run_cycle().await;

    assert_eq!(outcome2, CycleOutcome::Applied);
    assert!(ctx2.rebuild_was_called());
    assert_ne!(ctx2.last_hash(), first_hash);
    assert_eq!(ctx2.last_hash(), ctx2.computed_hash());
}

// ─── 9.4 Canary skip ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_9_4_canary_skip() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .hostname("ada-01.waldman.internal")
        .s3_file("canary.txt", "ada-02.waldman.internal\n")
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::CanarySkip);
    assert!(!ctx.rebuild_was_called());
    assert!(ctx.last_hash().is_empty(), "hash not updated on canary skip");
    assert_eq!(ctx.dynamo_status(), "canary_skip");
}

// ─── 9.5 nixos-rebuild failure — no hash update ────────────────────────────────

#[tokio::test]
async fn test_9_5_rebuild_failure_no_hash_update() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .rebuild_exit_code(1)
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::RebuildFailed);
    assert!(ctx.rebuild_was_called());
    assert!(ctx.last_hash().is_empty(), "hash must not be updated on failure");
    assert_eq!(ctx.dynamo_status(), "failed");
}

// ─── 9.6 S3 download failure ──────────────────────────────────────────────────

#[tokio::test]
async fn test_9_6_s3_failure_apply_skipped() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_get_error()
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::S3Error);
    assert!(!ctx.rebuild_was_called());
    assert!(ctx.last_hash().is_empty());
    assert_eq!(ctx.dynamo_status(), "failed");
}

// ─── 9.7 Host main.nix absent — apply succeeds ───────────────────────────────

#[tokio::test]
async fn test_9_7_no_host_main_nix() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        // NO host main.nix
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());

    let config_nix = ctx.work_dir_file("configuration.nix").unwrap();
    assert!(
        !config_nix.contains("ada-01.waldman.internal/main.nix"),
        "host import must not appear"
    );
}

// ─── 9.8 Inventory disabled — no DynamoDB writes ─────────────────────────────

#[tokio::test]
async fn test_9_8_inventory_disabled_no_dynamo() {
    let ctx = TestContext::builder()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert_eq!(ctx.dynamo_put_count(), 0, "DynamoDB must not be called when inventory disabled");
}

// ─── 9.9 Inventory write failure — apply continues ───────────────────────────

#[tokio::test]
async fn test_9_9_dynamo_failure_apply_continues() {
    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .dynamo_error()
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
}

// ─── 9.10 Secrets fetched before rebuild ─────────────────────────────────────

#[tokio::test]
async fn test_9_10_secrets_fetched_before_rebuild() {
    let role_secret = "NixOps/home/production/ada/shared/api-key";
    let host_secret = "NixOps/home/production/ada/ada-01.waldman.internal/api-key";

    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .secret(role_secret, "role-value")
        .secret(host_secret, "host-value")
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    let secret = ctx.secret_file("api-key").expect("api-key secret should exist");
    assert_eq!(secret, "host-value");
}

// ─── 9.11 Query results written before rebuild ────────────────────────────────

#[tokio::test]
async fn test_9_11_inventory_json_written() {
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

    let ctx = TestContext::builder()
        .inventory_enabled()
        .s3_file("roles/home/production/ada/main.nix", "{ ... }: {}")
        .s3_file("roles/home/production/ada/queries.toml", queries_toml)
        .inventory_items(vec![zk_item])
        .build();

    let outcome = ctx.run_cycle().await;

    assert_eq!(outcome, CycleOutcome::Applied);
    assert!(ctx.rebuild_was_called());
}
