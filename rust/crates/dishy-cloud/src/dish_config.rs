//! Typed dish configuration writes over the authenticated device gateway.

use serde_json::{Value, json};

use crate::CloudError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnowMeltMode {
    Auto,
    AlwaysOn,
    AlwaysOff,
}

impl SnowMeltMode {
    fn proto_name(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::AlwaysOn => "ALWAYS_ON",
            Self::AlwaysOff => "ALWAYS_OFF",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationRequestMode {
    None,
    Local,
}

impl LocationRequestMode {
    fn proto_name(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Local => "LOCAL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelDishMode {
    TiltLikeNormal,
    ForceLevel,
}

impl LevelDishMode {
    fn proto_name(self) -> &'static str {
        match self {
            Self::TiltLikeNormal => "TILT_LIKE_NORMAL",
            Self::ForceLevel => "FORCE_LEVEL",
        }
    }
}

/// Only fields present in this update are applied; omitted settings are never
/// inferred from stale status and are never sent with a false apply flag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DishConfigUpdate {
    pub snow_melt_mode: Option<SnowMeltMode>,
    pub location_request_mode: Option<LocationRequestMode>,
    pub level_dish_mode: Option<LevelDishMode>,
    pub power_save_start_minutes: Option<u32>,
    pub power_save_duration_minutes: Option<u32>,
    pub power_save_mode: Option<bool>,
    pub swupdate_reboot_hour: Option<u32>,
    pub swupdate_three_day_deferral_enabled: Option<bool>,
}

pub fn validate(update: &DishConfigUpdate) -> Result<(), CloudError> {
    if update == &DishConfigUpdate::default() {
        return Err(CloudError::GrpcProtocol(
            "dish config update contains no changes".into(),
        ));
    }
    if update
        .power_save_start_minutes
        .is_some_and(|minutes| minutes >= 1_440)
    {
        return Err(CloudError::GrpcProtocol(
            "power-save start must be between 0 and 1439 minutes".into(),
        ));
    }
    if update
        .power_save_duration_minutes
        .is_some_and(|minutes| minutes == 0 || minutes > 1_440)
    {
        return Err(CloudError::GrpcProtocol(
            "power-save duration must be between 1 and 1440 minutes".into(),
        ));
    }
    if update
        .swupdate_reboot_hour
        .is_some_and(|hour| ![3, 9, 15, 21].contains(&hour))
    {
        return Err(CloudError::GrpcProtocol(
            "update reboot hour must be 3, 9, 15, or 21".into(),
        ));
    }
    Ok(())
}

pub fn build_request(target_id: &str, update: DishConfigUpdate) -> Result<Value, CloudError> {
    validate(&update)?;
    if !target_id.starts_with("ut") {
        return Err(CloudError::GrpcProtocol("invalid dish target id".into()));
    }
    let mut config = serde_json::Map::new();
    macro_rules! set_field {
        ($field:ident, $json_name:literal, $apply:literal, $convert:expr) => {
            if let Some(value) = update.$field {
                config.insert($json_name.into(), $convert(value));
                config.insert($apply.into(), Value::Bool(true));
            }
        };
    }
    set_field!(
        snow_melt_mode,
        "snowMeltMode",
        "applySnowMeltMode",
        |value: SnowMeltMode| Value::String(value.proto_name().into())
    );
    set_field!(
        location_request_mode,
        "locationRequestMode",
        "applyLocationRequestMode",
        |value: LocationRequestMode| Value::String(value.proto_name().into())
    );
    set_field!(
        level_dish_mode,
        "levelDishMode",
        "applyLevelDishMode",
        |value: LevelDishMode| Value::String(value.proto_name().into())
    );
    set_field!(
        power_save_start_minutes,
        "powerSaveStartMinutes",
        "applyPowerSaveStartMinutes",
        |value: u32| json!(value)
    );
    set_field!(
        power_save_duration_minutes,
        "powerSaveDurationMinutes",
        "applyPowerSaveDurationMinutes",
        |value: u32| json!(value)
    );
    set_field!(
        power_save_mode,
        "powerSaveMode",
        "applyPowerSaveMode",
        |value: bool| json!(value)
    );
    set_field!(
        swupdate_reboot_hour,
        "swupdateRebootHour",
        "applySwupdateRebootHour",
        |value: u32| json!(value)
    );
    set_field!(
        swupdate_three_day_deferral_enabled,
        "swupdateThreeDayDeferralEnabled",
        "applySwupdateThreeDayDeferralEnabled",
        |value: bool| json!(value)
    );
    Ok(json!({ "targetId": target_id, "dishSetConfig": { "dishConfig": config } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-DISHCFG-001: only the requested fields and their true apply flags
    // are present in a partial update.
    #[test]
    fn partial_update_contains_only_changed_fields_and_apply_flags() {
        let request = build_request(
            "ut-dish-123",
            DishConfigUpdate {
                snow_melt_mode: Some(SnowMeltMode::AlwaysOn),
                power_save_start_minutes: Some(90),
                power_save_mode: Some(false),
                ..DishConfigUpdate::default()
            },
        )
        .unwrap();
        assert_eq!(
            request,
            json!({"targetId":"ut-dish-123", "dishSetConfig":{"dishConfig":{
                "snowMeltMode":"ALWAYS_ON", "applySnowMeltMode":true,
                "powerSaveStartMinutes":90, "applyPowerSaveStartMinutes":true,
                "powerSaveMode":false, "applyPowerSaveMode":true
            }}})
        );
    }

    // SPEC-DISHCFG-002: invalid and empty changes are refused before encoding.
    #[test]
    fn empty_and_out_of_range_updates_are_rejected() {
        assert!(validate(&DishConfigUpdate::default()).is_err());
        for update in [
            DishConfigUpdate {
                power_save_start_minutes: Some(1_440),
                ..DishConfigUpdate::default()
            },
            DishConfigUpdate {
                power_save_duration_minutes: Some(0),
                ..DishConfigUpdate::default()
            },
            DishConfigUpdate {
                power_save_duration_minutes: Some(1_441),
                ..DishConfigUpdate::default()
            },
            DishConfigUpdate {
                swupdate_reboot_hour: Some(12),
                ..DishConfigUpdate::default()
            },
        ] {
            assert!(validate(&update).is_err());
        }
    }

    // SPEC-DISHCFG-002: the only accepted software update windows are the four
    // offered by the Electron controls.
    #[test]
    fn update_reboot_hour_accepts_the_offered_windows() {
        for hour in [3, 9, 15, 21] {
            assert!(
                validate(&DishConfigUpdate {
                    swupdate_reboot_hour: Some(hour),
                    ..DishConfigUpdate::default()
                })
                .is_ok()
            );
        }
    }
}
