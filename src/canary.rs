use anyhow::Result;
use crate::paths::commit_canary;
use crate::traits::S3Ops;

/// Returns `true` if the daemon should apply (proceed), `false` to skip.
///
/// Canary is role-scoped and per-commit. The daemon fetches:
///     commits/<sha>/roles/<role>/canary.txt
///
/// Semantics:
/// - Absent (404) → apply
/// - Present, hostname listed → apply
/// - Present, hostname NOT listed → skip
pub async fn check_canary(
    s3: &dyn S3Ops,
    sha: &str,
    role: &str,
    hostname: &str,
) -> Result<bool> {
    let key = commit_canary(sha, role);
    match s3.get_object(&key).await? {
        None => Ok(true),
        Some(bytes) => {
            let content = String::from_utf8_lossy(&bytes);
            let listed = content
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .any(|l| l == hostname);
            Ok(listed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::collections::HashMap;

    struct MockS3(HashMap<String, Vec<u8>>);

    #[async_trait]
    impl S3Ops for MockS3 {
        async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(key).cloned())
        }
        async fn list_prefix(&self, prefix: &str) -> Result<Vec<String>> {
            Ok(self.0.keys().filter(|k| k.starts_with(prefix)).cloned().collect())
        }
    }

    const SHA: &str = "abc1234";
    const ROLE: &str = "home/production/webserver";
    const HOST: &str = "web-01.waldman.internal";

    fn s3_with(content: &str) -> MockS3 {
        let mut m = HashMap::new();
        m.insert(commit_canary(SHA, ROLE), content.as_bytes().to_vec());
        MockS3(m)
    }

    fn s3_empty() -> MockS3 {
        MockS3(HashMap::new())
    }

    #[tokio::test]
    async fn test_4_1_no_canary_file() {
        assert!(check_canary(&s3_empty(), SHA, ROLE, HOST).await.unwrap());
    }

    #[tokio::test]
    async fn test_4_2_hostname_listed() {
        let s3 = s3_with(&format!("{HOST}\n"));
        assert!(check_canary(&s3, SHA, ROLE, HOST).await.unwrap());
    }

    #[tokio::test]
    async fn test_4_3_hostname_not_listed() {
        let s3 = s3_with("web-02.waldman.internal\n");
        assert!(!check_canary(&s3, SHA, ROLE, HOST).await.unwrap());
    }

    #[tokio::test]
    async fn test_4_4_empty_all_skip() {
        assert!(!check_canary(&s3_with(""), SHA, ROLE, HOST).await.unwrap());
    }

    #[tokio::test]
    async fn test_4_5_partial_match_rejected() {
        let s3 = s3_with("web-01\n");
        assert!(!check_canary(&s3, SHA, ROLE, HOST).await.unwrap());
    }

    #[tokio::test]
    async fn test_4_6_comments_ignored() {
        let s3 = s3_with(&format!("# comment\n{HOST}\n"));
        assert!(check_canary(&s3, SHA, ROLE, HOST).await.unwrap());
    }

    #[tokio::test]
    async fn test_4_7_blank_lines_ignored() {
        let s3 = s3_with(&format!("\n\n{HOST}\n\n"));
        assert!(check_canary(&s3, SHA, ROLE, HOST).await.unwrap());
    }

    #[tokio::test]
    async fn test_4_8_multiple_hostnames() {
        let s3 = s3_with(&format!("web-01.waldman.internal\n{HOST}\n"));
        let host2 = "web-02.waldman.internal";
        assert!(check_canary(&s3, SHA, ROLE, host2).await.is_ok());
    }
}
