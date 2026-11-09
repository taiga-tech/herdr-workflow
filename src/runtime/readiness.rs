//! HTTPとTCPのreadiness確認(docs/specs/observability.md)。
//!
//! HTTPは外部crateを使わない最小のHTTP/1.1クライアントで、`http://`だけを扱う。
//! リダイレクトは追わず、応答のstatus行だけを読む(読み取り量を制限する)。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// status行とheaderの読み取り上限。
const MAX_RESPONSE_HEAD: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTarget {
    pub host: String,
    pub port: u16,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    #[error("unsupported URL {0:?}: only http:// is supported")]
    UnsupportedUrl(String),
    #[error("invalid URL {0:?}")]
    InvalidUrl(String),
}

/// `http://host[:port][/path]`を分解する。IPv6は`[::1]`表記を受け付ける。
pub fn parse_http_url(url: &str) -> Result<HttpTarget, ProbeError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| ProbeError::UnsupportedUrl(url.to_string()))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err(ProbeError::InvalidUrl(url.to_string()));
    }
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let end = stripped
            .find(']')
            .ok_or_else(|| ProbeError::InvalidUrl(url.to_string()))?;
        let host = &stripped[..end];
        let port = match &stripped[end + 1..] {
            "" => 80,
            other => other
                .strip_prefix(':')
                .and_then(|port| port.parse().ok())
                .ok_or_else(|| ProbeError::InvalidUrl(url.to_string()))?,
        };
        (host.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (
                host.to_string(),
                port.parse()
                    .map_err(|_| ProbeError::InvalidUrl(url.to_string()))?,
            ),
            None => (authority.to_string(), 80),
        }
    };
    if host.is_empty() || port == 0 {
        return Err(ProbeError::InvalidUrl(url.to_string()));
    }
    let path = path.split('#').next().unwrap_or("/").to_string();
    Ok(HttpTarget { host, port, path })
}

fn addresses(host: &str, port: u16) -> Vec<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .map(Iterator::collect)
        .unwrap_or_default()
}

fn connect(host: &str, port: u16, timeout: Duration) -> Option<TcpStream> {
    addresses(host, port)
        .into_iter()
        .find_map(|addr| TcpStream::connect_timeout(&addr, timeout).ok())
}

/// 指定host・portへ接続できるか。
#[must_use]
pub fn tcp_probe(host: &str, port: u16, timeout: Duration) -> bool {
    connect(host, port, timeout).is_some()
}

/// 期待statusを返したか。接続・読み取りの失敗は`false`。
#[must_use]
pub fn http_probe(target: &HttpTarget, expected_status: u16, timeout: Duration) -> bool {
    http_status(target, timeout) == Some(expected_status)
}

#[must_use]
pub fn http_status(target: &HttpTarget, timeout: Duration) -> Option<u16> {
    let mut stream = connect(&target.host, target.port, timeout)?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let host_header = if target.host.contains(':') {
        format!("[{}]:{}", target.host, target.port)
    } else {
        format!("{}:{}", target.host, target.port)
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: herdr-workflow\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        target.path
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut head = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !head.windows(2).any(|w| w == b"\r\n") && head.len() < MAX_RESPONSE_HEAD {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&buffer[..read]);
    }
    let line_end = head.windows(2).position(|w| w == b"\r\n")?;
    let line = std::str::from_utf8(&head[..line_end]).ok()?;
    let mut parts = line.split(' ');
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// 既定のloopback対象か(起動前のポート使用確認に使う)。
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parses_http_urls() {
        assert_eq!(
            parse_http_url("http://127.0.0.1:3000/health?x=1").unwrap(),
            HttpTarget {
                host: "127.0.0.1".to_string(),
                port: 3000,
                path: "/health?x=1".to_string()
            }
        );
        assert_eq!(parse_http_url("http://localhost").unwrap().port, 80);
        assert_eq!(parse_http_url("http://[::1]:8080/").unwrap().host, "::1");
        assert!(matches!(
            parse_http_url("https://example.com"),
            Err(ProbeError::UnsupportedUrl(_))
        ));
        assert!(parse_http_url("http://:80/").is_err());
        assert!(parse_http_url("http://u@h/").is_err());
    }

    #[test]
    fn tcp_probe_detects_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(tcp_probe("127.0.0.1", port, Duration::from_secs(1)));
        drop(listener);
        // 並行する試験が同じ番号を再利用することがあるため、何度か確かめる。
        assert!((0..20).any(|_| {
            let closed = !tcp_probe("127.0.0.1", port, Duration::from_millis(200));
            if !closed {
                std::thread::sleep(Duration::from_millis(50));
            }
            closed
        }));
    }

    #[test]
    fn http_probe_reads_status_line() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for status in ["200 OK", "503 Service Unavailable"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).unwrap();
                let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let target = parse_http_url(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert!(http_probe(&target, 200, Duration::from_secs(2)));
        assert!(!http_probe(&target, 200, Duration::from_secs(2)));
        server.join().unwrap();
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("example.com"));
    }
}
