//! Persistent per-minute energy store.
//!
//! One line per completed wall-clock minute: `{ minute, wattSeconds, samples,
//! downlinkBits, uplinkBits }`. `minute` is epoch seconds at the minute's start;
//! `samples` counts the per-second readings actually collected, so a gap shows
//! up as low coverage rather than fabricated energy.
//!
//! Detail is kept for the current calendar year. Older minutes are folded into
//! one row per month before they go.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::calendar::{Calendar, LocalCalendar, now_ms};
use crate::energy::{MinuteBucket, MonthBucket, fold_minutes_to_months};
use crate::jsonl::{
    JsonlResult, append_json_lines, ensure_parent_directory, read_json_lines,
    write_json_lines_atomically,
};

#[derive(Debug)]
pub struct EnergyStore {
    file_path: PathBuf,
    monthly_path: PathBuf,
    max_written_minute: i64,
}

impl EnergyStore {
    pub fn open(file_path: impl Into<PathBuf>) -> JsonlResult<Self> {
        Self::open_with(file_path, now_ms(), &LocalCalendar)
    }

    /// Open with an explicit clock and calendar. Tests and cross-language
    /// fixtures pin both so a run in another timezone cannot change the result.
    pub fn open_with(
        file_path: impl Into<PathBuf>,
        now_ms: i64,
        calendar: &dyn Calendar,
    ) -> JsonlResult<Self> {
        let file_path = file_path.into();
        ensure_parent_directory(&file_path)?;
        let mut store = Self {
            monthly_path: monthly_path(&file_path),
            file_path,
            max_written_minute: -1,
        };
        store.compact_with(now_ms, calendar)?;
        for bucket in store.read_all()? {
            if bucket.minute > store.max_written_minute {
                store.max_written_minute = bucket.minute;
            }
        }
        Ok(store)
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn monthly_path(&self) -> &Path {
        &self.monthly_path
    }

    /// Newest minute already persisted; incoming samples at or below this are
    /// ignored to avoid double-counting on restart.
    pub fn last_written_minute(&self) -> i64 {
        self.max_written_minute
    }

    pub fn append(&mut self, bucket: &MinuteBucket) -> JsonlResult<()> {
        append_json_lines(&self.file_path, std::slice::from_ref(bucket))?;
        if bucket.minute > self.max_written_minute {
            self.max_written_minute = bucket.minute;
        }
        Ok(())
    }

    /// Persisted buckets whose minute falls in `[start_sec, end_sec)`.
    pub fn read_range(&self, start_sec: i64, end_sec: i64) -> JsonlResult<Vec<MinuteBucket>> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter(|bucket| bucket.minute >= start_sec && bucket.minute < end_sec)
            .collect())
    }

    /// Archived monthly summaries, oldest first.
    pub fn read_months(&self) -> JsonlResult<Vec<MonthBucket>> {
        let mut rows: Vec<MonthBucket> = read_json_lines(&self.monthly_path)?;
        rows.sort_by_key(|row| row.month);
        Ok(rows)
    }

    /// Keep per-minute detail for the current calendar year; fold everything
    /// older into monthly rows first, so a year rollover summarises last year
    /// rather than erasing it. Idempotent: an archived month is not written
    /// twice.
    pub fn compact_with(&self, now_ms: i64, calendar: &dyn Calendar) -> JsonlResult<usize> {
        let all = self.read_all()?;
        let cutoff = calendar.year_start_sec(now_ms);
        let kept: Vec<_> = all
            .iter()
            .filter(|bucket| bucket.minute >= cutoff)
            .cloned()
            .collect();
        let expired: Vec<_> = all
            .iter()
            .filter(|bucket| bucket.minute < cutoff)
            .cloned()
            .collect();
        if expired.is_empty() {
            return Ok(0);
        }

        // Already-archived months are excluded before folding, not after: this
        // log is append-only, so a month written twice would read back as two
        // rows instead of one.
        let archived: HashSet<i64> = self
            .read_months()?
            .into_iter()
            .map(|row| row.month)
            .collect();
        let not_yet_archived: Vec<_> = expired
            .iter()
            .filter(|bucket| !archived.contains(&calendar.month_key_sec(bucket.minute)))
            .cloned()
            .collect();
        let folded =
            fold_minutes_to_months(&not_yet_archived, |minute| calendar.month_key_sec(minute));
        let rows: Vec<_> = folded.into_values().collect();
        append_json_lines(&self.monthly_path, &rows)?;

        write_json_lines_atomically(&self.file_path, &kept)?;
        Ok(expired.len())
    }

    fn read_all(&self) -> JsonlResult<Vec<MinuteBucket>> {
        Ok(read_json_lines(&self.file_path)?)
    }
}

fn monthly_path(file_path: &Path) -> PathBuf {
    let stem = file_path
        .file_stem()
        .map_or_else(|| OsString::from("energy"), OsString::from);
    let mut name = stem;
    name.push("-monthly.ndjson");
    file_path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::calendar::UtcCalendar;

    // 2026-06-15T12:00:00Z
    const NOW_MS: i64 = 1_781_524_800_000;

    fn bucket(minute: i64) -> MinuteBucket {
        MinuteBucket {
            minute,
            watt_seconds: 60.0,
            samples: 60,
            downlink_bits: 1_000.0,
            uplink_bits: 100.0,
        }
    }

    #[test]
    fn compaction_archives_past_years_once_and_keeps_this_year() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("energy.ndjson");
        let old = bucket(1_767_225_540); // 2025-12-31 23:59Z
        let current = bucket(1_787_227_260); // 2026-08-20 10:01Z
        write_json_lines_atomically(&path, &[old.clone(), current.clone()]).expect("seed");

        let store = EnergyStore::open_with(&path, NOW_MS, &UtcCalendar).expect("open");
        assert_eq!(store.last_written_minute(), current.minute);
        assert_eq!(store.read_range(0, i64::MAX).expect("read"), vec![current]);
        let months = store.read_months().expect("months");
        assert_eq!(months.len(), 1);
        assert_eq!(months[0].month, 1_764_547_200); // 2025-12-01
        assert_eq!(months[0].watt_seconds, 60.0);

        // Re-opening must not append the same month again.
        let reopened = EnergyStore::open_with(&path, NOW_MS, &UtcCalendar).expect("reopen");
        assert_eq!(reopened.read_months().expect("months").len(), 1);
    }

    #[test]
    fn appending_updates_the_write_floor() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("energy.ndjson");
        let mut store = EnergyStore::open_with(&path, NOW_MS, &UtcCalendar).expect("open");
        assert_eq!(store.last_written_minute(), -1);
        store.append(&bucket(1_787_227_260)).expect("append");
        assert_eq!(store.last_written_minute(), 1_787_227_260);
        assert_eq!(
            store
                .read_range(1_787_227_260, 1_787_227_261)
                .expect("range"),
            vec![bucket(1_787_227_260)]
        );
    }
}
