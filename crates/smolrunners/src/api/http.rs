use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

pub type Handler = dyn Fn(&Request) -> Response + Send + Sync + 'static;

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let count = stream
            .read(&mut chunk)
            .map_err(|error| format!("read request: {error}"))?;
        if count == 0 {
            return Err("client closed before headers".to_owned());
        }
        buffer.extend_from_slice(&chunk[..count]);
        if buffer.len() > 1024 * 1024 {
            return Err("request headers too large".to_owned());
        }
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();

    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length > 16 * 1024 * 1024 {
        return Err("request body too large".to_owned());
    }

    while buffer.len() < header_end + content_length {
        let count = stream
            .read(&mut chunk)
            .map_err(|error| format!("read body: {error}"))?;
        if count == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..count]);
    }

    let body = if buffer.len() > header_end {
        buffer[header_end..header_end + content_length.min(buffer.len() - header_end)].to_vec()
    } else {
        Vec::new()
    };

    Ok(Request {
        method,
        path,
        body,
    })
}

fn write_response(stream: &mut TcpStream, response: &Response) -> Result<(), String> {
    let body = response.body.as_bytes();
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        response.status,
        status_text(response.status),
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| format!("write response: {error}"))
}

pub fn serve(addr: &str, handler: Arc<Handler>) -> Result<(), String> {
    let socket = addr
        .to_socket_addrs()
        .map_err(|error| format!("invalid listen address {addr}: {error}"))?
        .next()
        .ok_or_else(|| format!("listen address {addr} resolved to nothing"))?;
    let listener = TcpListener::bind(socket).map_err(|error| format!("bind {addr}: {error}"))?;
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let handler = handler.clone();
                let _ = std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                    if let Ok(request) = read_request(&mut stream) {
                        let response = handler(&request);
                        let _ = write_response(&mut stream, &response);
                    }
                });
            }
            Err(error) => eprintln!("api: accept: {error}"),
        }
    }
    Ok(())
}

pub fn request(
    method: &str,
    url: &str,
    body: Option<&str>,
    timeout_ms: u64,
) -> Result<Response, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported URL scheme in {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let address = authority
        .to_socket_addrs()
        .map_err(|error| format!("resolve {authority}: {error}"))?
        .next()
        .ok_or_else(|| format!("{authority} resolved to nothing"))?;
    let timeout = Duration::from_millis(timeout_ms);
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("connect {authority}: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("write timeout: {error}"))?;

    let payload = body.unwrap_or("");
    let request_head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        method, path, authority, payload.len()
    );
    stream
        .write_all(request_head.as_bytes())
        .and_then(|_| stream.write_all(payload.as_bytes()))
        .map_err(|error| format!("send request: {error}"))?;

    let mut raw = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let count = stream
            .read(&mut chunk)
            .map_err(|error| format!("read response: {error}"))?;
        if count == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..count]);
    }
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response: no header terminator".to_owned())?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "malformed HTTP status line".to_owned())?;
    let body = String::from_utf8_lossy(&raw[split + 4..]).into_owned();
    Ok(Response { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_parse_response_reads_status_and_body() {
        let raw = b"HTTP/1.1 201 Created\r\nContent-Length: 7\r\n\r\n{\"ok\":1}";
        let response = parse_response(raw).expect("parse response");
        assert_eq!(response.status, 201);
        assert_eq!(response.body, "{\"ok\":1}");
    }
}
