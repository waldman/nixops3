use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::config::PinsConfig;
use crate::main_yaml::Pin;

/// Result of resolving the pin for a cycle.
#[derive(Debug, Clone, PartialEq)]
pub enum PinResolution {
    /// No pin block; daemon falls back to channel discovery.
    Loose,
    /// `channel` provided, `rev` resolved from channel.
    Floating { channel: String, rev: String },
    /// Both `channel` and `rev` provided explicitly.
    Pinned { channel: String, rev: String },
}

impl PinResolution {
    pub fn mode(&self) -> &'static str {
        match self {
            Self::Loose => "loose",
            Self::Floating { .. } => "floating",
            Self::Pinned { .. } => "pinned",
        }
    }

    pub fn channel(&self) -> Option<&str> {
        match self {
            Self::Loose => None,
            Self::Floating { channel, .. } | Self::Pinned { channel, .. } => Some(channel),
        }
    }

    pub fn rev(&self) -> Option<&str> {
        match self {
            Self::Loose => None,
            Self::Floating { rev, .. } | Self::Pinned { rev, .. } => Some(rev),
        }
    }
}

/// Construct the `-I nixpkgs=` value for a resolved pin.
///
/// - Loose: caller must fall back to `find_nixpkgs()` (local path).
/// - Floating/Pinned: a github tarball URL that nix downloads and caches itself.
pub fn nixpkgs_arg_url(res: &PinResolution) -> Option<String> {
    let rev = res.rev()?;
    Some(format!(
        "https://github.com/NixOS/nixpkgs/archive/{rev}.tar.gz"
    ))
}

/// Resolves a channel to a git rev.
#[async_trait]
pub trait ChannelResolver: Send + Sync {
    async fn resolve(&self, channel: &str) -> Result<String>;
}

/// Fetches `https://channels.nixos.org/<channel>/git-revision`.
/// Small text file, CDN-cached, effectively unlimited rate — returns the
/// last Hydra-tested rev for that channel (same as `nix-channel --update`
/// would fetch).
pub struct NixosChannelResolver {
    client: reqwest::Client,
}

impl NixosChannelResolver {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("nixops3d")
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client build"),
        }
    }
}

impl Default for NixosChannelResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelResolver for NixosChannelResolver {
    async fn resolve(&self, channel: &str) -> Result<String> {
        let url = format!("https://channels.nixos.org/{channel}/git-revision");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow!("channel resolve HTTP error for {channel}: {e}"))?;

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!(
                "channel not published on channels.nixos.org: {channel}"
            ));
        }
        if !status.is_success() {
            return Err(anyhow!(
                "channel resolve HTTP status {status} for {channel}"
            ));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| anyhow!("channel resolve body read: {e}"))?;
        let rev = body.trim();
        if rev.len() != 40 || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow!(
                "channel resolve: invalid rev in response for {channel}: {rev:?}"
            ));
        }
        Ok(rev.to_lowercase())
    }
}

/// In-process TTL cache wrapping any `ChannelResolver`.
pub struct CachedResolver<R: ChannelResolver> {
    inner: R,
    ttl: Duration,
    cache: Mutex<HashMap<String, (String, Instant)>>,
}

impl<R: ChannelResolver> CachedResolver<R> {
    pub fn new(inner: R, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<R: ChannelResolver> ChannelResolver for CachedResolver<R> {
    async fn resolve(&self, channel: &str) -> Result<String> {
        if let Some((rev, at)) = self.cache.lock().unwrap().get(channel).cloned() {
            if at.elapsed() < self.ttl {
                debug!("channel {channel} resolved from cache: {rev}");
                return Ok(rev);
            }
        }
        let rev = self.inner.resolve(channel).await?;
        self.cache
            .lock()
            .unwrap()
            .insert(channel.to_string(), (rev.clone(), Instant::now()));
        Ok(rev)
    }
}

/// Resolve a `Pin` (or absence thereof) into a `PinResolution`, respecting
/// operator strictness flags.
pub async fn resolve(
    pin: Option<&Pin>,
    cfg: &PinsConfig,
    resolver: &dyn ChannelResolver,
) -> Result<PinResolution> {
    match pin {
        None => {
            if cfg.require_pin {
                return Err(anyhow!(
                    "pin required (require_pin = true) but no pin declared in main.yaml"
                ));
            }
            warn!("nixpkgs unpinned — using channel discovery");
            Ok(PinResolution::Loose)
        }
        Some(pin) => match &pin.nixpkgs.rev {
            Some(rev) => {
                info!(
                    "nixpkgs channel={} rev={} (pinned)",
                    pin.nixpkgs.channel, rev
                );
                Ok(PinResolution::Pinned {
                    channel: pin.nixpkgs.channel.clone(),
                    rev: rev.clone(),
                })
            }
            None => {
                if cfg.require_explicit_rev {
                    return Err(anyhow!(
                        "pin.nixpkgs.rev required (require_explicit_rev = true) but only channel provided"
                    ));
                }
                let rev = resolver.resolve(&pin.nixpkgs.channel).await?;
                info!(
                    "nixpkgs channel={} rev={} (floating)",
                    pin.nixpkgs.channel, rev
                );
                Ok(PinResolution::Floating {
                    channel: pin.nixpkgs.channel.clone(),
                    rev,
                })
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::main_yaml::{NixpkgsPin, Pin};

    const REV: &str = "abcdef1234567890abcdef1234567890abcdef12";

    fn cfg() -> PinsConfig {
        PinsConfig::default()
    }

    struct StubResolver {
        result: Result<String>,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl StubResolver {
        fn ok(rev: &str) -> Self {
            Self {
                result: Ok(rev.to_string()),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn err(msg: &str) -> Self {
            Self {
                result: Err(anyhow!(msg.to_string())),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }
    #[async_trait]
    impl ChannelResolver for StubResolver {
        async fn resolve(&self, _channel: &str) -> Result<String> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match &self.result {
                Ok(r) => Ok(r.clone()),
                Err(e) => Err(anyhow!("{e}")),
            }
        }
    }

    #[tokio::test]
    async fn test_6b_1_loose_no_pin() {
        let r = StubResolver::ok(REV);
        let out = resolve(None, &cfg(), &r).await.unwrap();
        assert_eq!(out, PinResolution::Loose);
        assert_eq!(r.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_6b_2_loose_require_pin_fails() {
        let r = StubResolver::ok(REV);
        let c = PinsConfig { require_pin: true, ..cfg() };
        assert!(resolve(None, &c, &r).await.is_err());
    }

    #[tokio::test]
    async fn test_6b_3_floating_resolves() {
        let pin = Pin {
            nixpkgs: NixpkgsPin { channel: "nixos-25.05".into(), rev: None },
        };
        let r = StubResolver::ok(REV);
        let out = resolve(Some(&pin), &cfg(), &r).await.unwrap();
        assert_eq!(
            out,
            PinResolution::Floating {
                channel: "nixos-25.05".into(),
                rev: REV.into()
            }
        );
        assert_eq!(r.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_6b_4_cached_resolution() {
        let cached = CachedResolver::new(StubResolver::ok(REV), Duration::from_secs(60));
        let r1 = cached.resolve("nixos-25.05").await.unwrap();
        let r2 = cached.resolve("nixos-25.05").await.unwrap();
        assert_eq!(r1, r2);
        assert_eq!(
            cached.inner.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "second resolve should hit cache"
        );
    }

    #[tokio::test]
    async fn test_6b_5_floating_require_explicit_rev_fails() {
        let pin = Pin {
            nixpkgs: NixpkgsPin { channel: "c".into(), rev: None },
        };
        let r = StubResolver::ok(REV);
        let c = PinsConfig { require_explicit_rev: true, ..cfg() };
        assert!(resolve(Some(&pin), &c, &r).await.is_err());
    }

    #[tokio::test]
    async fn test_6b_6_pinned_no_resolver_call() {
        let pin = Pin {
            nixpkgs: NixpkgsPin { channel: "c".into(), rev: Some(REV.into()) },
        };
        let r = StubResolver::ok("other".into());
        let out = resolve(Some(&pin), &cfg(), &r).await.unwrap();
        assert_eq!(
            out,
            PinResolution::Pinned { channel: "c".into(), rev: REV.into() }
        );
        assert_eq!(r.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_6b_8_resolver_error_propagates() {
        let pin = Pin {
            nixpkgs: NixpkgsPin { channel: "c".into(), rev: None },
        };
        let r = StubResolver::err("boom");
        assert!(resolve(Some(&pin), &cfg(), &r).await.is_err());
    }

    #[test]
    fn test_6c_1_pinned_url() {
        let res = PinResolution::Pinned {
            channel: "nixos-25.05".into(),
            rev: REV.into(),
        };
        assert_eq!(
            nixpkgs_arg_url(&res).unwrap(),
            format!("https://github.com/NixOS/nixpkgs/archive/{REV}.tar.gz")
        );
    }

    #[test]
    fn test_6c_2_floating_url() {
        let res = PinResolution::Floating {
            channel: "c".into(),
            rev: REV.into(),
        };
        assert_eq!(
            nixpkgs_arg_url(&res).unwrap(),
            format!("https://github.com/NixOS/nixpkgs/archive/{REV}.tar.gz")
        );
    }

    #[test]
    fn test_6c_3_loose_no_url() {
        assert!(nixpkgs_arg_url(&PinResolution::Loose).is_none());
    }
}
