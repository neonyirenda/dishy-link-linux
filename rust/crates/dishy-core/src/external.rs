//! Public internet reads the sky view needs: the Starlink element catalogue
//! and a city-level location guess from the public IP.
//!
//! Neither touches the dish or the router. They live here rather than in the
//! GTK crate so every network call stays in `dishy-core` or `dishy-cloud`
//! (SPEC-ARCH-002), and rather than in `dishy-sky` so that crate stays pure.

use std::time::Duration;

use thiserror::Error;

use crate::http_body::{BodyError, MAX_RESPONSE_BYTES, read_capped};

/// CelesTrak's supplemental Starlink catalogue, the same file Electron reads.
pub const CELESTRAK_BASE_URL: &str = "https://celestrak.org";
pub const STARLINK_TLE_PATH: &str =
    "/NORAD/elements/supplemental/sup-gp.php?FILE=starlink&FORMAT=tle";

/// City-level geolocation. On Starlink it resolves toward the point of
/// presence, so it can be a few hundred km off: fine for the sky view.
pub const GEOJS_IP_LOCATION_URL: &str = "https://get.geojs.io/v1/ip/geo.json";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExternalFetchError {
    /// The request could not be made: offline, DNS, timeout, connection dropped.
    #[error("request could not be made: {0}")]
    Offline(String),
    /// The server answered with something other than what was asked for.
    #[error("unexpected response: {0}")]
    Source(String),
}

/// The client for these reads. It owns the HTTP stack so callers, the GTK
/// crate included, never build one themselves.
#[derive(Debug, Clone)]
pub struct ExternalClient {
    http: reqwest::Client,
    celestrak_base_url: String,
    ip_location_url: String,
}

impl Default for ExternalClient {
    fn default() -> Self {
        Self::with_urls(CELESTRAK_BASE_URL, GEOJS_IP_LOCATION_URL)
    }
}

impl ExternalClient {
    /// The catalogue is about 2 MB, so the timeout is generous compared with
    /// the LAN's.
    pub fn with_urls(celestrak_base_url: &str, ip_location_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            http,
            celestrak_base_url: celestrak_base_url.trim_end_matches('/').to_string(),
            ip_location_url: ip_location_url.to_string(),
        }
    }

    /// Fetch the catalogue text. The caller parses it. A reply that does not
    /// look like a three-line element file is a source error, not an empty
    /// catalogue.
    pub async fn starlink_tles(&self) -> Result<String, ExternalFetchError> {
        let text = self
            .get_text(&format!("{}{STARLINK_TLE_PATH}", self.celestrak_base_url))
            .await?;
        if !text.contains("\n1 ") {
            return Err(ExternalFetchError::Source(
                "not a three-line element file".into(),
            ));
        }
        Ok(text)
    }

    /// Latitude and longitude in degrees from the public IP.
    pub async fn ip_location(&self) -> Result<(f64, f64), ExternalFetchError> {
        let body = self.get_text(&self.ip_location_url).await?;
        parse_geojs(&body).ok_or_else(|| ExternalFetchError::Source("no coordinates".into()))
    }

    async fn get_text(&self, url: &str) -> Result<String, ExternalFetchError> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| ExternalFetchError::Offline(error.to_string()))?;
        if !response.status().is_success() {
            return Err(ExternalFetchError::Source(format!(
                "HTTP {}",
                response.status()
            )));
        }
        // SPEC-BODY-001
        let body =
            read_capped(response, MAX_RESPONSE_BYTES)
                .await
                .map_err(|error| match error {
                    BodyError::Transport(error) => ExternalFetchError::Offline(error.to_string()),
                    too_large => ExternalFetchError::Source(too_large.to_string()),
                })?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

/// geojs sends the coordinates as strings. Accept numbers too, and refuse
/// anything that is missing, not finite, or out of range.
pub fn parse_geojs(body: &str) -> Option<(f64, f64)> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let coordinate = |key: &str| -> Option<f64> {
        match value.get(key)? {
            serde_json::Value::String(text) => text.trim().parse().ok(),
            serde_json::Value::Number(number) => number.as_f64(),
            _ => None,
        }
    };
    let latitude = coordinate("latitude")?;
    let longitude = coordinate("longitude")?;
    (latitude.is_finite()
        && longitude.is_finite()
        && latitude.abs() <= 90.0
        && longitude.abs() <= 180.0)
        .then_some((latitude, longitude))
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const TLE: &str = "STARLINK-1007\n\
1 44713U 19074A   26300.50000000  .00001000  00000-0  80000-4 0  9990\n\
2 44713  53.0540 100.0000 0001400  90.0000 270.0000 15.06400000 10000\n";

    // SPEC-CONST-005: the catalogue fetch names which side failed, and treats
    // a non-catalogue reply as a source error.
    #[tokio::test]
    async fn the_catalogue_fetch_distinguishes_offline_from_a_bad_source() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/NORAD/elements/supplemental/sup-gp.php"))
            .and(query_param("FILE", "starlink"))
            .respond_with(ResponseTemplate::new(200).set_body_string(TLE))
            .mount(&server)
            .await;
        let catalogue = |base: &str| ExternalClient::with_urls(base, "http://127.0.0.1:1");
        assert_eq!(
            catalogue(&server.uri()).starlink_tles().await.as_deref(),
            Ok(TLE)
        );

        let busy = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>busy</html>"))
            .mount(&busy)
            .await;
        assert!(matches!(
            catalogue(&busy.uri()).starlink_tles().await,
            Err(ExternalFetchError::Source(_))
        ));

        let failing = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&failing)
            .await;
        assert!(matches!(
            catalogue(&failing.uri()).starlink_tles().await,
            Err(ExternalFetchError::Source(_))
        ));

        assert!(matches!(
            catalogue("http://127.0.0.1:1").starlink_tles().await,
            Err(ExternalFetchError::Offline(_))
        ));
    }

    // SPEC-CONST-006: the IP lookup reads geojs's string coordinates and
    // refuses a reply without usable ones.
    #[tokio::test]
    async fn the_ip_lookup_reads_string_coordinates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/geo.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"latitude":"6.4474","longitude":"3.3903"}"#),
            )
            .mount(&server)
            .await;
        let url = format!("{}/geo.json", server.uri());
        let client = ExternalClient::with_urls("http://127.0.0.1:1", &url);
        assert_eq!(client.ip_location().await, Ok((6.4474, 3.3903)));

        assert_eq!(
            parse_geojs(r#"{"latitude":1.5,"longitude":-2}"#),
            Some((1.5, -2.0))
        );
        assert_eq!(parse_geojs(r#"{"latitude":"nan","longitude":"0"}"#), None);
        assert_eq!(parse_geojs(r#"{"latitude":"91","longitude":"0"}"#), None);
        assert_eq!(parse_geojs(r#"{"country":"NG"}"#), None);
        assert_eq!(parse_geojs("not json"), None);
    }
}
