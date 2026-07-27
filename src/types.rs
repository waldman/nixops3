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
    Applied,
    CanarySkip,
    HashUnchanged,
    S3Error,
    RebuildFailed,
}

/// DynamoDB attribute value — avoids leaking AWS SDK types into trait.
#[derive(Debug, Clone)]
pub enum DynVal {
    S(String),
    /// DynamoDB stores numbers as strings.
    N(String),
}
