//! Linux Secret Service storage for the Starlink account session.

use std::io::Write;
use std::process::{Command, Stdio};

use dishy_cloud::SessionStore;

const SERVICE: &str = "com.dishylink.desktop";
const KEY: &str = "starlink-session";

#[derive(Default)]
pub struct SecretToolSessionStore;

impl SessionStore for SecretToolSessionStore {
    fn read_cookie(&self) -> Result<Option<String>, String> {
        let output = Command::new("secret-tool")
            .args(["lookup", "service", SERVICE, "key", KEY])
            .output()
            .map_err(|error| format!("Secret Service unavailable: {error}"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let value = String::from_utf8(output.stdout)
            .map_err(|error| format!("Secret Service returned invalid text: {error}"))?;
        let cookie = value.trim().to_owned();
        Ok((!cookie.is_empty()).then_some(cookie))
    }

    fn write_cookie(&self, cookie: &str) -> Result<(), String> {
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label=Dishylink Starlink account",
                "service",
                SERVICE,
                "key",
                KEY,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Secret Service unavailable: {error}"))?;
        // Always close stdin and reap the helper, even if the Secret Service
        // has already closed its input pipe. Returning from `write_all` early
        // used to drop a live child and hide the more useful exit status.
        let write_result = match child.stdin.take() {
            Some(mut input) => input.write_all(cookie.as_bytes()),
            None => {
                return Err("Secret Service input unavailable".to_owned());
            }
        };
        let status = child
            .wait()
            .map_err(|error| format!("Secret Service wait failed: {error}"))?;
        if let Err(error) = write_result {
            return Err(format!(
                "Secret Service closed the session input before it was stored ({error}; helper exit: {status})"
            ));
        }
        if status.success() {
            Ok(())
        } else {
            Err("Secret Service refused to store the account session".into())
        }
    }

    fn clear_cookie(&self) -> Result<(), String> {
        let status = Command::new("secret-tool")
            .args(["clear", "service", SERVICE, "key", KEY])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("Secret Service unavailable: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            // Clearing an absent secret is already the requested state.
            Ok(())
        }
    }
}
