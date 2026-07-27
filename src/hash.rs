use sha2::{Digest, Sha256};

/// Computes SHA-256 of all `.nix` file contents, sorted by S3 key.
/// `queries.toml` and other non-`.nix` files are excluded.
/// Returns lowercase hex string.
pub fn compute_hash(files: &[(String, Vec<u8>)]) -> String {
    let mut nix_files: Vec<&(String, Vec<u8>)> = files
        .iter()
        .filter(|(key, _)| key.ends_with(".nix"))
        .collect();
    nix_files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (_, content) in &nix_files {
        hasher.update(content);
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.as_bytes().to_vec())).collect()
    }

    #[test]
    fn test_3_1_deterministic() {
        let a = files(&[("profiles/base.nix", "a"), ("profiles/docker.nix", "b")]);
        let b = files(&[("profiles/docker.nix", "b"), ("profiles/base.nix", "a")]);
        assert_eq!(compute_hash(&a), compute_hash(&b));
    }

    #[test]
    fn test_3_2_hash_changes_on_content_change() {
        let a = files(&[("profiles/base.nix", "a")]);
        let b = files(&[("profiles/base.nix", "changed")]);
        assert_ne!(compute_hash(&a), compute_hash(&b));
    }

    #[test]
    fn test_3_3_hash_changes_on_addition() {
        let a = files(&[("profiles/base.nix", "a")]);
        let b = files(&[("profiles/base.nix", "a"), ("profiles/docker.nix", "b")]);
        assert_ne!(compute_hash(&a), compute_hash(&b));
    }

    #[test]
    fn test_3_4_hash_changes_on_removal() {
        let a = files(&[("profiles/base.nix", "a"), ("profiles/docker.nix", "b")]);
        let b = files(&[("profiles/base.nix", "a")]);
        assert_ne!(compute_hash(&a), compute_hash(&b));
    }

    #[test]
    fn test_3_5_empty_file_set() {
        let h = compute_hash(&[]);
        assert!(!h.is_empty());
        // should be stable
        assert_eq!(h, compute_hash(&[]));
    }

    #[test]
    fn test_3_6_queries_toml_excluded() {
        let with_queries = files(&[
            ("profiles/base.nix", "a"),
            ("nix-roles/home/production/ada/queries.toml", "[[query]]\nname=\"x\"\nrole_prefix=\"y\""),
        ]);
        let without_queries = files(&[("profiles/base.nix", "a")]);
        assert_eq!(compute_hash(&with_queries), compute_hash(&without_queries));
    }
}
