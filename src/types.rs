use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryItem {
    pub hostname: String,
    pub ip: String,
    pub role: String,
    pub last_run_status: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LastRunStatus {
    Ok,
    Failed,
    CanarySkip,
}

impl LastRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::CanarySkip => "canary_skip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CycleOutcome {
    /// Applied a new commit and advanced the symlink.
    Applied,
    /// Symlink already pointed at the target sha; nothing to do.
    NoOp,
    /// Canary gate rejected this host.
    CanarySkip,
    /// Failed to resolve the pointer or fetch the tree.
    S3Error,
    /// `nixos-rebuild` returned non-zero; symlink not advanced.
    RebuildFailed,
}

/// DynamoDB attribute value — avoids leaking AWS SDK types into trait.
#[derive(Debug, Clone)]
pub enum DynVal {
    S(String),
    /// DynamoDB stores numbers as strings.
    N(String),
}
