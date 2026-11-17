//! M6 の設定（`AppSettings`）を `AppState` 経由で確かめる: 保存と読み込み・検証・パスワードの扱い、
//! そして送信への反映（タイムアウト・リダイレクト・証明書の検証・プロキシ・CA 証明書・クライアント証明書・表示上限）。
//! 併せて、M5 の持ち越し（別 origin へのリダイレクトで `X-XSRF-TOKEN` を外す）も確かめる。
//!
//! 「OS の設定を使う」プロキシは環境変数を読むので、別のファイル（`m6_proxy_system.rs`）で確かめる。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nekoreq_core::{
    AppSettings, Body, ClientCertSetting, EnvVar, ProxyMode, RequestSpec, ResponseLimit, SendError,
    SendOutcome, SendSettings, VarValue,
};
use nekoreq_lib::error::AppError;
use nekoreq_lib::openapi::{ImportRequest, ImportResult, SourceInput};
use nekoreq_lib::send::{RequestFinished, RequestInput};
use nekoreq_lib::state::AppState;
use nekoreq_secrets::{MemorySecretStore, SecretKey, SecretStore};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- 道具 ----

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    state: AppState,
    secrets: Arc<MemorySecretStore>,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let data = root.join("data");
    let secrets = Arc::new(MemorySecretStore::new());
    let state = AppState::open_with_secrets(&data, secrets.clone()).unwrap();
    Fx {
        _dir: dir,
        root,
        data,
        state,
        secrets,
    }
}

fn get(url: &str) -> RequestInput {
    RequestInput {
        project_id: "p".into(),
        environment_id: None,
        request: RequestSpec {
            method: "GET".into(),
            url: url.into(),
            params: vec![],
            body: Body::None,
            auth: None,
        },
        overrides: vec![],
        op_key: None,
    }
}

async fn send(f: &Fx, input: RequestInput, settings: Option<SendSettings>) -> RequestFinished {
    let (tx, rx) = tokio::sync::oneshot::channel();
    f.state
        .request_send(input, settings, move |r| {
            let _ = tx.send(r);
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), rx)
        .await
        .expect("finished が届く")
        .unwrap()
}

fn project(f: &Fx) -> nekoreq_core::Project {
    f.state.project_create("p").unwrap()
}

/// プロジェクトを作った上で送る。
async fn send_to(f: &Fx, url: &str, settings: Option<SendSettings>) -> RequestFinished {
    let p = f
        .state
        .project_list()
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_else(|| project(f));
    let mut i = get(url);
    i.project_id = p.id;
    send(f, i, settings).await
}

fn failure(r: &RequestFinished) -> &SendError {
    match &r.result {
        SendOutcome::Failure { error } => error,
        SendOutcome::Success { response } => panic!("失敗するはず: {}", response.status),
    }
}

fn success(r: &RequestFinished) -> &nekoreq_core::ResponseData {
    match &r.result {
        SendOutcome::Success { response } => response,
        SendOutcome::Failure { error } => panic!("成功するはず: {error:?}"),
    }
}

fn set(f: &Fx, edit: impl FnOnce(&mut AppSettings)) -> AppSettings {
    let mut s = f.state.app_settings_get().unwrap();
    edit(&mut s);
    f.state.app_settings_set(&s).unwrap()
}

fn assert_not_on_disk(dir: &Path, values: &[&str]) {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap() {
                walk(&e.unwrap().path(), out);
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    for file in files {
        let bytes = std::fs::read(&file).unwrap();
        for v in values {
            assert!(
                !bytes.windows(v.len()).any(|w| w == v.as_bytes()),
                "{} に {v} が平文で残っている",
                file.display()
            );
        }
    }
}

// ---- 保存と読み込み・検証 ----

#[test]
fn settings_round_trip_survive_reopen_and_are_validated() {
    let f = fx();
    assert_eq!(f.state.app_settings_get().unwrap(), AppSettings::default());

    let mut want = AppSettings::default();
    want.appearance.theme = nekoreq_core::Theme::Dark;
    want.appearance.editor_font_size = 15;
    want.appearance.restore_tabs = false;
    want.request.timeout_secs = 0;
    want.request.follow_redirects = false;
    want.request.verify_certificates = false;
    want.request.response_limit = ResponseLimit::Unlimited;
    want.network.proxy.mode = ProxyMode::Manual;
    want.network.proxy.url = "http://proxy.internal:8080".into();
    want.network.proxy.username = "neko".into();
    want.network.proxy.no_proxy = vec!["localhost".into(), ".internal.example".into()];
    want.history.limit = 5000;
    want.history.omit_large_bodies = false;
    want.editor_command = Some("code -g {file}:{line}".into());
    let saved = f.state.app_settings_set(&want).unwrap();
    assert_eq!(saved, want);
    assert_eq!(f.state.app_settings_get().unwrap(), want);

    // 開き直しても残る
    drop(f.state);
    let reopened =
        AppState::open_with_secrets(&f.data, Arc::new(MemorySecretStore::new())).unwrap();
    assert_eq!(reopened.app_settings_get().unwrap(), want);

    // 選択肢にない値・読めない値
    let f = fx();
    let bad = |edit: &dyn Fn(&mut AppSettings)| {
        let mut s = AppSettings::default();
        edit(&mut s);
        let e = f.state.app_settings_set(&s).unwrap_err();
        assert!(matches!(e, AppError::Validation(_)), "{e:?}");
        e.to_string()
    };
    assert!(bad(&|s| s.history.limit = 123).contains("履歴"));
    assert!(bad(&|s| s.appearance.editor_font_size = 16).contains("文字サイズ"));
    assert!(bad(&|s| {
        s.network.proxy.mode = ProxyMode::Manual;
        s.network.proxy.url = "http://user:secretpw@proxy:8080".into();
    })
    .contains("ユーザー名"));
    assert!(!bad(&|s| {
        s.network.proxy.mode = ProxyMode::Manual;
        s.network.proxy.url = "http://user:secretpw@proxy:8080".into();
    })
    .contains("secretpw"));
    assert!(
        bad(&|s| s.network.ca_cert_path = Some("/no/such/ca.pem".into())).contains("CA 証明書")
    );
    assert!(bad(&|s| {
        s.network.client_certs = vec![ClientCertSetting {
            host_pattern: "*.x".into(),
            path: "/no/such/client.p12".into(),
            ..ClientCertSetting::default()
        }]
    })
    .contains("見つかりません"));
    // 保存に失敗しても、前の設定は変わらない
    assert_eq!(f.state.app_settings_get().unwrap(), AppSettings::default());
    // PEM でないファイルは CA 証明書にできない
    let junk = f.root.join("junk.pem");
    std::fs::write(&junk, "not a certificate").unwrap();
    assert!(
        bad(&|s| s.network.ca_cert_path = Some(junk.display().to_string())).contains("CA 証明書")
    );
}

#[test]
fn passwords_live_in_the_keychain_only_and_has_password_is_read_only() {
    let f = fx();
    let cert = f.root.join("client.pem");
    std::fs::write(&cert, "dummy").unwrap();
    let s = set(&f, |s| {
        s.network.proxy.mode = ProxyMode::Manual;
        s.network.proxy.url = "http://proxy:8080".into();
        s.network.proxy.username = "neko".into();
        s.network.client_certs = vec![ClientCertSetting {
            host_pattern: "*.internal".into(),
            path: cert.display().to_string(),
            ..ClientCertSetting::default()
        }];
    });
    let cert_id = s.network.client_certs[0].id.clone();
    assert!(!cert_id.is_empty());
    assert!(!s.network.proxy.has_password && !s.network.client_certs[0].has_password);

    let s = f.state.proxy_password_set("PROXY-PW-SECRET-1").unwrap();
    assert!(s.network.proxy.has_password);
    let s = f
        .state
        .client_cert_password_set(&cert_id, "CERT-PW-SECRET-2")
        .unwrap();
    assert!(s.network.client_certs[0].has_password);
    // 設定の保存・取得では、印は画面から書き換えられない（保存済みの値を引き継ぐ）
    let mut want = f.state.app_settings_get().unwrap();
    want.network.proxy.has_password = false;
    want.network.client_certs[0].has_password = false;
    let saved = f.state.app_settings_set(&want).unwrap();
    assert!(saved.network.proxy.has_password && saved.network.client_certs[0].has_password);
    // SQLite にもデバッグ表示にも出ない
    assert_not_on_disk(&f.data, &["PROXY-PW-SECRET-1", "CERT-PW-SECRET-2"]);
    assert!(!format!("{saved:?}").contains("SECRET"));
    assert_eq!(
        f.secrets
            .get(&SecretKey::new("app", "proxy", "password"))
            .unwrap()
            .as_deref(),
        Some("PROXY-PW-SECRET-1")
    );
    // 存在しない証明書
    assert!(matches!(
        f.state
            .client_cert_password_set("no-such", "x")
            .unwrap_err(),
        AppError::NotFound(_)
    ));
    // 空にすると消える
    let s = f.state.proxy_password_set("").unwrap();
    assert!(!s.network.proxy.has_password);
    assert_eq!(
        f.secrets
            .get(&SecretKey::new("app", "proxy", "password"))
            .unwrap(),
        None
    );
    // 設定から証明書を消すと、パスワードもキーチェーンから消える
    set(&f, |s| s.network.client_certs.clear());
    assert_eq!(
        f.secrets
            .get(&SecretKey::new("app", "client-cert", cert_id.as_str()))
            .unwrap(),
        None
    );
}

// ---- 送信への反映: タイムアウト・リダイレクト ----

#[tokio::test]
async fn saved_timeout_and_redirect_settings_apply_when_settings_is_null() {
    let f = fx();
    project(&f);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/old"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/new"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/new"))
        .respond_with(ResponseTemplate::new(200).set_body_string("arrived"))
        .mount(&server)
        .await;

    // 既定（30 秒・追う）
    let r = send_to(&f, &format!("{}/old", server.uri()), None).await;
    assert_eq!(success(&r).body.content, "arrived");

    // 保存した設定が使われる: リダイレクトを追わない
    set(&f, |s| s.request.follow_redirects = false);
    let r = send_to(&f, &format!("{}/old", server.uri()), None).await;
    assert_eq!(success(&r).status, 302);
    // 明示した設定が優先される
    let r = send_to(
        &f,
        &format!("{}/old", server.uri()),
        Some(SendSettings::default()),
    )
    .await;
    assert_eq!(success(&r).body.content, "arrived");

    // タイムアウト（秒）
    set(&f, |s| {
        s.request.follow_redirects = true;
        s.request.timeout_secs = 1;
    });
    let started = std::time::Instant::now();
    let r = send_to(&f, &format!("{}/slow", server.uri()), None).await;
    assert!(
        matches!(failure(&r), SendError::Timeout { .. }),
        "{:?}",
        failure(&r)
    );
    assert!(
        failure(&r).message().contains("1 秒"),
        "{}",
        failure(&r).message()
    );
    assert!(started.elapsed() < Duration::from_millis(2500));
}

#[tokio::test]
async fn unlimited_timeout_waits_for_a_slow_response() {
    let f = fx();
    project(&f);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(1500))
                .set_body_string("late"),
        )
        .mount(&server)
        .await;
    set(&f, |s| s.request.timeout_secs = 0);
    let r = send_to(&f, &server.uri(), None).await;
    assert_eq!(success(&r).body.content, "late");
}

#[tokio::test]
async fn response_display_limit_follows_the_setting() {
    let f = fx();
    project(&f);
    let server = MockServer::start().await;
    let body = vec![b'a'; 11 * 1024 * 1024];
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;
    // 10 MB（既定）: 先頭だけ。全体の大きさは残る
    let r = send_to(&f, &server.uri(), None).await;
    let b = &success(&r).body;
    assert!(b.truncated);
    assert_eq!(b.content.len(), 10 * 1024 * 1024);
    assert_eq!(b.size as usize, 11 * 1024 * 1024);
    // 50 MB
    set(&f, |s| s.request.response_limit = ResponseLimit::Mb50);
    let r = send_to(&f, &server.uri(), None).await;
    assert!(!success(&r).body.truncated);
    assert_eq!(success(&r).body.content.len(), 11 * 1024 * 1024);
    // 無制限
    set(&f, |s| s.request.response_limit = ResponseLimit::Unlimited);
    let r = send_to(&f, &server.uri(), None).await;
    assert!(!success(&r).body.truncated);
}

// ---- プロキシ ----

#[tokio::test]
async fn manual_proxy_from_settings_with_keychain_password_and_exclusions() {
    let f = fx();
    project(&f);
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("via-proxy"))
        .mount(&proxy)
        .await;
    let direct = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("direct"))
        .mount(&direct)
        .await;

    // 使わない
    set(&f, |s| s.network.proxy.mode = ProxyMode::None);
    let r = send_to(&f, "http://target.invalid:8123/x", None).await;
    assert!(
        matches!(
            failure(&r),
            SendError::Connect { .. } | SendError::Other { .. }
        ),
        "{:?}",
        failure(&r)
    );

    // 手動（認証つき）
    set(&f, |s| {
        s.network.proxy.mode = ProxyMode::Manual;
        s.network.proxy.url = proxy.uri();
        s.network.proxy.username = "neko".into();
        s.network.proxy.no_proxy = vec!["127.0.0.1".into()];
    });
    f.state.proxy_password_set("pw").unwrap();
    // 除外したホスト（127.0.0.1）は直接つなぐ
    let r = send_to(&f, &format!("{}/d", direct.uri()), None).await;
    assert_eq!(success(&r).body.content, "direct");
    assert_eq!(proxy.received_requests().await.unwrap().len(), 0);
    // それ以外はプロキシ経由。パスワードはキーチェーンから読んで Proxy-Authorization に
    let r = send_to(&f, "http://target.invalid:8123/api/x?q=1", None).await;
    assert_eq!(success(&r).body.content, "via-proxy");
    let seen = &proxy.received_requests().await.unwrap()[0];
    assert_eq!(seen.url.host_str(), Some("target.invalid"));
    assert_eq!(seen.url.path(), "/api/x");
    assert_eq!(
        seen.headers
            .get("proxy-authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        "Basic bmVrbzpwdw=="
    );
    // 履歴の送った内容には Proxy-Authorization は入らない（アプリが付けたヘッダーではない）
    let page = f
        .state
        .history_list(&f.state.project_list().unwrap()[0].id, &Default::default())
        .unwrap();
    let e = f.state.history_get(&page.items[0].id).unwrap();
    assert!(!format!("{e:?}").contains("bmVrbzpwdw"));
}

#[tokio::test]
async fn the_yaml_url_is_fetched_through_the_configured_proxy() {
    let f = fx();
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "openapi: 3.0.3\ninfo: {title: T, version: '1'}\npaths:\n  /a:\n    get:\n      operationId: getA\n      responses: {'200': {description: ok}}\n",
        ))
        .mount(&proxy)
        .await;
    set(&f, |s| {
        s.network.proxy.mode = ProxyMode::Manual;
        s.network.proxy.url = proxy.uri();
    });
    let r = f
        .state
        .openapi_import(ImportRequest {
            source: SourceInput::Url {
                url: "http://spec.invalid:8123/openapi.yaml".into(),
                token: None,
                allowed_hosts: vec![],
            },
            project_id: None,
            project_name: None,
            selected: None,
            options: Default::default(),
        })
        .await
        .unwrap();
    assert!(matches!(r, ImportResult::Imported { .. }), "{r:?}");
    let seen = &proxy.received_requests().await.unwrap()[0];
    assert_eq!(seen.url.host_str(), Some("spec.invalid"));
    // プロキシを使わないなら、名前を引けずに取り込めない
    set(&f, |s| s.network.proxy.mode = ProxyMode::None);
    let r = f
        .state
        .openapi_preview(
            SourceInput::Url {
                url: "http://spec.invalid:8123/openapi.yaml".into(),
                token: None,
                allowed_hosts: vec![],
            },
            None,
        )
        .await
        .unwrap();
    assert!(
        !matches!(r, nekoreq_lib::openapi::PreviewResult::Ready { .. }),
        "{r:?}"
    );
}

// ---- TLS: 検証・CA 証明書・クライアント証明書 ----

struct Pki {
    ca_der: CertificateDer<'static>,
    ca_pem: String,
    server_cert: CertificateDer<'static>,
    server_key: Vec<u8>,
    client_cert: CertificateDer<'static>,
    client_cert_pem: String,
    client_key_der: Vec<u8>,
    client_key_pem: String,
}

fn make_pki() -> Pki {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "NekoReq Test CA");
    let ca = CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let server_key = KeyPair::generate().unwrap();
    let mut sp =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    sp.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_cert = sp.signed_by(&server_key, &ca).unwrap();
    let client_key = KeyPair::generate().unwrap();
    let mut cp = CertificateParams::new(Vec::<String>::new()).unwrap();
    cp.distinguished_name
        .push(DnType::CommonName, "neko client");
    cp.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_cert = cp.signed_by(&client_key, &ca).unwrap();
    Pki {
        ca_der: ca.der().clone(),
        ca_pem: ca.pem(),
        server_cert: server_cert.der().clone(),
        server_key: server_key.serialize_der(),
        client_cert: client_cert.der().clone(),
        client_cert_pem: client_cert.pem(),
        client_key_der: client_key.serialize_der(),
        client_key_pem: client_key.serialize_pem(),
    }
}

async fn tls_server(pki: &Pki, require_client_cert: bool) -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap();
    let builder = if require_client_cert {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(pki.ca_der.clone()).unwrap();
        builder.with_client_cert_verifier(
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .unwrap(),
        )
    } else {
        builder.with_no_client_auth()
    };
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pki.server_key.clone()));
    let mut config = builder
        .with_single_cert(vec![pki.server_cert.clone(), pki.ca_der.clone()], key)
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let has_client = tls.get_ref().1.peer_certificates().is_some();
                let mut buf = vec![0u8; 4096];
                let mut seen = Vec::new();
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let body = if has_client { "mtls-ok" } else { "ok" };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    port
}

#[tokio::test]
async fn certificate_verification_and_ca_certificate_settings() {
    let f = fx();
    project(&f);
    let pki = make_pki();
    let port = tls_server(&pki, false).await;
    let url = format!("https://localhost:{port}/");

    // 検証する（既定）: 自分で作った CA の証明書は通らない
    let r = send_to(&f, &url, None).await;
    assert!(
        matches!(failure(&r), SendError::Tls { .. }),
        "{:?}",
        failure(&r)
    );
    // 検証を切る
    set(&f, |s| s.request.verify_certificates = false);
    let r = send_to(&f, &url, None).await;
    assert_eq!(success(&r).body.content, "ok");
    // 検証を戻して、CA 証明書（PEM）を設定する
    let ca_path = f.root.join("ca.pem");
    std::fs::write(&ca_path, &pki.ca_pem).unwrap();
    set(&f, |s| {
        s.request.verify_certificates = true;
        s.network.ca_cert_path = Some(ca_path.display().to_string());
    });
    let r = send_to(&f, &url, None).await;
    assert_eq!(success(&r).body.content, "ok");
    assert!(success(&r).connection.tls);
    // 設定を消すと、また通らない
    set(&f, |s| s.network.ca_cert_path = None);
    assert!(matches!(
        failure(&send_to(&f, &url, None).await),
        SendError::Tls { .. }
    ));
    // 設定したあとにファイルが消えたら、送信の失敗として理由が出る
    set(&f, |s| {
        s.network.ca_cert_path = Some(ca_path.display().to_string())
    });
    std::fs::remove_file(&ca_path).unwrap();
    let r = send_to(&f, &url, None).await;
    assert!(
        failure(&r).message().contains("CA 証明書"),
        "{}",
        failure(&r).message()
    );
}

#[tokio::test]
async fn client_certificates_by_host_pattern_with_pem_and_p12_files() {
    use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKey, PrivateKeyChain};

    let f = fx();
    project(&f);
    let pki = make_pki();
    let port = tls_server(&pki, true).await;
    let url = format!("https://localhost:{port}/");
    let ca_path = f.root.join("ca.pem");
    std::fs::write(&ca_path, &pki.ca_pem).unwrap();
    set(&f, |s| {
        s.network.ca_cert_path = Some(ca_path.display().to_string())
    });

    // クライアント証明書がなければ、サーバーに断られる
    assert!(matches!(
        failure(&send_to(&f, &url, None).await),
        SendError::Connect { .. } | SendError::Tls { .. } | SendError::Other { .. }
    ));

    // PEM（証明書と秘密鍵を同じファイルに）
    let pem_path = f.root.join("client.pem");
    std::fs::write(
        &pem_path,
        format!("{}{}", pki.client_cert_pem, pki.client_key_pem),
    )
    .unwrap();
    let s = set(&f, |s| {
        s.network.client_certs = vec![ClientCertSetting {
            host_pattern: "localhost".into(),
            path: pem_path.display().to_string(),
            ..ClientCertSetting::default()
        }]
    });
    let r = send_to(&f, &url, None).await;
    assert_eq!(success(&r).body.content, "mtls-ok");

    // ホストのパターンに当てはまらない（`*.internal.example`）なら送らない
    let cert_id = s.network.client_certs[0].id.clone();
    set(&f, |s| {
        s.network.client_certs[0].host_pattern = "*.internal.example".into()
    });
    assert!(failure_or_not_mtls(&send_to(&f, &url, None).await));
    set(&f, |s| s.network.client_certs[0].host_pattern = "*".into());
    assert_eq!(
        success(&send_to(&f, &url, None).await).body.content,
        "mtls-ok"
    );

    // .p12（パスワードはキーチェーン）
    let chain = PrivateKeyChain::new(
        b"id".to_vec(),
        PrivateKey::from_der(&pki.client_key_der).unwrap(),
        vec![
            Certificate::from_der(&pki.client_cert).unwrap(),
            Certificate::from_der(&pki.ca_der).unwrap(),
        ],
    );
    let mut store = KeyStore::new();
    store.add_entry("client", KeyStoreEntry::PrivateKeyChain(chain));
    let p12_path = f.root.join("client.p12");
    std::fs::write(&p12_path, store.writer("P12-PW-SECRET-3").write().unwrap()).unwrap();
    set(&f, |s| {
        s.network.client_certs = vec![ClientCertSetting {
            id: cert_id.clone(),
            host_pattern: "localhost".into(),
            path: p12_path.display().to_string(),
            ..ClientCertSetting::default()
        }]
    });
    // パスワードがまだなければ読めない（メッセージに証明書の名前が出る）
    let r = send_to(&f, &url, None).await;
    assert!(
        failure(&r).message().contains("クライアント証明書"),
        "{}",
        failure(&r).message()
    );
    f.state
        .client_cert_password_set(&cert_id, "wrong-pw")
        .unwrap();
    assert!(!failure(&send_to(&f, &url, None).await)
        .message()
        .contains("wrong-pw"));
    f.state
        .client_cert_password_set(&cert_id, "P12-PW-SECRET-3")
        .unwrap();
    assert_eq!(
        success(&send_to(&f, &url, None).await).body.content,
        "mtls-ok"
    );
    assert_not_on_disk(&f.data, &["P12-PW-SECRET-3", "wrong-pw"]);
}

/// 失敗した、またはクライアント証明書を送らなかった（`ok`）。
fn failure_or_not_mtls(r: &RequestFinished) -> bool {
    match &r.result {
        SendOutcome::Failure { .. } => true,
        SendOutcome::Success { response } => response.body.content != "mtls-ok",
    }
}

// ---- M5 の持ち越し: 別 origin へのリダイレクトで X-XSRF-TOKEN を外す ----

#[tokio::test]
async fn xsrf_token_and_cookies_do_not_follow_a_redirect_to_another_origin() {
    let f = fx();
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(307).insert_header("location", format!("{}/dest", b.uri())),
        )
        .mount(&a)
        .await;
    Mock::given(method("POST"))
        .and(path("/same"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/dest"))
        .mount(&a)
        .await;
    for s in [&a, &b] {
        Mock::given(method("POST"))
            .and(path("/dest"))
            .respond_with(ResponseTemplate::new(200))
            .mount(s)
            .await;
    }
    let p = project(&f);
    let env = f.state.env_create(&p.id, "dev").unwrap();
    let mut e = env.clone();
    e.vars = vec![
        EnvVar {
            name: "baseUrl".into(),
            value: VarValue::Plain(a.uri()),
            description: String::new(),
            enabled: true,
        },
        EnvVar {
            name: "xsrfToken".into(),
            value: VarValue::Secret,
            description: String::new(),
            enabled: true,
        },
        EnvVar {
            name: "session".into(),
            value: VarValue::Secret,
            description: String::new(),
            enabled: true,
        },
    ];
    let env = f.state.env_save(&e).unwrap();
    f.state
        .secret_set(&p.id, &env.id, "xsrfToken", "XSRF-TOKEN-VALUE-77")
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "session", "SESSION-VALUE-88")
        .unwrap();

    let post = |url: &str| {
        let mut i = get(url);
        i.project_id = p.id.clone();
        i.request.method = "POST".into();
        i
    };
    // 別の origin へ: 認証情報は付かない
    let r = send(&f, post("{{baseUrl}}/start"), None).await;
    assert_eq!(success(&r).redirect_count, 1);
    let first = &a.received_requests().await.unwrap()[0];
    assert_eq!(
        first.headers.get("x-xsrf-token").unwrap().to_str().unwrap(),
        "XSRF-TOKEN-VALUE-77",
        "最初の送り先には付く"
    );
    let at_b = &b.received_requests().await.unwrap()[0];
    for name in ["x-xsrf-token", "cookie", "authorization"] {
        assert!(
            at_b.headers.get(name).is_none(),
            "{name} が別の origin に届いた"
        );
    }
    // 同じ origin のリダイレクトでは付いたまま
    let r = send(&f, post("{{baseUrl}}/same"), None).await;
    assert_eq!(success(&r).redirect_count, 1);
    let seen = a.received_requests().await.unwrap();
    let dest = seen.iter().find(|r| r.url.path() == "/dest").unwrap();
    assert_eq!(
        dest.headers.get("x-xsrf-token").unwrap().to_str().unwrap(),
        "XSRF-TOKEN-VALUE-77"
    );
    assert!(dest
        .headers
        .get("cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("SESSION=SESSION-VALUE-88"));
}
