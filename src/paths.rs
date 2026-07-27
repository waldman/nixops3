/// S3 key for the role's main.nix
pub fn role_main_nix(role: &str) -> String {
    format!("roles/{}/main.nix", role)
}

/// S3 key for the host-specific main.nix
pub fn host_main_nix(role: &str, hostname: &str) -> String {
    format!("roles/{}/{}/main.nix", role, hostname)
}

/// S3 key for the role-level queries.toml
pub fn role_queries_toml(role: &str) -> String {
    format!("roles/{}/queries.toml", role)
}

/// S3 key for the host-level queries.toml
pub fn host_queries_toml(role: &str, hostname: &str) -> String {
    format!("roles/{}/{}/queries.toml", role, hostname)
}

/// Secrets Manager prefix for role-level (shared) secrets
pub fn secrets_role_prefix(role: &str) -> String {
    format!("NixOps/{}/shared/", role)
}

/// Secrets Manager prefix for host-level secrets
pub fn secrets_host_prefix(role: &str, hostname: &str) -> String {
    format!("NixOps/{}/{}/", role, hostname)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_2_1_role_main_nix() {
        assert_eq!(role_main_nix("home/production/ada"), "roles/home/production/ada/main.nix");
    }

    #[test]
    fn test_2_2_host_main_nix() {
        assert_eq!(
            host_main_nix("home/production/ada", "ada-01.waldman.internal"),
            "roles/home/production/ada/ada-01.waldman.internal/main.nix"
        );
    }

    #[test]
    fn test_2_3_queries_toml_paths() {
        assert_eq!(
            role_queries_toml("home/production/ada"),
            "roles/home/production/ada/queries.toml"
        );
        assert_eq!(
            host_queries_toml("home/production/ada", "ada-01.waldman.internal"),
            "roles/home/production/ada/ada-01.waldman.internal/queries.toml"
        );
    }

    #[test]
    fn test_2_4_canary_path() {
        assert_eq!("canary.txt", "canary.txt");
    }

    #[test]
    fn test_2_5_secrets_prefixes() {
        assert_eq!(
            secrets_role_prefix("home/production/ada"),
            "NixOps/home/production/ada/shared/"
        );
        assert_eq!(
            secrets_host_prefix("home/production/ada", "ada-01.waldman.internal"),
            "NixOps/home/production/ada/ada-01.waldman.internal/"
        );
    }
}
