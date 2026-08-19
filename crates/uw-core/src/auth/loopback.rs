//! Loopback redirect receiver (RFC 8252 §7.3).
//!
//! We use a loopback listener rather than a custom URI scheme: custom schemes
//! are flaky across the three desktop platforms, and both Claude and Codex
//! register loopback redirects anyway.
//!
//! Deliberately hand-rolled rather than pulling in a web framework — this
//! serves exactly one request and then dies.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Loopback {
    listener: TcpListener,
    addr: SocketAddr,
    path: String,
}

/// What came back on the redirect, already validated against our `state`.
pub struct Callback {
    pub code: String,
}

impl Loopback {
    /// Bind the redirect receiver. `port` of 0 asks the OS for an ephemeral
    /// port; pass a fixed one where the provider registered a fixed redirect
    /// (Codex requires 1455).
    pub async fn bind(port: u16, path: &str) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| {
                if port == 0 {
                    "failed to bind a loopback port for the OAuth redirect".to_string()
                } else {
                    format!(
                        "failed to bind 127.0.0.1:{port} for the OAuth redirect — \
                         another process (likely a running `codex login`) may hold it"
                    )
                }
            })?;
        let addr = listener.local_addr()?;
        Ok(Loopback {
            listener,
            addr,
            path: path.to_string(),
        })
    }

    /// Built with the `localhost` hostname, not `127.0.0.1`.
    /// Claude Code registers `http://localhost:<port>/callback`, and the
    /// authorize endpoint rejects a redirect that does not match that form.
    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}{}", self.addr.port(), self.path)
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Block until the browser hits our redirect, then return the auth code.
    ///
    /// `expected_state` is compared in full; a mismatch aborts rather than
    /// proceeding, since that is the signature of a cross-site login attempt.
    pub async fn wait(self, expected_state: &str, timeout: Duration) -> Result<Callback> {
        let accept = async {
            loop {
                let (mut sock, _) = self.listener.accept().await?;

                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await?;
                let req = String::from_utf8_lossy(&buf[..n]).to_string();

                let Some(target) = request_target(&req) else {
                    respond(&mut sock, 400, "Malformed request.").await;
                    continue;
                };
                // Browsers ask for /favicon.ico and similar; ignore anything
                // that is not our redirect path.
                if !target.starts_with(&self.path) {
                    respond(&mut sock, 404, "Not found.").await;
                    continue;
                }

                let params = query_params(target);

                if let Some(err) = params.get("error") {
                    let desc = params
                        .get("error_description")
                        .map(String::as_str)
                        .unwrap_or("no description given");
                    respond(&mut sock, 400, &format!("Login failed: {desc}")).await;
                    bail!("provider rejected the authorization: {err} — {desc}");
                }

                match (params.get("code"), params.get("state")) {
                    (Some(code), Some(state)) if state == expected_state => {
                        respond(
                            &mut sock,
                            200,
                            "Signed in. You can close this tab and return to your terminal.",
                        )
                        .await;
                        return Ok(Callback { code: code.clone() });
                    }
                    (Some(_), Some(_)) => {
                        respond(&mut sock, 400, "State mismatch — login rejected.").await;
                        bail!(
                            "OAuth state mismatch: the redirect did not come from the \
                             request we started. Login aborted."
                        );
                    }
                    _ => {
                        respond(&mut sock, 400, "Redirect was missing code or state.").await;
                        bail!("redirect carried neither an error nor a usable code/state pair");
                    }
                }
            }
        };

        tokio::time::timeout(timeout, accept)
            .await
            .map_err(|_| anyhow!("timed out after {}s waiting for the browser redirect", timeout.as_secs()))?
    }
}

fn request_target(req: &str) -> Option<&str> {
    req.lines().next()?.split_whitespace().nth(1)
}

fn query_params(target: &str) -> HashMap<String, String> {
    let Some((_, qs)) = target.split_once('?') else {
        return HashMap::new();
    };
    url::form_urlencoded::parse(qs.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

async fn respond(sock: &mut tokio::net::TcpStream, status: u16, message: &str) {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>usage-watcher</title>\
         <body style=\"font:16px system-ui;display:grid;place-items:center;height:90vh;margin:0\">\
         <p>{message}</p></body>"
    );
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body.as_bytes()).await;
    let _ = sock.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_and_params() {
        let req = "GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let target = request_target(req).unwrap();
        assert_eq!(target, "/callback?code=abc&state=xyz");
        let p = query_params(target);
        assert_eq!(p.get("code").unwrap(), "abc");
        assert_eq!(p.get("state").unwrap(), "xyz");
    }

    #[test]
    fn percent_decodes_params() {
        let p = query_params("/cb?error_description=user%20said%20no");
        assert_eq!(p.get("error_description").unwrap(), "user said no");
    }

    #[tokio::test]
    async fn redirect_uri_uses_localhost_and_the_bound_port() {
        let lb = Loopback::bind(0, "/callback").await.unwrap();
        let uri = lb.redirect_uri();
        // Must be `localhost`, not `127.0.0.1`: Anthropic registers the former
        // and rejects the latter with "Invalid request format".
        assert!(uri.starts_with("http://localhost:"), "got {uri}");
        assert!(uri.ends_with("/callback"));
        // The ephemeral port must be resolved, never left as 0.
        assert!(!uri.contains(":0/"));
        assert_eq!(uri, format!("http://localhost:{}/callback", lb.port()));
    }
}
