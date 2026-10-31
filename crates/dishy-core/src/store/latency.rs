//! Persistent per-minute latency store.
//!
//! One line per completed minute holding that minute's histogram and sums (see
//! [`crate::latency`]). Mirrors the energy store: current-year detail, older
//! minutes folded into one row per month.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::calendar::{Calendar, LocalCalendar, now_ms};
use crate::jsonl::{
    JsonlResult, append_json_lines, ensure_parent_directory, read_json_lines,
    write_json_lines_atomically,
};
use crate::latency::{LatencyMinuteBucket, LatencyMonthBucket, fold_latency_minutes_to_months};

#[derive(Debug)]
pub struct LatencyStore {
    file_path: PathBuf,
    monthly_path: PathBuf,
    max_written_minute: i64,
}

impl LatencyStore {
    pub fn open(file_path: impl Into<PathBuf>) -> JsonlResult<Self> {
        Self::open_with(file_path, now_ms(), &LocalCalendar)
    }

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

    pub fn last_written_minute(&self) -> i64 {
        self.max_written_minute
    }

    pub fn append(&mut self, bucket: &LatencyMinuteBucket) -> JsonlResult<()> {
        append_json_lines(&self.file_path, std::slice::from_ref(bucket))?;
        if bucket.minute > self.max_written_minute {
            self.max_written_minute = bucket.minute;
        }
        Ok(())
    }

    pub fn read_range(
        &self,
        start_sec: i64,
        end_sec: i64,
    ) -> JsonlResult<Vec<LatencyMinuteBucket>> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter(|bucket| bucket.minute >= start_sec && bucket.minute < end_sec)
            .collect())
    }

    pub fn read_months(&self) -> JsonlResult<Vec<LatencyMonthBucket>> {
        let mut rows: Vec<LatencyMonthBucket> = read_json_lines(&self.monthly_path)?;
        rows.sort_by_key(|row| row.month);
        Ok(rows)
    }

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
        let folded = fold_latency_minutes_to_months(&not_yet_archived, |minute| {
            calendar.month_key_sec(minute)
        });
        let rows: Vec<_> = folded.into_values().collect();
        append_json_lines(&self.monthly_path, &rows)?;

        write_json_lines_atomically(&self.file_path, &kept)?;
        Ok(expired.len())
    }

    fn read_all(&self) -> JsonlResult<Vec<LatencyMinuteBucket>> {
        Ok(read_json_lines(&self.file_path)?)
    }
}

fn monthly_path(file_path: &Path) -> PathBuf {
    let stem = file_path
        .file_stem()
        .map_or_else(|| OsString::from("latency"), OsString::from);
    let mut name = stem;
    name.push("-monthly.ndjson");
    file_path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::calendar::UtcCalendar;
    use crate::latency::{LATENCY_BIN_COUNT, LatencySourceStats};

    const NOW_MS: i64 = 1_781_524_800_000;

    fn bucket(minute: i64) -> LatencyMinuteBucket {
        LatencyMinuteBucket {
            minute,
            samples: 60,
            dish: LatencySourceStats {
                count: 59,
                sum_ms: 1_180.0,
                sum_sq_ms: 23_600.0,
                min_ms: Some(15.0),
                max_ms: Some(30.0),
                bins: vec![0; LATENCY_BIN_COUNT],
            },
            router: LatencySourceStats::default(),
            drop_sum: 1.0,
            drop_count: 60,
        }
    }

    #[test]
    fn compaction_archives_past_years_and_keeps_histograms() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("latency.ndjson");
        let old = bucket(1_767_225_540);
        let current = bucket(1_787_227_260);
        write_json_lines_atomically(&path, &[old, current.clone()]).expect("seed");

        let store = LatencyStore::open_with(&path, NOW_MS, &UtcCalendar).expect("open");
        assert_eq!(store.last_written_minute(), current.minute);
        assert_eq!(store.read_range(0, i64::MAX).expect("range"), vec![current]);
        let months = store.read_months().expect("months");
        assert_eq!(months.len(), 1);
        assert_eq!(months[0].month, 1_764_547_200);
        assert_eq!(months[0].dish.count, 59);
        assert_eq!(months[0].dish.bins.len(), LATENCY_BIN_COUNT);

        let reopened = LatencyStore::open_with(&path, NOW_MS, &UtcCalendar).expect("reopen");
        assert_eq!(reopened.read_months().expect("months").len(), 1);
    }
}
