//! api.cgi: DSM runs this for requests from the Jenova window. It forwards
//! the request, with the caller's DSM session, to the service on loopback.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const MAX_BODY: usize = 64 * 1024;

pub fn run() {
    let method = std::env::var("REQUEST_METHOD").unwrap_or_else(|_| "GET".into());
    let query = std::env::var("QUERY_STRING").unwrap_or_default();
    let cookie = std::env::var("HTTP_COOKIE").unwrap_or_default();
    let token = std::env::var("HTTP_X_SYNO_TOKEN").unwrap_or_default();
    let remote = std::env::var("REMOTE_ADDR").unwrap_or_default();
    let length: usize = std::env::var("CONTENT_LENGTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
        .min(MAX_BODY);
    let mut body = vec![0u8; length];
    if length > 0 {
        let _ = std::io::stdin().read_exact(&mut body);
    }
    let method = if method == "POST" { "POST" } else { "GET" };

    match forward(method, &query, &cookie, &token, &remote, &body) {
        Ok((status, payload)) => reply(status, &payload),
        Err(_) => reply(
            503,
            br#"{"error":"The Jenova service isn't running. Start it in Package Center."}"#,
        ),
    }
}

fn clean(value: &str) -> String {
    value.chars().filter(|c| *c != '\r' && *c != '\n').collect()
}

fn forward(
    method: &str,
    query: &str,
    cookie: &str,
    token: &str,
    remote: &str,
    body: &[u8],
) -> std::io::Result<(u16, Vec<u8>)> {
    let addr = format!("127.0.0.1:{}", crate::api::port());
    let mut stream = TcpStream::connect_timeout(&addr.parse().map_err(std::io::Error::other)?, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let head = format!(
        "{method} /api?{query} HTTP/1.0\r\nHost: localhost\r\nX-Jenova-Cookie: {}\r\nX-Jenova-Token: {}\r\nX-Jenova-Remote: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        clean(cookie),
        clean(token),
        clean(remote),
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("bad response"))?;
    let head = String::from_utf8_lossy(&response[..split]);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(502);
    Ok((status, response[split + 4..].to_vec()))
}

fn reply(status: u16, body: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = write!(
        out,
        "Status: {status}\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n"
    );
    let _ = out.write_all(body);
    let _ = out.flush();
}
