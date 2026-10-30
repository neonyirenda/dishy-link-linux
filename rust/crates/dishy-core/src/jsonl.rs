//! Durability primitives shared by every recorder store.
//!
//! Each store keeps a JSON Lines log: one JSON object per line, appended as
//! facts arrive and rewritten whole when rows expire. The format predates the
//! Rust port, so the `.ndjson` names stay.
//!
//! Read and write policy lives here, not in each store, so a torn-write fix
//! reaches all of them at once.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum JsonlError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type JsonlResult<T> = Result<T, JsonlError>;

/// Create the directory a store's file lives in. Called from constructors, so a
/// first run on a clean machine writes into a path that already exists.
pub fn ensure_parent_directory(file_path: &Path) -> io::Result<()> {
    match file_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// Every parseable row of a JSON Lines log, in file order.
///
/// A missing file is an empty log, not an error: that is a store which has not
/// recorded anything yet. A line that does not parse is skipped rather than
/// fatal — a process killed mid-append leaves a torn final line, and discarding
/// a whole history over one bad tail would turn a crash into data loss.
pub fn read_json_lines<Row: DeserializeOwned>(file_path: &Path) -> io::Result<Vec<Row>> {
    read_json_lines_with(file_path, |_| true)
}

/// Read with a guard for stores that can recognise a row too malformed to use
/// even though it parsed as JSON.
pub fn read_json_lines_with<Row, F>(file_path: &Path, mut is_valid: F) -> io::Result<Vec<Row>>
where
    Row: DeserializeOwned,
    F: FnMut(&Row) -> bool,
{
    let text = match fs::read_to_string(file_path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut rows = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        if let Ok(row) = serde_json::from_str::<Row>(line)
            && is_valid(&row)
        {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Replace a log with exactly these rows, atomically.
///
/// The write goes to a sibling `.tmp` and is renamed over the target, because
/// rename within a filesystem is atomic: a reader sees either the whole old file
/// or the whole new one, never a half-written log.
pub fn write_json_lines_atomically<Row: Serialize>(
    file_path: &Path,
    rows: &[Row],
) -> JsonlResult<()> {
    let mut body = String::new();
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            body.push('\n');
        }
        body.push_str(&serde_json::to_string(row)?);
    }
    if !body.is_empty() {
        body.push('\n');
    }
    replace_file_atomically(file_path, &body)?;
    Ok(())
}

/// Add rows to the end of a log. Nothing to add is not a write: an idle poll
/// should not touch the disk at all.
pub fn append_json_lines<Row: Serialize>(file_path: &Path, rows: &[Row]) -> JsonlResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    ensure_parent_directory(file_path)?;
    let mut body = String::new();
    for row in rows {
        body.push_str(&serde_json::to_string(row)?);
        body.push('\n');
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)?;
    file.write_all(body.as_bytes())?;
    Ok(())
}

/// The same atomic replace for stores holding one JSON document rather than a
/// row-per-line log — the telemetry snapshot and the odometers.
pub fn write_json_atomically<Row: Serialize>(file_path: &Path, value: &Row) -> JsonlResult<()> {
    replace_file_atomically(file_path, &serde_json::to_string(value)?)?;
    Ok(())
}

fn temporary_path(file_path: &Path) -> PathBuf {
    let mut name = OsString::from(file_path.as_os_str());
    name.push(".tmp");
    PathBuf::from(name)
}

fn replace_file_atomically(file_path: &Path, contents: &str) -> io::Result<()> {
    ensure_parent_directory(file_path)?;
    let temporary = temporary_path(file_path);
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, file_path)
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Row {
        minute: i64,
        value: f64,
    }

    // SPEC-RECORDER-004: one JSON object per line, with the existing field names.
    #[test]
    fn rows_round_trip_as_one_object_per_line() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("rows.ndjson");
        let rows = vec![
            Row {
                minute: 60,
                value: 1.5,
            },
            Row {
                minute: 120,
                value: 2.5,
            },
        ];
        write_json_lines_atomically(&path, &rows).expect("write");
        let text = fs::read_to_string(&path).expect("text");
        assert_eq!(text.lines().count(), 2);
        assert_eq!(read_json_lines::<Row>(&path).expect("read"), rows);
        assert!(!temporary_path(&path).exists());
    }

    // SPEC-RECORDER-005: a torn final line is discarded, not fatal.
    #[test]
    fn a_torn_final_line_is_skipped() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("rows.ndjson");
        fs::write(
            &path,
            "{\"minute\":60,\"value\":1.5}\n{\"minute\":120,\"val",
        )
        .expect("fixture");
        let rows: Vec<Row> = read_json_lines(&path).expect("read");
        assert_eq!(
            rows,
            vec![Row {
                minute: 60,
                value: 1.5
            }]
        );
    }

    #[test]
    fn a_missing_log_is_empty_and_an_empty_append_writes_nothing() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("absent.ndjson");
        assert!(read_json_lines::<Row>(&path).expect("read").is_empty());
        append_json_lines::<Row>(&path, &[]).expect("append");
        assert!(!path.exists());
    }

    #[test]
    fn append_adds_rows_without_rewriting() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("rows.ndjson");
        append_json_lines(
            &path,
            &[Row {
                minute: 60,
                value: 1.5,
            }],
        )
        .expect("first append");
        append_json_lines(
            &path,
            &[Row {
                minute: 120,
                value: 2.5,
            }],
        )
        .expect("second append");
        assert_eq!(read_json_lines::<Row>(&path).expect("read").len(), 2);
    }
}
