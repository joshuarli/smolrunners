use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use x509_parser::prelude::*;

const HOST_PREFIX: &str = "host:";
const OPERATOR_PREFIX: &str = "operator:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityKind {
    Host,
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub kind: IdentityKind,
    pub cert_fingerprint: String,
}

impl Identity {
    pub fn from_cert(der: &[u8]) -> Result<Self, String> {
        let (_, certificate) =
            X509Certificate::from_der(der).map_err(|error| format!("parse client certificate: {error}"))?;
        let common_name = certificate
            .subject()
            .iter_common_name()
            .next()
            .and_then(|attribute| attribute.as_str().ok())
            .ok_or_else(|| "client certificate has no common name".to_owned())?;
        let (kind, name) = if let Some(name) = common_name.strip_prefix(HOST_PREFIX) {
            (IdentityKind::Host, name)
        } else if let Some(name) = common_name.strip_prefix(OPERATOR_PREFIX) {
            (IdentityKind::Operator, name)
        } else {
            return Err(format!("client certificate common name is not an identity: {common_name}"));
        };
        if name.is_empty() {
            return Err("client certificate identity is empty".to_owned());
        }
        Ok(Self {
            name: name.to_owned(),
            kind,
            cert_fingerprint: fingerprint(der),
        })
    }

    pub fn is_host(&self) -> bool {
        self.kind == IdentityKind::Host
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peer {
    Plain,
    TlsUnauthenticated,
    TlsAuthenticated(Identity),
}

impl Peer {
    pub fn is_authenticated(&self) -> bool {
        matches!(self, Peer::TlsAuthenticated(_))
    }

    pub fn is_tls(&self) -> bool {
        !matches!(self, Peer::Plain)
    }
}

pub struct ClientTls {
    pub config: Arc<rustls::ClientConfig>,
    pub server_name: String,
}

fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn roots(ca_path: &Path) -> Result<rustls::RootCertStore, String> {
    let file = File::open(ca_path).map_err(|error| format!("open TLS CA {}: {error}", ca_path.display()))?;
    let mut store = rustls::RootCertStore::empty();
    for certificate in rustls_pemfile::certs(&mut BufReader::new(file)) {
        let certificate = certificate.map_err(|error| format!("read TLS CA {}: {error}", ca_path.display()))?;
        store
            .add(certificate)
            .map_err(|error| format!("add TLS CA {}: {error}", ca_path.display()))?;
    }
    if store.is_empty() {
        return Err(format!("TLS CA {} has no certificates", ca_path.display()));
    }
    Ok(store)
}

fn certificate_chain(path: &Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let file = File::open(path).map_err(|error| format!("open TLS certificate {}: {error}", path.display()))?;
    let chain: Result<Vec<_>, _> = rustls_pemfile::certs(&mut BufReader::new(file)).collect();
    chain.map_err(|error| format!("read TLS certificate {}: {error}", path.display()))
}

fn private_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let file = File::open(path).map_err(|error| format!("open TLS key {}: {error}", path.display()))?;
    rustls_pemfile::private_key(&mut BufReader::new(file))
        .map_err(|error| format!("read TLS key {}: {error}", path.display()))?
        .ok_or_else(|| format!("TLS key {} has no private key", path.display()))
}

pub fn server_config(
    ca_path: &Path,
    cert_path: &Path,
    key_path: &Path,
) -> Result<Arc<rustls::ServerConfig>, String> {
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots(ca_path)?))
        .allow_unauthenticated()
        .build()
        .map_err(|error| format!("build client certificate verifier: {error}"))?;
    let config = rustls::ServerConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("select TLS protocol versions: {error}"))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificate_chain(cert_path)?, private_key(key_path)?)
        .map_err(|error| format!("configure server certificate: {error}"))?;
    Ok(Arc::new(config))
}

pub fn client_config(
    ca_path: &Path,
    cert_path: &Path,
    key_path: &Path,
    server_name: &str,
) -> Result<ClientTls, String> {
    let config = rustls::ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("select TLS protocol versions: {error}"))?
        .with_root_certificates(roots(ca_path)?)
        .with_client_auth_cert(certificate_chain(cert_path)?, private_key(key_path)?)
        .map_err(|error| format!("configure client certificate: {error}"))?;
    rustls::pki_types::ServerName::try_from(server_name)
        .map_err(|error| format!("invalid TLS server name {server_name}: {error}"))?;
    Ok(ClientTls {
        config: Arc::new(config),
        server_name: server_name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_taken_from_the_certificate_common_name() {
        let key = rcgen::KeyPair::generate().expect("generate key");
        let mut params = rcgen::CertificateParams::new(vec!["host-sr-host-a".to_owned()]).expect("params");
        params.distinguished_name.push(rcgen::DnType::CommonName, "host:sr-host-a");
        let certificate = params.self_signed(&key).expect("sign cert");
        let identity = Identity::from_cert(certificate.der().as_ref()).expect("identity");
        assert_eq!(identity.name, "sr-host-a");
        assert_eq!(identity.kind, IdentityKind::Host);
        assert!(identity.is_host());
        assert_eq!(identity.cert_fingerprint.len(), 64);
    }

    #[test]
    fn an_unlabelled_common_name_is_not_an_identity() {
        let key = rcgen::KeyPair::generate().expect("generate key");
        let params = rcgen::CertificateParams::new(vec!["unknown".to_owned()]).expect("params");
        let certificate = params.self_signed(&key).expect("sign cert");
        assert!(Identity::from_cert(certificate.der().as_ref()).is_err());
    }
}
