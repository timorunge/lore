//! Human-friendly formatting utilities (byte sizes, durations, counts).

use std::time::Duration;

use anyhow::{Context, Result};

/// Convert megabytes to bytes.
pub fn mb_to_bytes(mb: usize) -> u64 {
    (mb as u64).saturating_mul(1024 * 1024)
}

/// Format a byte count as a human-readable string.
pub fn format_bytes(bytes: u64) -> String {
    // Thresholds tightened: values whose ratio rounds to 1024.0 at one decimal
    // place (e.g. 1_048_525 / 1024 = 1023.95...) are promoted to the next unit.
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1_048_525 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1_073_689_396 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format a large count with K/M/B suffixes.
pub fn format_count(n: u64) -> String {
    // Thresholds tightened: values whose ratio rounds to 1000.0 at one decimal
    // place (e.g. 999_950 / 1_000 = 999.95...) are promoted to the next unit.
    if n < 1_000 {
        format!("{n}")
    } else if n < 999_950 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else if n < 999_950_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n < 999_950_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else {
        format!("{:.1}T", n as f64 / 1_000_000_000_000.0)
    }
}

/// Format a duration as a human-readable string.
pub fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{:.1}s", d.as_secs_f64())
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!(
            "{}h {:02}m {:02}s",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    }
}

/// Render a build version as `lore v<version>`.
///
/// Stores written by older builds recorded the version with a leading `v`,
/// so the prefix is stripped before re-adding it.
pub fn format_version(version: &str) -> String {
    format!("lore v{}", version.strip_prefix('v').unwrap_or(version))
}

/// Serialize a value as pretty-printed JSON.
///
/// # Errors
///
/// Returns an error if the value cannot be serialized to JSON.
pub fn to_json_pretty<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string_pretty(value).context("JSON serialization failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_functions() {
        for (bytes, expected) in [
            (0u64, "0 B"),
            (500, "500 B"),
            (1023, "1023 B"),
            (1024, "1.0 KB"),
            (1_048_524, "1023.9 KB"),
            (1_048_525, "1.0 MB"),
            (1_048_576, "1.0 MB"),
            (1_073_689_395, "1023.9 MB"),
            (1_073_689_396, "1.0 GB"),
            (1_073_741_824, "1.0 GB"),
        ] {
            assert_eq!(
                format_bytes(bytes),
                expected,
                "format_bytes({bytes}) should be {expected:?}"
            );
        }

        for (n, expected) in [
            (0u64, "0"),
            (999, "999"),
            (1_000, "1.0K"),
            (999_949, "999.9K"),
            (999_950, "1.0M"),
            (999_999, "1.0M"),
            (1_000_000, "1.0M"),
            (1_500_000, "1.5M"),
            (999_949_999, "999.9M"),
            (999_950_000, "1.0B"),
            (1_000_000_000, "1.0B"),
            (1_500_000_000_000, "1.5T"),
        ] {
            assert_eq!(
                format_count(n),
                expected,
                "format_count({n}) should be {expected:?}"
            );
        }

        for (version, expected) in [
            ("0.1.0", "lore v0.1.0"),
            ("0.1.0-abc1234", "lore v0.1.0-abc1234"),
            // Stores written by older builds recorded a leading "v".
            ("v0.1.0", "lore v0.1.0"),
            ("v0.1.0-abc1234", "lore v0.1.0-abc1234"),
        ] {
            assert_eq!(
                format_version(version),
                expected,
                "format_version({version:?}) should be {expected:?}"
            );
        }

        for (secs, expected) in [
            (0.5f64, "0.5s"),
            (59.9, "59.9s"),
            (60.0, "1m 00s"),
            (3599.0, "59m 59s"),
            (3600.0, "1h 00m 00s"),
            (7261.0, "2h 01m 01s"),
        ] {
            assert_eq!(
                format_elapsed(Duration::from_secs_f64(secs)),
                expected,
                "format_elapsed({secs}s) should be {expected:?}"
            );
        }
    }
}
