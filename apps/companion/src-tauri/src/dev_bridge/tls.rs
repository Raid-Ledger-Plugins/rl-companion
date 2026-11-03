//! Per-install self-signed TLS (spec §7): generated with `rcgen`, SAN = the
//! bound IP, kept beside `config.json` so the fingerprint stays stable across
//! restarts (skeleton-key pinning, C5). Regenerated when the bound IP changes.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::{self, ServerConfig};
use tokio_rustls::TlsAcceptor;

/// File (in the app-config dir) holding the cert + key.
pub const TLS_FILE: &str = "dev-bridge-tls.json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsMaterial {
    pub ip: String,
    cert_der: String,
    key_der: String,
}

impl TlsMaterial {
    pub fn generate(ip: Ipv4Addr) -> Result<Self, rcgen::Error> {
        let mut params = rcgen::CertificateParams::new(vec![ip.to_string()])?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "RL Companion dev bridge");
        let key = rcgen::KeyPair::generate()?;
        let cert = params.self_signed(&key)?;
        Ok(Self {
            ip: ip.to_string(),
            cert_der: B64.encode(cert.der()),
            key_der: B64.encode(key.serialize_der()),
        })
    }

    /// The stored material if it was made for `ip`, else a fresh pair (saved).
    pub fn load_or_create(path: &Path, ip: Ipv4Addr) -> Result<Self, String> {
        let stored = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .filter(|m| m.ip == ip.to_string() && m.cert().is_ok());
        if let Some(m) = stored {
            return Ok(m);
        }
        let m = Self::generate(ip).map_err(|e| format!("could not create a certificate: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string(&m).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("could not save the certificate: {e}"))?;
        Ok(m)
    }

    pub fn cert(&self) -> Result<Vec<u8>, base64::DecodeError> {
        B64.decode(&self.cert_der)
    }

    /// sha256 of the certificate DER, lowercase hex (for pinning).
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let der = self.cert().unwrap_or_default();
        Sha256::digest(der)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub fn acceptor(&self) -> Result<TlsAcceptor, String> {
        let cert = self.cert().map_err(|e| e.to_string())?;
        let key = B64.decode(&self.key_der).map_err(|e| e.to_string())?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
            )
            .map_err(|e| e.to_string())?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}
