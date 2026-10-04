#[cfg(target_os = "windows")]
use std::time::{SystemTime, UNIX_EPOCH};

// WinRT DateTime uses 100 ns intervals since 1601-01-01 UTC.
#[cfg(target_os = "windows")]
const WINDOWS_EPOCH_TICKS: i64 = 11_644_473_600 * 10_000_000;
const TICKS_PER_SECOND: f64 = 10_000_000.0;

#[cfg(target_os = "windows")]
pub(crate) fn winrt_utc_now_ticks() -> Option<i64> {
    let unix_ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos()
        .checked_div(100)?;
    i64::try_from(unix_ticks)
        .ok()?
        .checked_add(WINDOWS_EPOCH_TICKS)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn estimate_position_secs(
    position_ticks: i64,
    last_updated_ticks: Option<i64>,
    now_ticks: Option<i64>,
    playing: bool,
) -> f64 {
    let elapsed_ticks = if playing {
        last_updated_ticks
            .zip(now_ticks)
            .map(|(updated, now)| now.saturating_sub(updated).max(0))
            .unwrap_or(0)
    } else {
        0
    };

    position_ticks.max(0).saturating_add(elapsed_ticks) as f64 / TICKS_PER_SECOND
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 10_000_000;

    #[test]
    fn advances_a_position_from_its_smtc_update_time() {
        assert_eq!(
            estimate_position_secs(20 * SECOND, Some(100 * SECOND), Some(102 * SECOND), true),
            22.0
        );
    }

    #[test]
    fn repeated_stale_timeline_values_still_advance_smoothly() {
        let updated = 100 * SECOND;
        let position = 20 * SECOND;
        let samples = [102 * SECOND, 102 * SECOND + SECOND / 2, 103 * SECOND];
        let estimates: Vec<_> = samples
            .into_iter()
            .map(|now| estimate_position_secs(position, Some(updated), Some(now), true))
            .collect();

        assert_eq!(estimates, [22.0, 22.5, 23.0]);
    }

    #[test]
    fn paused_or_missing_timestamp_does_not_extrapolate() {
        assert_eq!(
            estimate_position_secs(20 * SECOND, Some(100 * SECOND), Some(102 * SECOND), false),
            20.0
        );
        assert_eq!(
            estimate_position_secs(20 * SECOND, None, Some(102 * SECOND), true),
            20.0
        );
    }

    #[test]
    fn future_timestamp_and_negative_position_are_safe() {
        assert_eq!(
            estimate_position_secs(20 * SECOND, Some(103 * SECOND), Some(102 * SECOND), true),
            20.0
        );
        assert_eq!(
            estimate_position_secs(-SECOND, Some(100 * SECOND), Some(102 * SECOND), true),
            2.0
        );
    }
}
