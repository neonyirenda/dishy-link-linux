//! Reading HTTP reply bodies with a size cap (spec 0193).
//!
//! The dish and router answer over plain HTTP on the LAN, so whatever answers
//! for their addresses decides how much the app buffers. Every reply the app
//! reads, LAN, cloud, or public, goes through [`read_capped`].

use thiserror::Error;

/// SPEC-BODY-002: eight times the largest real reply (the ~2 MB CelesTrak
/// catalogue).
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum BodyError {
    #[error("reply body is larger than the {limit}-byte cap")]
    TooLarge { limit: usize },
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

/// The whole body, or [`BodyError::TooLarge`] once it passes `limit`.
pub async fn read_capped(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, BodyError> {
    let declared = response.content_length().unwrap_or(0);
    if declared > limit as u64 {
        return Err(BodyError::TooLarge { limit });
    }
    let mut body = Vec::with_capacity(declared as usize);
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Err(BodyError::TooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve one raw HTTP reply and return its URL.
    fn serve(reply: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            // The client may hang up early on an over-cap reply.
            let _ = stream.write_all(&reply);
        });
        format!("http://{address}/")
    }

    async fn fetch(url: &str, limit: usize) -> Result<Vec<u8>, BodyError> {
        let response = reqwest::get(url).await.unwrap();
        read_capped(response, limit).await
    }

    // SPEC-BODY-001
    #[tokio::test]
    async fn a_body_under_the_cap_is_read_whole() {
        let url = serve(
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
        );
        assert_eq!(fetch(&url, 16).await.unwrap(), b"hello");
    }

    // SPEC-BODY-001
    #[tokio::test]
    async fn a_declared_length_over_the_cap_is_refused() {
        let mut reply =
            b"HTTP/1.1 200 OK\r\ncontent-length: 64\r\nconnection: close\r\n\r\n".to_vec();
        reply.extend_from_slice(&[b'x'; 64]);
        let url = serve(reply);
        assert!(matches!(
            fetch(&url, 16).await,
            Err(BodyError::TooLarge { limit: 16 })
        ));
    }

    // SPEC-BODY-001: no Content-Length, so only counting can catch it.
    #[tokio::test]
    async fn a_chunked_body_over_the_cap_is_refused() {
        let mut reply =
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n".to_vec();
        for _ in 0..8 {
            reply.extend_from_slice(b"8\r\nxxxxxxxx\r\n");
        }
        reply.extend_from_slice(b"0\r\n\r\n");
        let url = serve(reply);
        assert!(matches!(
            fetch(&url, 16).await,
            Err(BodyError::TooLarge { limit: 16 })
        ));
    }
}
