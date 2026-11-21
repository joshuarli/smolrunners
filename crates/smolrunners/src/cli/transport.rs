use crate::api::{ApiReply, http_request, server_url};

pub trait Transport {
    fn send(&self, method: &str, path: &str, body: Option<&str>) -> Result<ApiReply, String>;
}

pub struct HttpTransport {
    base: String,
    timeout_ms: u64,
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self {
            base: server_url(),
            timeout_ms: 5000,
        }
    }
}

impl HttpTransport {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            timeout_ms: 5000,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

impl Transport for HttpTransport {
    fn send(&self, method: &str, path: &str, body: Option<&str>) -> Result<ApiReply, String> {
        let url = format!("{}{}", self.base.trim_end_matches('/'), path);
        let response = http_request(method, &url, body, self.timeout_ms)?;
        if response.body.trim().is_empty() {
            return Err(format!("empty response from {url}"));
        }
        serde_json::from_str::<ApiReply>(&response.body)
            .map_err(|error| format!("decode response from {url}: {error}"))
    }
}
