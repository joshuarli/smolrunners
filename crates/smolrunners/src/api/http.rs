use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConnection, ServerConnection};

use super::tls::{ClientTls, Identity, Peer};

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    pub peer: Peer,
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
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn tls_io_error(error: rustls::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

struct ServerTlsStream {
    stream: TcpStream,
    connection: ServerConnection,
}

impl Read for ServerTlsStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.connection.reader().read(buffer) {
                Ok(decoded) if decoded > 0 => return Ok(decoded),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            let count = self.connection.read_tls(&mut self.stream)?;
            if count == 0 {
                return Ok(0);
            }
            self.connection.process_new_packets().map_err(tls_io_error)?;
            while self.connection.wants_write() {
                self.connection.write_tls(&mut self.stream)?;
            }
        }
    }
}

impl Write for ServerTlsStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.connection.writer().write(buffer)?;
        self.flush()?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        while self.connection.wants_write() {
            self.connection.write_tls(&mut self.stream)?;
        }
        self.stream.flush()
    }
}

struct ClientTlsStream {
    stream: TcpStream,
    connection: ClientConnection,
}

impl ClientTlsStream {
    fn handshake(&mut self) -> io::Result<()> {
        while self.connection.is_handshaking() {
            while self.connection.wants_write() {
                self.connection.write_tls(&mut self.stream)?;
            }
            let count = self.connection.read_tls(&mut self.stream)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "TLS handshake closed before completion",
                ));
            }
            self.connection.process_new_packets().map_err(tls_io_error)?;
        }
        while self.connection.wants_write() {
            self.connection.write_tls(&mut self.stream)?;
        }
        Ok(())
    }
}

impl Read for ClientTlsStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.connection.reader().read(buffer) {
                Ok(decoded) if decoded > 0 => return Ok(decoded),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            let count = self.connection.read_tls(&mut self.stream)?;
            if count == 0 {
                return Ok(0);
            }
            self.connection.process_new_packets().map_err(tls_io_error)?;
            while self.connection.wants_write() {
                self.connection.write_tls(&mut self.stream)?;
            }
        }
    }
}

impl Write for ClientTlsStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.connection.writer().write(buffer)?;
        self.flush()?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        while self.connection.wants_write() {
            self.connection.write_tls(&mut self.stream)?;
        }
        self.stream.flush()
    }
}

enum ServerStream {
    Plain(TcpStream),
    Tls(ServerTlsStream),
}

impl Read for ServerStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            ServerStream::Plain(stream) => stream.read(buffer),
            ServerStream::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for ServerStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            ServerStream::Plain(stream) => stream.write(buffer),
            ServerStream::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            ServerStream::Plain(stream) => stream.flush(),
            ServerStream::Tls(stream) => stream.flush(),
        }
    }
}

impl ServerStream {
    fn peer(&self) -> Peer {
        match self {
            ServerStream::Plain(_) => Peer::Plain,
            ServerStream::Tls(stream) => match stream
                .connection
                .peer_certificates()
                .and_then(|chain| chain.first())
            {
                Some(certificate) => Identity::from_cert(certificate.as_ref())
                    .map(Peer::TlsAuthenticated)
                    .unwrap_or(Peer::TlsUnauthenticated),
                None => Peer::TlsUnauthenticated,
            },
        }
    }

    fn set_timeout(&self, timeout: Duration) -> io::Result<()> {
        let stream = match self {
            ServerStream::Plain(stream) => stream,
            ServerStream::Tls(stream) => &stream.stream,
        };
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))
    }
}

enum ClientStream {
    Plain(TcpStream),
    Tls(ClientTlsStream),
}

impl Read for ClientStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            ClientStream::Plain(stream) => stream.read(buffer),
            ClientStream::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for ClientStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            ClientStream::Plain(stream) => stream.write(buffer),
            ClientStream::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            ClientStream::Plain(stream) => stream.flush(),
            ClientStream::Tls(stream) => stream.flush(),
        }
    }
}

fn read_request<S: Read>(stream: &mut S) -> Result<Request, String> {
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
        peer: Peer::Plain,
    })
}

fn write_response<S: Write>(stream: &mut S, response: &Response) -> Result<(), String> {
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

fn serve_connection(stream: TcpStream, handler: Arc<Handler>, tls: Option<Arc<rustls::ServerConfig>>) {
    let _ = std::thread::spawn(move || {
        let mut stream = match tls {
            Some(config) => {
                let connection = match ServerConnection::new(config) {
                    Ok(connection) => connection,
                    Err(error) => {
                        eprintln!("api: TLS connection: {error}");
                        return;
                    }
                };
                ServerStream::Tls(ServerTlsStream { stream, connection })
            }
            None => ServerStream::Plain(stream),
        };
        if stream.set_timeout(Duration::from_secs(30)).is_err() {
            return;
        }
        match read_request(&mut stream) {
            Ok(mut request) => {
                request.peer = stream.peer();
                let response = handler(&request);
                let _ = write_response(&mut stream, &response);
            }
            Err(error) => eprintln!("api: read request: {error}"),
        }
    });
}

pub fn serve(
    addr: &str,
    handler: Arc<Handler>,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<(), String> {
    let socket = addr
        .to_socket_addrs()
        .map_err(|error| format!("invalid listen address {addr}: {error}"))?
        .next()
        .ok_or_else(|| format!("listen address {addr} resolved to nothing"))?;
    let listener = TcpListener::bind(socket).map_err(|error| format!("bind {addr}: {error}"))?;
    serve_on(listener, handler, tls)
}

pub fn serve_on(
    listener: TcpListener,
    handler: Arc<Handler>,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<(), String> {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => serve_connection(stream, handler.clone(), tls.clone()),
            Err(error) => eprintln!("api: accept: {error}"),
        }
    }
    Ok(())
}

fn parse_url(url: &str) -> Result<(&str, &str, &str), String> {
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(format!("unsupported URL scheme in {url}"));
    };
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    Ok((scheme, authority, path))
}

fn connect(authority: &str, timeout: Duration, tls: Option<&ClientTls>) -> Result<ClientStream, String> {
    let address = authority
        .to_socket_addrs()
        .map_err(|error| format!("resolve {authority}: {error}"))?
        .next()
        .ok_or_else(|| format!("{authority} resolved to nothing"))?;
    let stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("connect {authority}: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("write timeout: {error}"))?;
    match tls {
        None => Ok(ClientStream::Plain(stream)),
        Some(tls) => {
            let name = ServerName::try_from(tls.server_name.as_str())
                .map_err(|error| format!("invalid TLS server name: {error}"))?
                .to_owned();
            let connection = ClientConnection::new(tls.config.clone(), name)
                .map_err(|error| format!("create TLS connection: {error}"))?;
            let mut stream = ClientTlsStream { stream, connection };
            stream
                .handshake()
                .map_err(|error| format!("TLS handshake with {authority}: {error}"))?;
            Ok(ClientStream::Tls(stream))
        }
    }
}

fn exchange(
    stream: &mut ClientStream,
    method: &str,
    path: &str,
    authority: &str,
    payload: &str,
) -> Result<Response, String> {
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

fn request_with(
    method: &str,
    url: &str,
    body: Option<&str>,
    timeout_ms: u64,
    tls: Option<&ClientTls>,
) -> Result<Response, String> {
    let (scheme, authority, path) = parse_url(url)?;
    if (scheme == "https") != tls.is_some() {
        return Err(format!("URL scheme {scheme} does not match the transport for {url}"));
    }
    let timeout = Duration::from_millis(timeout_ms);
    let mut stream = connect(authority, timeout, tls)?;
    exchange(&mut stream, method, path, authority, body.unwrap_or(""))
}

pub fn request(
    method: &str,
    url: &str,
    body: Option<&str>,
    timeout_ms: u64,
) -> Result<Response, String> {
    request_with(method, url, body, timeout_ms, None)
}

pub fn request_tls(
    method: &str,
    url: &str,
    body: Option<&str>,
    timeout_ms: u64,
    tls: &ClientTls,
) -> Result<Response, String> {
    request_with(method, url, body, timeout_ms, Some(tls))
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
