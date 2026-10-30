//! The grpc-web transport the dish and router speak on their LAN ports.
//!
//! Port 9201 on the dish and 9001 on the router are grpc-web over HTTP/1.1, not
//! native gRPC. A unary call is one POST whose body is a single message frame,
//! answered by a message frame followed by a trailers frame. Errors arrive
//! either as trailers or as `grpc-status` on the HTTP headers.
//!
//! Two device quirks are load-bearing, and both are handled by *not* doing
//! something: the dish only answers CORS preflights for its own origin, and it
//! returns an empty 200 to requests carrying an unrecognised `Referer`. So this
//! client sends neither `Origin` nor `Referer`. `reqwest` does not add them.

use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue};

use crate::http_body::{BodyError, MAX_RESPONSE_BYTES, read_capped};

/// Payload of a protobuf message.
const MESSAGE_FRAME: u8 = 0x00;
/// Payload is grpc-web trailers, ASCII key/value pairs.
const TRAILERS_FRAME: u8 = 0x80;

/// Bytes each call costs, for the diagnostics panel and the payload budgets in
/// `LOCAL-API.md`. Counts the framed body, the headers we set, the URL, and a
/// status line. TCP/IP framing below the socket is not visible here and is a
/// known few-percent undercount.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrpcCallBytes {
    pub request_bytes: usize,
    pub response_bytes: usize,
}

/// gRPC status codes this client needs to name. The full enum lives in the
/// protocol; only the ones the app branches on are useful here.
pub mod status {
    pub const OK: u16 = 0;
    pub const UNKNOWN: u16 = 2;
    pub const PERMISSION_DENIED: u16 = 7;
    pub const UNIMPLEMENTED: u16 = 12;
    pub const UNAUTHENTICATED: u16 = 16;
}

#[derive(Debug, thiserror::Error)]
pub enum GrpcWebError {
    #[error("grpc-web call failed (status {status}): {message}")]
    Status { status: u16, message: String },
    #[error("grpc-web response was malformed: {0}")]
    Protocol(String),
    #[error("HTTP request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

impl From<BodyError> for GrpcWebError {
    fn from(error: BodyError) -> Self {
        match error {
            BodyError::Transport(error) => Self::Transport(error),
            too_large @ BodyError::TooLarge { .. } => Self::Protocol(too_large.to_string()),
        }
    }
}

impl GrpcWebError {
    /// The gRPC status code, when the failure had one. Transport and framing
    /// failures are `UNKNOWN`, matching the TypeScript client.
    pub fn grpc_status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            Self::Protocol(_) | Self::Transport(_) => None,
        }
    }

    pub fn is_unimplemented(&self) -> bool {
        self.grpc_status() == Some(status::UNIMPLEMENTED)
    }

    pub fn is_permission_denied(&self) -> bool {
        self.grpc_status() == Some(status::PERMISSION_DENIED)
    }
}

type ByteReporter = std::sync::Arc<dyn Fn(GrpcCallBytes) + Send + Sync>;

/// A reusable HTTP client for grpc-web unary calls.
#[derive(Clone)]
pub struct GrpcWebClient {
    http: reqwest::Client,
    reporter: Option<ByteReporter>,
}

impl std::fmt::Debug for GrpcWebClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcWebClient")
            .field("reporter", &self.reporter.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for GrpcWebClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GrpcWebClient {
    /// A client with the LAN's tight timeouts: the boxes answer in tens of
    /// milliseconds, and a stalled call must not hold the poller open.
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client with static configuration");
        Self {
            http,
            reporter: None,
        }
    }

    pub fn with_http(http: reqwest::Client) -> Self {
        Self {
            http,
            reporter: None,
        }
    }

    /// Report every call's wire cost. Used by the diagnostics panel.
    pub fn with_reporter(mut self, reporter: ByteReporter) -> Self {
        self.reporter = Some(reporter);
        self
    }

    /// Perform a unary call and return the response message bytes.
    pub async fn call(&self, handle_url: &str, request: &[u8]) -> Result<Bytes, GrpcWebError> {
        self.call_with_timeout(handle_url, request, None).await
    }

    /// As [`Self::call`], with a per-call deadline. `None` uses the client's
    /// own timeout.
    pub async fn call_with_timeout(
        &self,
        handle_url: &str,
        request: &[u8],
        timeout: Option<Duration>,
    ) -> Result<Bytes, GrpcWebError> {
        let request_frame = encode_frame(MESSAGE_FRAME, request);
        let request_headers = request_headers();
        let request_header_bytes = header_bytes(&request_headers);

        let mut builder = self
            .http
            .post(handle_url)
            .headers(request_headers)
            .body(request_frame.clone());
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }

        let response = builder.send().await?;

        let report = |response_body_bytes: usize, response_headers: &HeaderMap| {
            if let Some(reporter) = &self.reporter {
                reporter(GrpcCallBytes {
                    request_bytes: request_frame.len()
                        + request_header_bytes
                        + handle_url.len()
                        + 17,
                    response_bytes: response_body_bytes + header_bytes(response_headers) + 17,
                });
            }
        };

        // Trailers-only responses carry the status in HTTP headers. Check that
        // before the HTTP status: a 200 with status 7 is a normal policy
        // refusal, not a transport failure.
        let header_status = response
            .headers()
            .get("grpc-status")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u16>().ok());
        if let Some(status) = header_status
            && status != status::OK
        {
            let message = response
                .headers()
                .get("grpc-message")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unknown error")
                .to_string();
            report(0, response.headers());
            return Err(GrpcWebError::Status { status, message });
        }

        let http_status = response.status();
        if !http_status.is_success() {
            report(0, response.headers());
            let status = if http_status == reqwest::StatusCode::UNAUTHORIZED
                || http_status == reqwest::StatusCode::FORBIDDEN
            {
                status::UNAUTHENTICATED
            } else {
                status::UNKNOWN
            };
            return Err(GrpcWebError::Status {
                status,
                message: format!("HTTP {http_status}"),
            });
        }

        let response_headers = response.headers().clone();
        // SPEC-BODY-001: whatever answers on the LAN decides the body size.
        let body = read_capped(response, MAX_RESPONSE_BYTES).await?;
        report(body.len(), &response_headers);

        parse_response_body(&body)
    }
}

fn request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/grpc-web+proto"),
    );
    headers.insert("x-grpc-web", HeaderValue::from_static("1"));
    headers.insert(
        "accept",
        HeaderValue::from_static("application/grpc-web+proto"),
    );
    headers
}

fn header_bytes(headers: &HeaderMap) -> usize {
    headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 4)
        .sum()
}

/// Wrap a message in a grpc-web frame.
pub fn encode_frame(flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(flag);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Extract the message from a unary response body.
///
/// The last message frame wins, matching the TypeScript client: a device that
/// streams progress frames before its answer would otherwise return progress.
pub fn parse_response_body(body: &[u8]) -> Result<Bytes, GrpcWebError> {
    let mut message: Option<Bytes> = None;
    let mut offset = 0usize;

    while offset + 5 <= body.len() {
        let flag = body[offset];
        let length = u32::from_be_bytes([
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
            body[offset + 4],
        ]) as usize;
        let start = offset + 5;
        let end = start
            .checked_add(length)
            .ok_or_else(|| GrpcWebError::Protocol("frame length overflow".into()))?;
        let payload = body
            .get(start..end)
            .ok_or_else(|| GrpcWebError::Protocol("truncated frame".into()))?;
        offset = end;

        if flag == MESSAGE_FRAME {
            message = Some(Bytes::copy_from_slice(payload));
        } else if flag & TRAILERS_FRAME != 0 {
            let text = std::str::from_utf8(payload)
                .map_err(|_| GrpcWebError::Protocol("trailers were not UTF-8".into()))?;
            let (status, message_text) = parse_trailers(text);
            if status != status::OK {
                return Err(GrpcWebError::Status {
                    status,
                    message: message_text,
                });
            }
        }
    }

    if offset != body.len() {
        return Err(GrpcWebError::Protocol(
            "trailing bytes after the last frame".into(),
        ));
    }

    message.ok_or_else(|| GrpcWebError::Protocol("response contained no message frame".into()))
}

/// Parse grpc-web trailers into `(status, message)`.
pub fn parse_trailers(text: &str) -> (u16, String) {
    let mut status = status::OK;
    let mut message = String::new();
    for line in text.split("\r\n") {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "grpc-status" => status = value.trim().parse().unwrap_or(status::UNKNOWN),
            "grpc-message" => message = percent_decode(value.trim()),
            _ => {}
        }
    }
    (status, message)
}

/// gRPC encodes the message as percent-encoded UTF-8. The common case has no
/// escapes, so an owned copy is cheaper than a decoder ceremony.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-TRANSPORT-001: a frame is flag + big-endian u32 length + payload.
    #[test]
    fn frame_layout_is_big_endian_length_prefixed() {
        assert_eq!(
            encode_frame(MESSAGE_FRAME, &[1, 2, 3]),
            vec![0, 0, 0, 0, 3, 1, 2, 3]
        );
    }

    // SPEC-TRANSPORT-001: message and trailers frames are separated correctly.
    #[test]
    fn response_parser_returns_the_last_message() {
        let mut body = encode_frame(MESSAGE_FRAME, &[0xAA]);
        body.extend(encode_frame(MESSAGE_FRAME, &[0xBB, 0xCC]));
        body.extend(encode_frame(TRAILERS_FRAME, b"grpc-status:0\r\n"));
        assert_eq!(parse_response_body(&body).unwrap().as_ref(), &[0xBB, 0xCC]);
    }

    // SPEC-TRANSPORT-003: non-zero trailers are an error carrying the message.
    #[test]
    fn non_zero_trailers_are_an_error() {
        let mut body = encode_frame(MESSAGE_FRAME, &[0xAA]);
        body.extend(encode_frame(
            TRAILERS_FRAME,
            b"grpc-status:7\r\ngrpc-message:Disabled%20due%20to%20policy\r\n",
        ));
        let error = parse_response_body(&body).unwrap_err();
        assert_eq!(error.grpc_status(), Some(status::PERMISSION_DENIED));
        assert!(error.to_string().contains("Disabled due to policy"));
    }

    // SPEC-TRANSPORT-001: truncated or trailing bytes are refused, not guessed.
    #[test]
    fn malformed_frames_are_refused() {
        assert!(parse_response_body(&[0, 0, 0]).is_err());
        assert!(parse_response_body(&[0, 0, 0, 0, 2, 1]).is_err());
        let mut body = encode_frame(MESSAGE_FRAME, &[1]);
        body.push(0xFF);
        assert!(parse_response_body(&body).is_err());
        assert!(parse_response_body(&[]).is_err());
    }
}
