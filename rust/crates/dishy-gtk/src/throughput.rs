//! The tray's live throughput readout.
//!
//! Rates are bits per second, the same unit the dish reports and the dashboard
//! renders. The thresholds and the packed unit spelling match
//! `electron/menuBarThroughput.ts`, so a Linux tray and a macOS menu bar describe
//! the same link the same way.

pub const THROUGHPUT_STALE_MS: i64 = 6_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThroughputSample {
    pub at_ms: i64,
    pub down_bps: f64,
    pub up_bps: f64,
}

/// Compact bitrate: `1.2Mb/s`, `340Kb/s`, `2.0Gb/s`.
pub fn format_rate(bits_per_second: f64) -> String {
    if bits_per_second >= 1_000_000_000.0 {
        format!("{:.1}Gb/s", bits_per_second / 1_000_000_000.0)
    } else if bits_per_second >= 1_000_000.0 {
        format!("{:.1}Mb/s", bits_per_second / 1_000_000.0)
    } else {
        format!("{}Kb/s", (bits_per_second / 1_000.0).round())
    }
}

/// The same rate with a space before the unit, for a surface with width to spare.
pub fn format_spaced_rate(bits_per_second: f64) -> String {
    let compact = format_rate(bits_per_second);
    for unit in ["Kb/s", "Mb/s", "Gb/s"] {
        if let Some(prefix) = compact.strip_suffix(unit) {
            return format!("{prefix} {unit}");
        }
    }
    compact
}

pub fn readout(down_bps: f64, up_bps: f64) -> String {
    format!(
        "↓{} ↑{}",
        format_spaced_rate(down_bps),
        format_spaced_rate(up_bps)
    )
}

/// A stale feed reads as zero rather than freezing its last value: the dish is
/// unreachable, and a number that never moves would claim otherwise.
pub fn live_readout(sample: Option<&ThroughputSample>, now_ms: i64) -> String {
    match sample {
        Some(sample) if now_ms.saturating_sub(sample.at_ms) < THROUGHPUT_STALE_MS => {
            readout(sample.down_bps, sample.up_bps)
        }
        _ => readout(0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SHELL-007: the tray reports a live readout while the recorder polls,
    // and ages a dead feed out to zero instead of freezing it.
    #[test]
    fn tray_readout_uses_the_shared_rate_format() {
        assert_eq!(format_rate(0.0), "0Kb/s");
        assert_eq!(format_rate(340_000.0), "340Kb/s");
        assert_eq!(format_rate(999_999.0), "1000Kb/s");
        assert_eq!(format_rate(1_000_000.0), "1.0Mb/s");
        assert_eq!(format_rate(23_500_000.0), "23.5Mb/s");
        assert_eq!(format_rate(2_400_000_000.0), "2.4Gb/s");
        assert_eq!(format_spaced_rate(1_200_000.0), "1.2 Mb/s");
        assert_eq!(readout(1_200_000.0, 340_000.0), "↓1.2 Mb/s ↑340 Kb/s");
    }

    #[test]
    fn stale_samples_read_as_zero() {
        let sample = ThroughputSample {
            at_ms: 1_000,
            down_bps: 23_500_000.0,
            up_bps: 1_200_000.0,
        };
        assert_eq!(live_readout(Some(&sample), 6_999), "↓23.5 Mb/s ↑1.2 Mb/s");
        assert_eq!(live_readout(Some(&sample), 7_000), "↓0 Kb/s ↑0 Kb/s");
        assert_eq!(live_readout(None, 7_000), "↓0 Kb/s ↑0 Kb/s");
    }
}
