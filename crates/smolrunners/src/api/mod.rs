mod http;
mod routes;
mod schema;
mod state;
mod tls;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use http::{
    Request, Response, request as http_request, request_tls as http_request_tls, serve_on as http_serve_on,
};
pub use schema::{API_SCHEMA_VERSION, ApiReply, JsonOutput};
pub use state::{
    CacheRecord, EffectiveLimits, HostRecord, ImageRecord, Installation, Invitation, JobRecord,
    NewExecution, ProfileRecord, State, StateData, is_terminal,
};
pub use tls::{ClientTls, Identity, IdentityKind, Peer, client_config, server_config};
pub use routes::route;

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

pub fn default_state_dir() -> PathBuf {
    std::env::var_os("SMOLRUNNERS_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".work/api-state"))
}

pub fn server_url() -> String {
    std::env::var("SMOLRUNNERS_API_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8787".to_owned())
}

pub fn serve(args: &[String]) -> Result<(), String> {
    let mut listen = "127.0.0.1:8787".to_owned();
    let mut state_dir = default_state_dir();
    let mut tls_ca: Option<PathBuf> = None;
    let mut tls_cert: Option<PathBuf> = None;
    let mut tls_key: Option<PathBuf> = None;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--listen" => listen = value(args, &mut index, "--listen")?,
            "--state-dir" => state_dir = PathBuf::from(value(args, &mut index, "--state-dir")?),
            "--tls-ca" => tls_ca = Some(PathBuf::from(value(args, &mut index, "--tls-ca")?)),
            "--tls-cert" => tls_cert = Some(PathBuf::from(value(args, &mut index, "--tls-cert")?)),
            "--tls-key" => tls_key = Some(PathBuf::from(value(args, &mut index, "--tls-key")?)),
            "--help" | "-h" => {
                eprintln!(
                    "usage: smolrunners serve [--listen HOST:PORT] [--state-dir DIR] [--tls-ca CA --tls-cert CERT --tls-key KEY]"
                );
                return Ok(());
            }
            other => return Err(format!("serve: unknown option {other}")),
        }
        index += 1;
    }

    let tls = match (tls_ca, tls_cert, tls_key) {
        (Some(ca), Some(cert), Some(key)) => {
            Some(server_config(&ca, &cert, &key)?)
        }
        (None, None, None) => None,
        _ => return Err("serve: --tls-ca, --tls-cert and --tls-key must be supplied together".to_owned()),
    };
    let transport = if tls.is_some() { "mtls" } else { "plain-http" };

    let state = Arc::new(Mutex::new(State::open(&state_dir)?));
    eprintln!("smolrunners api listening on {listen} transport={transport}");
    serve_state(&listen, state, tls)
}

pub fn serve_state(
    listen: &str,
    state: Arc<Mutex<State>>,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<(), String> {
    let handler = Arc::new(move |request: &Request| {
        let reply = route(request, &state);
        let status = reply.status;
        let body = serde_json::to_string(&reply).unwrap_or_else(|error| {
            format!("{{\"status\":500,\"data\":{{\"code\":\"encode\",\"message\":\"{error}\"}}}}")
        });
        Response::json(status, body)
    });
    http::serve(listen, handler, tls)
}
