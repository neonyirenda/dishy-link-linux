//! Transport behaviour against a fake grpc-web server.
//!
//! The dish is real hardware with reboot consequences, so every transport rule
//! is proven here first, against bytes we control.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dishy_core::grpc_web::{GrpcCallBytes, GrpcWebClient, GrpcWebError, encode_frame, status};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MESSAGE: u8 = 0x00;
const TRAILERS: u8 = 0x80;

#[derive(Default)]
struct Seen {
    body: Vec<u8>,
    content_type: Option<String>,
    x_grpc_web: Option<String>,
    accept: Option<String>,
    origin: Option<String>,
    referer: Option<String>,
}

struct Capture {
    response: Vec<u8>,
    seen: Arc<Mutex<Option<Seen>>>,
}

impl Respond for Capture {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        *self.seen.lock().expect("seen lock") = Some(Seen {
            body: request.body.clone(),
            content_type: header("content-type"),
            x_grpc_web: header("x-grpc-web"),
            accept: header("accept"),
            origin: header("origin"),
            referer: header("referer"),
        });
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/grpc-web+proto")
            .set_body_bytes(self.response.clone())
    }
}

async fn capture_server(response: Vec<u8>) -> (MockServer, Arc<Mutex<Option<Seen>>>) {
    let seen = Arc::new(Mutex::new(None));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Capture {
            response,
            seen: seen.clone(),
        })
        .mount(&server)
        .await;
    (server, seen)
}

// SPEC-TRANSPORT-001: the request body is exactly one length-delimited frame.
// SPEC-TRANSPORT-002: the call carries grpc-web headers and no Origin/Referer.
#[tokio::test]
async fn call_frames_the_request_and_sends_no_origin_or_referer() {
    let (server, seen) = capture_server(encode_frame(MESSAGE, &[0x09, 0x09])).await;
    let client = GrpcWebClient::new();

    let response = client
        .call(&format!("{}/handle", server.uri()), &[0x01, 0x02, 0x03])
        .await
        .expect("successful call");

    assert_eq!(response.as_ref(), &[0x09, 0x09]);
    let seen = seen
        .lock()
        .expect("seen lock")
        .take()
        .expect("request seen");
    assert_eq!(seen.body, encode_frame(MESSAGE, &[0x01, 0x02, 0x03]));
    assert_eq!(
        seen.content_type.as_deref(),
        Some("application/grpc-web+proto")
    );
    assert_eq!(seen.x_grpc_web.as_deref(), Some("1"));
    assert_eq!(seen.accept.as_deref(), Some("application/grpc-web+proto"));
    assert_eq!(seen.origin, None, "the dish rejects unknown origins");
    assert_eq!(
        seen.referer, None,
        "the dish returns an empty 200 to unknown referers"
    );
}

// SPEC-TRANSPORT-004: a trailers-only refusal is a typed status, not transport.
#[tokio::test]
async fn trailers_frame_status_is_a_typed_error() {
    let mut body = encode_frame(MESSAGE, &[0x01]);
    body.extend(encode_frame(
        TRAILERS,
        b"grpc-status:12\r\ngrpc-message:Unimplemented\r\n",
    ));
    let (server, _) = capture_server(body).await;
    let client = GrpcWebClient::new();

    let error = client
        .call(&format!("{}/handle", server.uri()), &[])
        .await
        .expect_err("status 12");

    assert!(matches!(
        error,
        GrpcWebError::Status {
            status: status::UNIMPLEMENTED,
            ..
        }
    ));
    assert!(error.is_unimplemented());
}

// SPEC-TRANSPORT-004: a 403 with no trailer maps to Unauthenticated.
#[tokio::test]
async fn forbidden_http_maps_to_unauthenticated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let client = GrpcWebClient::new();

    let error = client
        .call(&format!("{}/handle", server.uri()), &[])
        .await
        .expect_err("403");

    assert_eq!(error.grpc_status(), Some(status::UNAUTHENTICATED));
}

// SPEC-TRANSPORT-005: every call reports its wire cost exactly once.
#[tokio::test]
async fn byte_accounting_reports_one_call() {
    let (server, _) = capture_server(encode_frame(MESSAGE, &[0; 10])).await;
    let calls = Arc::new(Mutex::new(Vec::<GrpcCallBytes>::new()));
    let sink = calls.clone();
    let client = GrpcWebClient::new().with_reporter(Arc::new(move |bytes| {
        sink.lock().expect("calls lock").push(bytes);
    }));

    client
        .call(&format!("{}/handle", server.uri()), &[0xFF; 20])
        .await
        .expect("successful call");

    let calls = calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].request_bytes >= 25 + 20,
        "framed request plus headers"
    );
    assert!(
        calls[0].response_bytes >= 15,
        "framed response plus headers"
    );
}

// SPEC-TRANSPORT-006: a per-call deadline wins over a slow or stuck box.
#[tokio::test]
async fn per_call_timeout_is_observed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(300))
                .set_body_bytes(encode_frame(MESSAGE, &[0x01])),
        )
        .mount(&server)
        .await;
    let client = GrpcWebClient::new();

    let error = client
        .call_with_timeout(
            &format!("{}/handle", server.uri()),
            &[],
            Some(Duration::from_millis(25)),
        )
        .await
        .expect_err("deadline");

    assert!(matches!(error, GrpcWebError::Transport(_)));
}
