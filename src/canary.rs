use anyhow::Result;
use crate::traits::S3Ops;

/// Returns `true` if the daemon should apply (proceed), `false` to skip.
///
/// Logic:
/// - `canary.txt` absent → apply
/// - `canary.txt` present and hostname listed → apply
/// - `canary.txt` present and hostname NOT listed → skip
pub async fn check_canary(s3: &dyn S3Ops, hostname: &str) -> Result<bool> {
    match s3.get_object("canary.txt").await? {
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

    fn s3_with(key: &str, content: &str) -> MockS3 {
        let mut m = HashMap::new();
        m.insert(key.to_string(), content.as_bytes().to_vec());
        MockS3(m)
    }

    fn s3_empty() -> MockS3 {
        MockS3(HashMap::new())
    }

    #[tokio::test]
    async fn test_4_1_no_canary_file() {
        assert!(check_canary(&s3_empty(), "ada-01.waldman.internal").await.unwrap());
    }

    #[tokio::test]
    async fn test_4_2_hostname_in_canary() {
        let s3 = s3_with("canary.txt", "ada-01.waldman.internal\n");
        assert!(check_canary(&s3, "ada-01.waldman.internal").await.unwrap());
    }

    #[tokio::test]
    async fn test_4_3_hostname_not_in_canary() {
        let s3 = s3_with("canary.txt", "ada-02.waldman.internal\n");
        assert!(!check_canary(&s3, "ada-01.waldman.internal").await.unwrap());
    }

    #[tokio::test]
    async fn test_4_4_empty_canary_all_skip() {
        let s3 = s3_with("canary.txt", "");
        assert!(!check_canary(&s3, "ada-01.waldman.internal").await.unwrap());
    }

    #[tokio::test]
    async fn test_4_5_partial_match_not_accepted() {
        let s3 = s3_with("canary.txt", "ada-01\n");
        assert!(!check_canary(&s3, "ada-01.waldman.internal").await.unwrap());
    }

    #[tokio::test]
    async fn test_4_6_comment_lines_ignored() {
        let s3 = s3_with("canary.txt", "# comment\nada-01.waldman.internal\n");
        assert!(check_canary(&s3, "ada-01.waldman.internal").await.unwrap());
    }

    #[tokio::test]
    async fn test_4_7_blank_lines_ignored() {
        let s3 = s3_with("canary.txt", "\n\nada-01.waldman.internal\n\n");
        assert!(check_canary(&s3, "ada-01.waldman.internal").await.unwrap());
    }

    #[tokio::test]
    async fn test_4_8_multiple_hostnames() {
        let s3 = s3_with("canary.txt", "ada-01.waldman.internal\nada-02.waldman.internal\n");
        assert!(check_canary(&s3, "ada-02.waldman.internal").await.unwrap());
    }
}
