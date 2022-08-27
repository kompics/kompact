use std::{sync::Arc, time::Instant, convert::TryInto};

use hex_literal::hex;
use quinn_proto::{
    ServerConfig,
    ClientConfig, Endpoint, Transmit,
};
use rustls::{Certificate, KeyLogFile, PrivateKey};
use lazy_static::lazy_static;
use assert_matches::assert_matches;


pub fn server_config() -> ServerConfig {
    ServerConfig::with_crypto(Arc::new(server_crypto()))
}

pub fn server_crypto() -> rustls::ServerConfig {
    let cert = Certificate(CERTIFICATE.serialize_der().unwrap());
    let key = PrivateKey(CERTIFICATE.serialize_private_key_der());
    server_crypto_with_cert(cert, key)
}

pub fn server_crypto_with_cert(cert: Certificate, key: PrivateKey) -> rustls::ServerConfig {
    server_conf(vec![cert], key).unwrap()
}

pub fn server_conf(
    cert_chain: Vec<rustls::Certificate>,
    key: rustls::PrivateKey,
) -> Result<rustls::ServerConfig, rustls::Error> {
    let mut cfg = rustls::ServerConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;
    cfg.max_early_data_size = u32::MAX;
    Ok(cfg)
}

pub fn client_config() -> ClientConfig {
    ClientConfig::new(Arc::new(client_crypto()))
}

pub fn client_crypto() -> rustls::ClientConfig {
    let cert = rustls::Certificate(CERTIFICATE.serialize_der().unwrap());
    client_crypto_with_certs(vec![cert])
}

pub fn client_crypto_with_certs(certs: Vec<rustls::Certificate>) -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs {
        roots.add(&cert).unwrap();
    }
    let mut config = rustls::ClientConfig::builder()
        .with_safe_default_cipher_suites()
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.enable_early_data = true;
    config.key_log = Arc::new(KeyLogFile::new());
    config
}

lazy_static! {
    static ref CERTIFICATE: rcgen::Certificate =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
}

#[test]
fn version_negotiate_server() {
    let client_addr = "127.0.0.1:7890".parse().expect("Address should work");
    let mut server = Endpoint::new(Default::default(), Some(Arc::new(server_config())));
    let now = Instant::now();
    let event = server.handle(
        now,
        client_addr,
        None,
        None,
        // Long-header packet with reserved version number
        hex!("80 0a1a2a3a 04 00000000 04 00000000 00")[..].into(),
    );
    assert!(event.is_none());

    let io = server.poll_transmit();
    assert!(io.is_some());
    if let Some(Transmit { contents, .. }) = io {
        assert_ne!(contents[0] & 0x80, 0);
        assert_eq!(&contents[1..15], hex!("00000000 04 00000000 04 00000000"));
       assert!(contents[15..].chunks(4).any(|x| {
           DEFAULT_SUPPORTED_VERSIONS.contains(&u32::from_be_bytes(x.try_into().unwrap()))
       }));
    }
    assert_matches!(server.poll_transmit(), None);
    /// The QUIC protocol version implemented.
    pub const DEFAULT_SUPPORTED_VERSIONS: &[u32] = &[
        0x00000001,
        0xff00_001d,
        0xff00_001e,
        0xff00_001f,
        0xff00_0020,
        0xff00_0021,
        0xff00_0022,
    ];
}