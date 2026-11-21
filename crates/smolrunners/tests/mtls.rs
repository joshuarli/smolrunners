use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use smolrunners::api::{
    Peer, Request, Response, client_config, http_request_tls, http_serve_on, server_config,
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "smolrunners-mtls-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, contents: &str) -> PathBuf {
    fs::write(path, contents).expect("write cert material");
    path.to_owned()
}

struct Authority {
    cert: PathBuf,
    issuer: rcgen::CertifiedIssuer<'static, rcgen::KeyPair>,
}

fn authority(dir: &Path, name: &str) -> Authority {
    let key = rcgen::KeyPair::generate().expect("generate CA key");
    let key_pem = key.serialize_pem();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("CA params");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, name);
    let issuer = rcgen::CertifiedIssuer::self_signed(params, key).expect("sign CA cert");
    let cert = write(&dir.join(format!("{name}.crt")), &issuer.pem());
    let _key_path = write(&dir.join(format!("{name}.key")), &key_pem);
    Authority { cert, issuer }
}

fn leaf(
    dir: &Path,
    ca: &Authority,
    name: &str,
    common_name: &str,
    san: &str,
    usage: rcgen::ExtendedKeyUsagePurpose,
) -> (PathBuf, PathBuf) {
    let key = rcgen::KeyPair::generate().expect("generate leaf key");
    let mut params = rcgen::CertificateParams::new(vec![san.to_owned()]).expect("leaf params");
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params.extended_key_usages = vec![usage];
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    let certificate = params
        .signed_by(&key, &ca.issuer)
        .expect("sign leaf cert");
    let cert_path = write(&dir.join(format!("{name}.crt")), &certificate.pem());
    let key_path = write(&dir.join(format!("{name}.key")), &key.serialize_pem());
    (cert_path, key_path)
}

#[test]
fn tls_verifies_the_client_certificate_and_derives_identity_from_it() {
    let dir = TestDir::new();
    let ca = authority(dir.path(), "ca");
    let (server_cert, server_key) = leaf(
        dir.path(),
        &ca,
        "server",
        "server",
        "localhost",
        rcgen::ExtendedKeyUsagePurpose::ServerAuth,
    );
    let (client_cert, client_key) = leaf(
        dir.path(),
        &ca,
        "host-a",
        "host:sr-host-a",
        "host-sr-host-a",
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    );
    let rogue_ca = authority(dir.path(), "rogue-ca");
    let (rogue_cert, rogue_key) = leaf(
        dir.path(),
        &rogue_ca,
        "rogue-host",
        "host:sr-host-a",
        "host-sr-host-a",
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    );

    let server_config = server_config(&ca.cert, &server_cert, &server_key).expect("server config");
    let handler = Arc::new(|request: &Request| match &request.peer {
        Peer::TlsAuthenticated(identity) => Response::json(
            200,
            json!({
                "identity": identity.name,
                "kind": format!("{:?}", identity.kind),
                "fingerprint": identity.cert_fingerprint,
            })
            .to_string(),
        ),
        Peer::TlsUnauthenticated => Response::json(200, json!({ "identity": null }).to_string()),
        Peer::Plain => Response::json(200, json!({ "identity": "plain" }).to_string()),
    });

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    std::thread::spawn(move || {
        let _ = http_serve_on(listener, handler, Some(server_config));
    });
    let url = format!("https://127.0.0.1:{port}/api/v1/status");

    let trusted = client_config(&ca.cert, &client_cert, &client_key, "localhost").expect("client config");
    let response = http_request_tls("GET", &url, None, 3000, &trusted).expect("trusted TLS request");
    assert_eq!(response.status, 200);
    let body: serde_json::Value = serde_json::from_str(&response.body).expect("decode response");
    assert_eq!(body["identity"], "sr-host-a");
    assert_eq!(body["kind"], "Host");
    assert_eq!(body["fingerprint"].as_str().map(str::len), Some(64));

    let untrusted = client_config(&ca.cert, &rogue_cert, &rogue_key, "localhost").expect("rogue client config");
    let result = http_request_tls("GET", &url, None, 3000, &untrusted);
    assert!(result.is_err(), "a client certificate outside the CA must fail: {result:?}");
}
