//! Number formatting shared by the GTK views.
//!
//! JavaScript's `toFixed` rounds a positive exact half away from zero, while
//! Rust's formatter rounds half to even. These helpers round the scaled value
//! first so the native views print the same figures as the Electron app.

/// Fixed-point text with trailing zeros and a trailing decimal point removed.
pub fn fixed(value: f64, decimals: usize) -> String {
    let factor = 10_f64.powi(i32::try_from(decimals).unwrap_or(0));
    let rounded = if factor.is_finite() && factor > 0.0 {
        (value * factor + 0.5).floor() / factor
    } else {
        value
    };
    let mut text = format!("{rounded:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

/// Exactly `digits` decimals, zeros kept, rounding ties away from zero as
/// JavaScript's `toFixed` does.
pub fn to_fixed(value: f64, digits: usize) -> String {
    let scale = 10_f64.powi(i32::try_from(digits).unwrap_or(0));
    format!("{:.digits$}", (value * scale).round() / scale)
}

/// `"268 kbps"`, `"1.5 Mbps"`, `"2.00 Gbps"` — the shared throughput label.
pub fn format_mbps(mbps: f64) -> String {
    let mbps = if mbps.is_finite() { mbps.max(0.0) } else { 0.0 };
    if mbps >= 1_000.0 {
        format!("{} Gbps", fixed(mbps / 1_000.0, 2))
    } else if mbps >= 1.0 {
        format!("{} Mbps", fixed(mbps, 1))
    } else {
        format!("{} kbps", fixed(mbps * 1_000.0, 0))
    }
}

/// Byte counters → human readable string (e.g. `"500 B"`, `"12 kB"`, `"1.5 MB"`, `"3.25 GB"`).
pub fn format_bytes(bytes: u64) -> String {
    let bytes_f = bytes as f64;
    if bytes_f < 1_000.0 {
        return format!("{bytes} B");
    }
    const SCALES: &[(f64, &str, usize)] = &[(1e3, "kB", 0), (1e6, "MB", 1), (1e9, "GB", 2)];
    for &(scale, unit, decimals) in SCALES {
        let val_str = fixed(bytes_f / scale, decimals);
        if let Ok(val) = val_str.parse::<f64>()
            && val < 1_000.0
        {
            return format!("{val_str} {unit}");
        }
    }
    format!("{} TB", fixed(bytes_f / 1e12, 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_scales_properly() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1_500), "2 kB");
        assert_eq!(format_bytes(1_500_000), "1.5 MB");
        assert_eq!(format_bytes(2_340_000_000), "2.34 GB");
        assert_eq!(format_bytes(4_500_000_000_000), "4.5 TB");
    }

    // SPEC-NET-003: rates read the same way as the Electron panel.
    #[test]
    fn throughput_labels_choose_a_readable_unit() {
        assert_eq!(format_mbps(0.0), "0 kbps");
        assert_eq!(format_mbps(0.268), "268 kbps");
        assert_eq!(format_mbps(1.25), "1.3 Mbps");
        assert_eq!(format_mbps(1_234.0), "1.23 Gbps");
        assert_eq!(format_mbps(f64::NAN), "0 kbps");
    }
}
