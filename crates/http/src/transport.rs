//! 接続まわりの設定（プロキシ・CA 証明書・クライアント証明書・ボディの上限）。
//!
//! `SendSettings`（タイムアウト・リダイレクト・証明書の検証）は画面とやりとりする型で、秘密を持たない。
//! こちらは app が設定（`AppSettings`）とキーチェーンから作って渡す、実行時の値（証明書の中身・パスワード込みの
//! プロキシ）。ファイルやキーチェーンは読まない（読むのは app の仕事）。

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::Limits;

/// プロキシの使い方。
#[derive(Clone, PartialEq, Eq, Default)]
pub enum ProxyChoice {
    /// OS の設定（macOS・Windows のシステム設定）と環境変数（`HTTP_PROXY` など）に従う。
    System,
    /// プロキシを使わない。
    #[default]
    None,
    /// 手で指定する。
    Manual {
        /// `http://host:port`（`https://` も可）。
        url: String,
        /// 認証が要るときのユーザー名とパスワード（URL には入れない）。
        username: Option<String>,
        password: Option<String>,
        /// プロキシを通さないホスト（`localhost`・`.internal.example`・`10.0.0.0/8` など。reqwest の `NO_PROXY` 形式）。
        no_proxy: Vec<String>,
    },
}

// パスワードを持つので、Debug に出さない
impl std::fmt::Debug for ProxyChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyChoice::System => f.write_str("System"),
            ProxyChoice::None => f.write_str("None"),
            ProxyChoice::Manual { url, no_proxy, .. } => f
                .debug_struct("Manual")
                .field("url", url)
                .field("no_proxy", no_proxy)
                .finish_non_exhaustive(),
        }
    }
}

/// クライアント証明書（ホストのパターンつき）。
pub struct ClientIdentity {
    /// 使うホストのパターン（`*.internal.example`・`api.example.com`・`host:8443`）。`*` は何文字でも。
    pub host_pattern: String,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl Clone for ClientIdentity {
    fn clone(&self) -> Self {
        Self {
            host_pattern: self.host_pattern.clone(),
            chain: self.chain.clone(),
            key: self.key.clone_key(),
        }
    }
}

// 秘密鍵を持つので、Debug に出さない
impl std::fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientIdentity")
            .field("host_pattern", &self.host_pattern)
            .field("certificates", &self.chain.len())
            .finish_non_exhaustive()
    }
}

impl ClientIdentity {
    /// 証明書と秘密鍵を同じ PEM ファイルに並べたもの（`BEGIN CERTIFICATE` と `BEGIN PRIVATE KEY` など）から作る。
    pub fn from_pem(host_pattern: &str, pem: &[u8]) -> Result<Self, String> {
        let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("証明書を読めません: {e}"))?;
        if chain.is_empty() {
            return Err("PEM ファイルに証明書（BEGIN CERTIFICATE）がありません".to_string());
        }
        let key = PrivateKeyDer::from_pem_slice(pem).map_err(|e| match e {
            rustls::pki_types::pem::Error::NoItemsFound => {
                "PEM ファイルに秘密鍵（BEGIN PRIVATE KEY など）がありません".to_string()
            }
            other => format!("秘密鍵を読めません: {other}"),
        })?;
        Ok(Self {
            host_pattern: host_pattern.to_string(),
            chain,
            key,
        })
    }

    /// PKCS#12（.p12・.pfx）から作る。`password` は空でもよい。
    pub fn from_pkcs12(host_pattern: &str, der: &[u8], password: &str) -> Result<Self, String> {
        let keystore = p12_keystore::KeyStore::from_pkcs12(
            der,
            password,
            p12_keystore::Pkcs12ImportPolicy::Relaxed,
        )
        .map_err(|e| {
            format!("PKCS#12 ファイルを読めません（パスワードが違うか、壊れています）: {e}")
        })?;
        let (_, chain) = keystore
            .private_key_chain()
            .ok_or_else(|| "PKCS#12 ファイルに秘密鍵つきの証明書がありません".to_string())?;
        let certs: Vec<CertificateDer<'static>> = chain
            .certs()
            .iter()
            .map(|c| CertificateDer::from(c.as_der().to_vec()))
            .collect();
        if certs.is_empty() {
            return Err("PKCS#12 ファイルに証明書がありません".to_string());
        }
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(chain.key().as_der().to_vec()));
        Ok(Self {
            host_pattern: host_pattern.to_string(),
            chain: certs,
            key,
        })
    }

    /// ファイルの拡張子（`.p12`・`.pfx` なら PKCS#12、それ以外は PEM）で読み分ける。
    pub fn from_file_bytes(
        host_pattern: &str,
        file_name: &str,
        bytes: &[u8],
        password: &str,
    ) -> Result<Self, String> {
        let lower = file_name.to_ascii_lowercase();
        if lower.ends_with(".p12") || lower.ends_with(".pfx") {
            Self::from_pkcs12(host_pattern, bytes, password)
        } else {
            Self::from_pem(host_pattern, bytes)
        }
    }

    pub(crate) fn chain(&self) -> Vec<CertificateDer<'static>> {
        self.chain.clone()
    }

    pub(crate) fn key(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }

    /// このホスト（とポート）に使う証明書か。
    pub fn matches(&self, host: &str, port: Option<u16>) -> bool {
        host_matches(&self.host_pattern, host, port)
    }
}

/// PEM（複数の証明書を並べたものも可）から CA 証明書を読む。
pub fn parse_ca_pem(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("証明書を読めません: {e}"))?;
    if certs.is_empty() {
        return Err("PEM ファイルに証明書（BEGIN CERTIFICATE）がありません".to_string());
    }
    Ok(certs)
}

/// ホストのパターン（`*` は何文字でも。大文字小文字は区別しない）に当てはまるか。
/// パターンに `:` があれば `host:port` と比べ、なければホスト名だけと比べる。
pub fn host_matches(pattern: &str, host: &str, port: Option<u16>) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    // IPv6 のリテラル（`::1`）は `:` を含むので、`[..]` で囲んだ場合だけポートつきとみなす
    let with_port =
        pattern.contains(':') && !pattern.starts_with("::") && pattern.matches(':').count() == 1;
    let target = match (with_port, port) {
        (true, Some(p)) => format!("{host}:{p}"),
        (true, None) => return false,
        (false, _) => host,
    };
    glob(&pattern, &target)
}

/// `*` だけを特別扱いする照合。
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// リダイレクトの途中のレスポンスの `Set-Cookie`（`SESSION`・`XSRF-TOKEN`）を、追従先のリクエストに反映する設定
/// （cookieAuth・CSRF の自動化。ログイン後の 302 で新しいセッションが発行されても、追従先には新しい値が付く）。
#[derive(Debug, Clone)]
pub struct RedirectCookies {
    /// 自動化の対象の origin（環境の `baseUrl`）。このレスポンスの origin と同じときだけ反映する
    /// （別のサイトの `Set-Cookie` を反映しない）。
    pub origin: String,
    /// `SESSION` を反映する（SESSION の自動化がオン）。
    pub session: bool,
    /// `XSRF-TOKEN`（と `X-XSRF-TOKEN` ヘッダー）を反映する（CSRF の自動化がオン）。
    pub csrf: bool,
}

/// 送信・取得のときの接続まわりの設定。
#[derive(Debug, Clone, Default)]
pub struct Transport {
    /// リダイレクトの途中の `Set-Cookie` を追従先に反映する設定。`None` なら何もしない。
    pub redirect_cookies: Option<RedirectCookies>,
    pub proxy: ProxyChoice,
    /// OS の証明書ストアに足して信頼する CA 証明書。
    pub ca_certs: Vec<CertificateDer<'static>>,
    /// クライアント証明書（URL のホストが `host_pattern` に当てはまる最初の 1 つを使う）。
    pub client_identities: Vec<ClientIdentity>,
    /// ボディの上限。
    pub limits: Limits,
}

impl Transport {
    /// 送り先に使うクライアント証明書。
    pub(crate) fn identity_for(&self, url: &url::Url) -> Option<&ClientIdentity> {
        let host = url.host_str()?;
        let port = url.port_or_known_default();
        self.client_identities
            .iter()
            .find(|i| i.matches(host, port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_patterns() {
        assert!(host_matches(
            "*.internal.example",
            "api.internal.example",
            Some(443)
        ));
        assert!(host_matches(
            "*.internal.example",
            "A.B.INTERNAL.example",
            None
        ));
        assert!(!host_matches(
            "*.internal.example",
            "internal.example",
            None
        ));
        assert!(!host_matches(
            "*.internal.example",
            "api.internal.example.evil.com",
            None
        ));
        assert!(host_matches(
            "api.example.com",
            "api.example.com",
            Some(8443)
        ));
        assert!(!host_matches("api.example.com", "xapi.example.com", None));
        // ポートつきのパターンはポートも比べる
        assert!(host_matches("localhost:8443", "localhost", Some(8443)));
        assert!(!host_matches("localhost:8443", "localhost", Some(8080)));
        assert!(host_matches("*", "anything", None));
        assert!(host_matches("10.0.*", "10.0.3.4", None));
        assert!(!host_matches("", "x", None));
        // IPv6
        assert!(host_matches("::1", "[::1]", None));
    }

    #[test]
    fn debug_hides_proxy_password() {
        let p = ProxyChoice::Manual {
            url: "http://p:8080".into(),
            username: Some("u".into()),
            password: Some("PW-SECRET".into()),
            no_proxy: vec![],
        };
        assert!(!format!("{p:?}").contains("PW-SECRET"));
    }

    #[test]
    fn bad_pem_is_reported() {
        assert!(parse_ca_pem(b"not a pem").is_err());
        assert!(ClientIdentity::from_pem("*", b"").is_err());
    }
}
