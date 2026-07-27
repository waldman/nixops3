use rand::Rng;
use std::time::Duration;

/// Returns `poll_interval_secs + jitter` where jitter is uniform in `[0, 60)` seconds.
pub fn sleep_duration(poll_interval_secs: u64) -> Duration {
    let jitter = rand::thread_rng().gen_range(0..60u64);
    Duration::from_secs(poll_interval_secs + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_8_1_jitter_within_bounds() {
        for _ in 0..1000 {
            let d = sleep_duration(600);
            let secs = d.as_secs();
            assert!(secs >= 600 && secs < 660, "secs={secs} out of [600, 660)");
        }
    }

    #[test]
    fn test_8_2_jitter_not_constant() {
        let values: std::collections::HashSet<u64> =
            (0..100).map(|_| sleep_duration(600).as_secs()).collect();
        assert!(values.len() >= 2, "all jitter values were identical");
    }
}
