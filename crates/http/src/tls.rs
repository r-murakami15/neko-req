//! TLS の設定。rustls（ring）を使い、証明書の検証は OS の証明書ストア（rustls-platform-verifier）に任せる。
//! 検証を切ったとき（`verify_certificates = false`）は、どんな証明書でも通す。

use std::sync::{Arc, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

use crate::timing::{Recorder, TlsStartStore};
use crate::transport::ClientIdentity;

fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// OS の証明書ストアで検証するもの。作るのに時間がかかることがあるので使い回す。
fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>, String> {
    static VERIFIER: OnceLock<Result<Arc<dyn ServerCertVerifier>, String>> = OnceLock::new();
    VERIFIER
        .get_or_init(|| {
            rustls_platform_verifier::Verifier::new(provider())
                .map(|v| Arc::new(v) as Arc<dyn ServerCertVerifier>)
                .map_err(|e| format!("OS の証明書ストアを読めません: {e}"))
        })
        .clone()
}

/// reqwest に渡す rustls の設定を作る。TLS の開始時刻を `recorder` に記録する。
///
/// - `extra_roots`: OS の証明書ストアに足して信頼する CA（設定の「CA 証明書」）。検証を切っているときは使わない。
/// - `identity`: クライアント証明書（サーバーが求めたときに送る）。
pub(crate) fn client_config(
    verify_certificates: bool,
    recorder: Arc<Recorder>,
    extra_roots: &[CertificateDer<'static>],
    identity: Option<&ClientIdentity>,
) -> Result<ClientConfig, String> {
    let provider = provider();
    let verifier: Arc<dyn ServerCertVerifier> = if !verify_certificates {
        Arc::new(NoVerifier(provider.clone()))
    } else if extra_roots.is_empty() {
        platform_verifier()?
    } else {
        Arc::new(
            rustls_platform_verifier::Verifier::new_with_extra_roots(
                extra_roots.iter().cloned(),
                provider.clone(),
            )
            .map_err(|e| format!("CA 証明書を使えません: {e}"))?,
        )
    };
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS の設定に失敗しました: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let mut config = match identity {
        Some(id) => builder
            .with_client_auth_cert(id.chain(), id.key())
            .map_err(|e| format!("クライアント証明書を使えません: {e}"))?,
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config.resumption = rustls::client::Resumption::store(Arc::new(TlsStartStore { recorder }));
    Ok(config)
}

/// 証明書を検証しない。署名の検証だけは通常どおり行う（ハンドシェイクを成立させるため）。
#[derive(Debug)]
struct NoVerifier(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
