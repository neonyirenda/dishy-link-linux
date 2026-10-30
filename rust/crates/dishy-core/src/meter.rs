//! Watch-only monthly per-device data limits.
//!
//! A limit is a device's own monthly rule (spec 0169, `meter_store`); this
//! module keeps the limit's value type and its progress against the
//! odometer's current-month totals. It never reads from or writes to a router.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientMonthlyLimit {
    pub client_key: String,
    pub limit_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyLimitProgress {
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub reached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MonthlyLimitError {
    #[error("client key must not be empty")]
    EmptyClientKey,
    #[error("monthly limit must be greater than zero")]
    ZeroLimit,
}

impl ClientMonthlyLimit {
    pub fn new(client_key: impl Into<String>, limit_bytes: u64) -> Result<Self, MonthlyLimitError> {
        let client_key = client_key.into();
        if client_key.trim().is_empty() {
            return Err(MonthlyLimitError::EmptyClientKey);
        }
        if limit_bytes == 0 {
            return Err(MonthlyLimitError::ZeroLimit);
        }
        Ok(Self {
            client_key,
            limit_bytes,
        })
    }
}

pub fn limit_progress(
    limit: &ClientMonthlyLimit,
    rx_bytes: u64,
    tx_bytes: u64,
) -> MonthlyLimitProgress {
    let used_bytes = rx_bytes.saturating_add(tx_bytes);
    MonthlyLimitProgress {
        used_bytes,
        limit_bytes: limit.limit_bytes,
        reached: used_bytes >= limit.limit_bytes,
    }
}
