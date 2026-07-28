use anyhow::{anyhow, Result};

/// Parse the contents of the `current` pointer file.
/// Requires exactly 40 hex characters, optionally followed by whitespace.
/// Returns the lowercase sha.
pub fn parse(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.len() != 40 {
        return Err(anyhow!(
            "invalid pointer: expected 40 hex chars, got {}",
            trimmed.len()
        ));
    }
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid pointer: contains non-hex characters"));
    }
    Ok(trimmed.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "abcdef1234567890abcdef1234567890abcdef12";

    #[test]
    fn test_3_1_valid_sha() {
        assert_eq!(parse(SHA).unwrap(), SHA);
    }

    #[test]
    fn test_3_2_trailing_newline() {
        assert_eq!(parse(&format!("{SHA}\n")).unwrap(), SHA);
    }

    #[test]
    fn test_3_3_trailing_whitespace() {
        assert_eq!(parse(&format!("{SHA} \n")).unwrap(), SHA);
    }

    #[test]
    fn test_3_4_too_short() {
        assert!(parse("abc").is_err());
    }

    #[test]
    fn test_3_5_too_long() {
        assert!(parse(&format!("{SHA}0")).is_err());
    }

    #[test]
    fn test_3_6_non_hex() {
        let bad = "gbcdef1234567890abcdef1234567890abcdef12";
        assert!(parse(bad).is_err());
    }

    #[test]
    fn test_3_7_empty() {
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn test_3_8_embedded_whitespace() {
        let bad = "abcdef 234567890abcdef1234567890abcdef123";
        assert!(parse(bad).is_err());
    }

    #[test]
    fn test_uppercase_normalized() {
        let upper = "ABCDEF1234567890ABCDEF1234567890ABCDEF12";
        assert_eq!(parse(upper).unwrap(), SHA);
    }
}
