// S3 key builders.
//
// The bucket has exactly one root-level object (`current`) plus a tree of
// immutable per-commit prefixes under `commits/<sha>/`. Everything is
// commit-scoped.

/// The bucket-root pointer object.
pub const POINTER_KEY: &str = "current";

/// Prefix for the commit tree of a given sha (trailing slash included).
pub fn commit_prefix(sha: &str) -> String {
    format!("commits/{sha}/")
}

/// Role-scoped canary.txt inside the commit tree.
pub fn commit_canary(sha: &str, role: &str) -> String {
    format!("commits/{sha}/roles/{role}/canary.txt")
}

/// Role main.nix inside the commit tree.
pub fn commit_role_main(sha: &str, role: &str) -> String {
    format!("commits/{sha}/roles/{role}/main.nix")
}

/// Host-scoped main.nix inside the commit tree.
pub fn commit_host_main(sha: &str, role: &str, hostname: &str) -> String {
    format!("commits/{sha}/roles/{role}/hosts/{hostname}/main.nix")
}

/// Role-scoped queries.toml.
pub fn commit_role_queries(sha: &str, role: &str) -> String {
    format!("commits/{sha}/roles/{role}/queries.toml")
}

/// Host-scoped queries.toml.
pub fn commit_host_queries(sha: &str, role: &str, hostname: &str) -> String {
    format!("commits/{sha}/roles/{role}/hosts/{hostname}/queries.toml")
}

/// Secrets Manager prefix for role-level (shared) secrets. Not commit-scoped.
pub fn secrets_role_prefix(role: &str) -> String {
    format!("NixOps/{role}/shared/")
}

/// Secrets Manager prefix for host-level secrets. Not commit-scoped.
pub fn secrets_host_prefix(role: &str, hostname: &str) -> String {
    format!("NixOps/{role}/{hostname}/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "abc1234";
    const ROLE: &str = "home/production/webserver";
    const HOST: &str = "web-01.waldman.internal";

    #[test]
    fn test_2_1_pointer_path() {
        assert_eq!(POINTER_KEY, "current");
    }

    #[test]
    fn test_2_2_commit_prefix() {
        assert_eq!(commit_prefix(SHA), "commits/abc1234/");
    }

    #[test]
    fn test_2_3_role_main() {
        assert_eq!(
            commit_role_main(SHA, ROLE),
            "commits/abc1234/roles/home/production/webserver/main.nix"
        );
    }

    #[test]
    fn test_2_4_host_main() {
        assert_eq!(
            commit_host_main(SHA, ROLE, HOST),
            "commits/abc1234/roles/home/production/webserver/hosts/web-01.waldman.internal/main.nix"
        );
    }

    #[test]
    fn test_2_5_canary_path() {
        assert_eq!(
            commit_canary(SHA, ROLE),
            "commits/abc1234/roles/home/production/webserver/canary.txt"
        );
    }

    #[test]
    fn test_2_6_secrets_prefixes() {
        assert_eq!(secrets_role_prefix(ROLE), "NixOps/home/production/webserver/shared/");
        assert_eq!(
            secrets_host_prefix(ROLE, HOST),
            "NixOps/home/production/webserver/web-01.waldman.internal/"
        );
    }
}
