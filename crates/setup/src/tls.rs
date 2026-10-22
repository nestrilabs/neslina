//! The setup UI's certificate: self-signed, made on first boot.
//!
//! Browsers warn about it once, as they do for Unraid or Cockpit. It is kept
//! on the state disk when there is one, so the warning does not come back on
//! every boot; before that it lives in RAM.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use neslina_common::info;
use neslina_common::sys::is_mountpoint;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

fn dir() -> PathBuf {
    if is_mountpoint("/var") { PathBuf::from("/var/neslina/tls") } else { PathBuf::from("/run/neslina/tls") }
}

/// Loads the certificate, making one first if there is none.
pub fn server_config(hostname: &str) -> Result<Arc<rustls::ServerConfig>> {
    let dir = dir();
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    if !cert.exists() || !key.exists() {
        generate(hostname, &dir, &cert, &key)?;
    }
    let certs = CertificateDer::pem_file_iter(&cert)
        .with_context(|| format!("reading {}", cert.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(&key).with_context(|| format!("reading {}", key.display()))?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(Arc::new(config))
}

/// Once the state disk exists, keeps the certificate made in RAM there, so
/// the browser does not warn a second time after the install reboots.
pub fn keep_on_state_disk() {
    let (from, to) = (Path::new("/run/neslina/tls"), Path::new("/var/neslina/tls"));
    if !is_mountpoint("/var") || to.join("cert.pem").exists() || !from.join("cert.pem").exists() {
        return;
    }
    let copied = fs::create_dir_all(to).and_then(|()| {
        for f in ["cert.pem", "key.pem"] {
            fs::copy(from.join(f), to.join(f))?;
        }
        Ok(())
    });
    if let Err(e) = copied {
        neslina_common::warn!("keeping the certificate on the state disk: {e}");
    }
}

fn generate(hostname: &str, dir: &Path, cert: &Path, key: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let names = vec![format!("{hostname}.local"), hostname.to_owned()];
    let made = rcgen::generate_simple_self_signed(names).context("making the certificate")?;
    fs::write(cert, made.cert.pem())?;
    let key_pem = made.signing_key.serialize_pem();
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(key)?;
        f.write_all(key_pem.as_bytes())?;
    }
    info!("made a certificate for {hostname}.local in {}", dir.display());
    Ok(())
}
