//! Where the observer stands, for satellite look angles.
//!
//! Port of `lib/observerLocation.ts`. SpaceX removed the local-API GPS
//! (`get_location`) for consumer plans, and this port adds no device read for
//! it, so the location is a saved choice: pasted coordinates or a city-level
//! IP guess. Look angles only need accuracy to about a kilometre.
//!
//! "Cleared" and "never set" are different intents and stay distinguishable.
//! An automatic source may fill in for someone who has never chosen, but must
//! never undo a deliberate clear.

use dishy_sky::ObserverLocation;

use crate::preferences::PreferencesStore;

/// Saving a location also un-clears it, so the two fields never drift apart.
pub fn save_location(
    store: &mut PreferencesStore,
    location: ObserverLocation,
) -> std::io::Result<()> {
    store.set(|preferences| {
        preferences.observer_location = Some(location);
        preferences.location_cleared = false;
    })
}

pub fn clear_saved_location(store: &mut PreferencesStore) -> std::io::Result<()> {
    store.set(|preferences| {
        preferences.observer_location = None;
        preferences.location_cleared = true;
    })
}

/// Parse pasted coordinates: "6.5244, 3.3792", "6.5244 3.3792", and the
/// degree-suffixed forms map apps copy. A trailing S or W after a number
/// makes it negative, so "33.9°S, 18.4°E" is the southern hemisphere.
pub fn parse_coordinate_text(coordinate_text: &str) -> Option<ObserverLocation> {
    let values = signed_numbers(coordinate_text);
    let [latitude_deg, longitude_deg, ..] = values[..] else {
        return None;
    };
    let location = ObserverLocation {
        latitude_deg,
        longitude_deg,
        altitude_m: 0.0,
    };
    location.is_valid().then_some(location)
}

/// Every `-?digits(.digits)?` in the text, in order, negated when the next
/// letter after it (skipping spaces and degree marks) is S or W.
fn signed_numbers(text: &str) -> Vec<f64> {
    let chars: Vec<char> = text.chars().collect();
    let mut values = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let negative =
            chars[index] == '-' && chars.get(index + 1).is_some_and(char::is_ascii_digit);
        if !(chars[index].is_ascii_digit() || negative) {
            index += 1;
            continue;
        }
        let start = index;
        index += usize::from(negative);
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }
        if chars.get(index) == Some(&'.') && chars.get(index + 1).is_some_and(char::is_ascii_digit)
        {
            index += 1;
            while index < chars.len() && chars[index].is_ascii_digit() {
                index += 1;
            }
        }
        let Ok(mut value) = chars[start..index]
            .iter()
            .collect::<String>()
            .parse::<f64>()
        else {
            continue;
        };
        let hemisphere = chars[index..]
            .iter()
            .find(|character| !matches!(character, ' ' | '°' | '\''));
        if matches!(hemisphere, Some('S' | 's' | 'W' | 'w')) {
            value = -value.abs();
        }
        values.push(value);
    }
    values
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    const SITE: ObserverLocation = ObserverLocation {
        latitude_deg: 4.8172,
        longitude_deg: 6.9679,
        altitude_m: 0.0,
    };

    fn parsed(text: &str) -> Option<(f64, f64)> {
        parse_coordinate_text(text).map(|location| (location.latitude_deg, location.longitude_deg))
    }

    // SPEC-CONST-006: pasted coordinates in the forms map apps produce, with
    // hemisphere letters honoured and out-of-range values refused.
    #[test]
    fn pasted_coordinates_parse_in_the_common_forms() {
        assert_eq!(parsed("6.5244, 3.3792"), Some((6.5244, 3.3792)));
        assert_eq!(parsed("6.5244 3.3792"), Some((6.5244, 3.3792)));
        assert_eq!(parsed("-33.9249, 18.4241"), Some((-33.9249, 18.4241)));
        assert_eq!(parsed("6.5244°N, 3.3792°E"), Some((6.5244, 3.3792)));
        assert_eq!(parsed("33.9249° S, 18.4241° E"), Some((-33.9249, 18.4241)));
        assert_eq!(parsed("40.7128N 74.0060W"), Some((40.7128, -74.006)));
        assert_eq!(parsed("6.5244"), None);
        assert_eq!(parsed("91.0, 0.0"), None);
        assert_eq!(parsed("0.0, -181.0"), None);
        assert_eq!(parsed("Lagos"), None);
    }

    // SPEC-CONST-006: a deliberate clear is remembered, and choosing a location
    // un-clears, so the next clear still reads as new. Both survive a reload
    // with the field names the TypeScript host uses.
    #[test]
    fn the_cleared_flag_is_kept_apart_from_never_set() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        let mut store = PreferencesStore::load(&path);
        assert_eq!(store.get().observer_location, None);
        assert!(!store.get().location_cleared);

        save_location(&mut store, SITE).expect("save");
        clear_saved_location(&mut store).expect("clear");
        let reloaded = PreferencesStore::load(&path);
        assert_eq!(reloaded.get().observer_location, None);
        assert!(reloaded.get().location_cleared);

        save_location(&mut store, SITE).expect("save again");
        let text = std::fs::read_to_string(&path).expect("file");
        assert!(text.contains("\"latitudeDeg\""), "{text}");
        let reloaded = PreferencesStore::load(&path);
        assert_eq!(reloaded.get().observer_location, Some(SITE));
        assert!(!reloaded.get().location_cleared);
    }
}
