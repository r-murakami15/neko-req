//! 接続まわりの設定（M6）: リダイレクトでの認証情報の扱い・手動のプロキシ・CA 証明書・クライアント証明書（相互 TLS）・
//! ボディの上限。公開 API（`send_with_transport` など）だけを使う。

use std::sync::Arc;

use nekoreq_core::{HeaderPair, PreparedBody, PreparedRequest, SendError, SendSettings};
use nekoreq_http::{
    parse_ca_pem, send, send_with_transport, CancellationToken, ClientIdentity, Limits,
    ProxyChoice, Transport,
};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn prepared(method: &str, url: &str, headers: &[(&str, &str)]) -> PreparedRequest {
    PreparedRequest {
        method: method.into(),
        url: url.into(),
        headers: headers
            .iter()
            .map(|(n, v)| HeaderPair {
                name: (*n).into(),
                value: (*v).into(),
            })
            .collect(),
        body: PreparedBody::None,
        secrets: vec![],
    }
}

async fn go(req: &PreparedRequest, t: &Transport) -> Result<nekoreq_http::SentResponse, SendError> {
    send_with_transport(req, &SendSettings::default(), &CancellationToken::new(), t).await
}

fn header_of(req: &wiremock::Request, name: &str) -> Option<String> {
    req.headers
        .get(name)
        .map(|v| v.to_str().unwrap_or("").to_string())
}

// ---- リダイレクトで認証情報を外す ----

#[tokio::test]
async fn cross_origin_redirect_drops_credentials_but_keeps_other_headers() {
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/dest", b.uri())),
        )
        .mount(&a)
        .await;
    Mock::given(method("GET"))
        .and(path("/dest"))
        .respond_with(ResponseTemplate::new(200).set_body_string("arrived"))
        .mount(&b)
        .await;

    let req = prepared(
        "GET",
        &format!("{}/start", a.uri()),
        &[
            ("X-XSRF-TOKEN", "csrf-secret"),
            ("Cookie", "SESSION=sess; XSRF-TOKEN=csrf-secret"),
            ("Authorization", "Bearer tok"),
            ("Proxy-Authorization", "Basic abc"),
            ("X-Custom", "keep-me"),
        ],
    );
    let sent = go(&req, &Transport::default()).await.unwrap();
    assert_eq!(sent.data.status, 200);
    assert_eq!(sent.data.redirect_count, 1);
    assert_eq!(sent.data.final_url, format!("{}/dest", b.uri()));

    // 最初の送り先には全部届いている
    let first = &a.received_requests().await.unwrap()[0];
    assert_eq!(
        header_of(first, "x-xsrf-token").as_deref(),
        Some("csrf-secret")
    );
    assert!(header_of(first, "cookie").is_some());
    // 別の origin には、認証情報は届かない。それ以外のヘッダーは届く
    let second = &b.received_requests().await.unwrap()[0];
    for name in [
        "x-xsrf-token",
        "cookie",
        "authorization",
        "proxy-authorization",
    ] {
        assert_eq!(
            header_of(second, name),
            None,
            "{name} が別の origin に送られた"
        );
    }
    assert_eq!(header_of(second, "x-custom").as_deref(), Some("keep-me"));
}

#[tokio::test]
async fn same_origin_redirect_keeps_credentials() {
    let a = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/old"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/new"))
        .mount(&a)
        .await;
    Mock::given(method("GET"))
        .and(path("/new"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&a)
        .await;
    let req = prepared(
        "GET",
        &format!("{}/old", a.uri()),
        &[("X-XSRF-TOKEN", "csrf"), ("Cookie", "SESSION=s")],
    );
    let sent = go(&req, &Transport::default()).await.unwrap();
    assert_eq!(sent.data.redirect_count, 1);
    let reqs = a.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(header_of(&reqs[1], "x-xsrf-token").as_deref(), Some("csrf"));
    assert_eq!(header_of(&reqs[1], "cookie").as_deref(), Some("SESSION=s"));
}

#[tokio::test]
async fn a_different_port_is_a_different_origin() {
    // 同じホスト（127.0.0.1）でもポートが違えば別の origin
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    assert_ne!(a.address().port(), b.address().port());
    Mock::given(method("POST"))
        .and(path("/go"))
        .respond_with(
            ResponseTemplate::new(307).insert_header("location", format!("{}/in", b.uri())),
        )
        .mount(&a)
        .await;
    Mock::given(method("POST"))
        .and(path("/in"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&b)
        .await;
    let mut req = prepared("POST", &format!("{}/go", a.uri()), &[("X-XSRF-TOKEN", "t")]);
    req.body = PreparedBody::Bytes(b"payload".to_vec());
    let sent = go(&req, &Transport::default()).await.unwrap();
    assert_eq!(sent.data.status, 200);
    let second = &b.received_requests().await.unwrap()[0];
    assert_eq!(header_of(second, "x-xsrf-token"), None);
    // 307 はメソッドもボディもそのまま送り直す
    assert_eq!(second.method.as_str(), "POST");
    assert_eq!(second.body, b"payload");
}

#[tokio::test]
async fn post_becomes_get_on_302_and_303_and_drops_content_headers() {
    for status in [302u16, 303] {
        let a = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/form"))
            .respond_with(ResponseTemplate::new(status).insert_header("location", "/done"))
            .mount(&a)
            .await;
        Mock::given(method("GET"))
            .and(path("/done"))
            .respond_with(ResponseTemplate::new(200).set_body_string("done"))
            .mount(&a)
            .await;
        let mut req = prepared(
            "POST",
            &format!("{}/form", a.uri()),
            &[("Content-Type", "application/json")],
        );
        req.body = PreparedBody::Bytes(b"{}".to_vec());
        let sent = go(&req, &Transport::default()).await.unwrap();
        assert_eq!(sent.data.body.content, "done", "{status}");
        let reqs = a.received_requests().await.unwrap();
        assert_eq!(reqs[1].method.as_str(), "GET");
        assert!(reqs[1].body.is_empty());
        assert_eq!(header_of(&reqs[1], "content-type"), None);
    }
}

#[tokio::test]
async fn too_many_redirects_is_an_error() {
    let a = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
        .mount(&a)
        .await;
    let err = go(
        &prepared("GET", &format!("{}/loop", a.uri()), &[]),
        &Transport::default(),
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("リダイレクト"), "{}", err.message());
    assert!(err.message().contains("10"), "{}", err.message());
}

#[tokio::test]
async fn redirect_without_following_returns_the_3xx_with_credentials_untouched() {
    let a = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/x"))
        .mount(&a)
        .await;
    let settings = SendSettings {
        follow_redirects: false,
        ..SendSettings::default()
    };
    let sent = send_with_transport(
        &prepared("GET", &format!("{}/a", a.uri()), &[]),
        &settings,
        &CancellationToken::new(),
        &Transport::default(),
    )
    .await
    .unwrap();
    assert_eq!(sent.data.status, 302);
    assert_eq!(sent.data.redirect_count, 0);
}

// ---- プロキシ ----

#[tokio::test]
async fn manual_proxy_receives_the_request_in_absolute_form() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("via-proxy"))
        .mount(&proxy)
        .await;
    let t = Transport {
        proxy: ProxyChoice::Manual {
            url: proxy.uri(),
            username: None,
            password: None,
            no_proxy: vec![],
        },
        ..Transport::default()
    };
    // 存在しないホスト。プロキシ経由でなければ名前を引けずに失敗する
    let sent = go(
        &prepared("GET", "http://target.invalid:8123/api/x?q=1", &[]),
        &t,
    )
    .await
    .unwrap();
    assert_eq!(sent.data.body.content, "via-proxy");
    let seen = &proxy.received_requests().await.unwrap()[0];
    assert_eq!(seen.url.path(), "/api/x");
    assert_eq!(seen.url.query(), Some("q=1"));
    assert_eq!(seen.url.host_str(), Some("target.invalid"));
    assert_eq!(seen.url.port(), Some(8123));
}

#[tokio::test]
async fn manual_proxy_sends_basic_credentials_and_honors_no_proxy() {
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
    let t = Transport {
        proxy: ProxyChoice::Manual {
            url: proxy.uri(),
            username: Some("neko".into()),
            password: Some("pw".into()),
            no_proxy: vec!["127.0.0.1".into()],
        },
        ..Transport::default()
    };
    // 認証つき
    let t2 = Transport {
        proxy: ProxyChoice::Manual {
            url: proxy.uri(),
            username: Some("neko".into()),
            password: Some("pw".into()),
            no_proxy: vec![],
        },
        ..Transport::default()
    };
    go(&prepared("GET", "http://target.invalid/p", &[]), &t2)
        .await
        .unwrap();
    let seen = &proxy.received_requests().await.unwrap()[0];
    // neko:pw
    assert_eq!(
        header_of(seen, "proxy-authorization").as_deref(),
        Some("Basic bmVrbzpwdw==")
    );
    // 除外したホストは直接つなぐ
    let sent = go(&prepared("GET", &format!("{}/d", direct.uri()), &[]), &t)
        .await
        .unwrap();
    assert_eq!(sent.data.body.content, "direct");
    assert_eq!(direct.received_requests().await.unwrap().len(), 1);
    assert_eq!(proxy.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn no_proxy_mode_connects_directly_and_a_bad_proxy_url_is_reported() {
    let direct = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&direct)
        .await;
    let sent = go(
        &prepared("GET", &format!("{}/d", direct.uri()), &[]),
        &Transport {
            proxy: ProxyChoice::None,
            ..Transport::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(sent.data.status, 200);

    let err = go(
        &prepared("GET", "http://x.invalid/", &[]),
        &Transport {
            proxy: ProxyChoice::Manual {
                url: "not a url".into(),
                username: None,
                password: None,
                no_proxy: vec![],
            },
            ..Transport::default()
        },
    )
    .await
    .unwrap_err();
    assert!(err.message().contains("プロキシ"), "{}", err.message());
}

// ---- ボディの上限 ----

#[tokio::test]
async fn preview_limit_comes_from_the_transport() {
    let a = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0123456789"))
        .mount(&a)
        .await;
    let t = Transport {
        limits: Limits {
            preview_bytes: 4,
            ..Limits::default()
        },
        ..Transport::default()
    };
    let sent = go(&prepared("GET", &format!("{}/b", a.uri()), &[]), &t)
        .await
        .unwrap();
    assert!(sent.data.body.truncated);
    assert_eq!(sent.data.body.content, "0123");
    assert_eq!(sent.data.body.size, 10);
    assert_eq!(sent.body.len(), 10, "全体は持っている");
}

// ---- TLS: CA 証明書とクライアント証明書 ----

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

/// TLS を話す最小の HTTP/1.1 サーバー。`require_client_cert` なら、`pki` の CA が署名した証明書を求める。
/// 応答のボディは、クライアント証明書があれば `mtls-ok`、なければ `ok`。
async fn tls_server(pki: &Pki, require_client_cert: bool) -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap();
    let builder = if require_client_cert {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(pki.ca_der.clone()).unwrap();
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .unwrap();
        builder.with_client_cert_verifier(verifier)
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
async fn a_custom_ca_makes_a_private_server_verify() {
    let pki = make_pki();
    let port = tls_server(&pki, false).await;
    let url = format!("https://localhost:{port}/");
    let req = prepared("GET", &url, &[]);

    // OS の証明書ストアだけでは通らない
    let err = send(&req, &SendSettings::default(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, SendError::Tls { .. }), "{err:?}");

    // CA（PEM）を足すと、検証したまま通る
    let t = Transport {
        ca_certs: parse_ca_pem(pki.ca_pem.as_bytes()).unwrap(),
        ..Transport::default()
    };
    let sent = go(&req, &t).await.unwrap();
    assert_eq!(sent.data.status, 200);
    assert_eq!(sent.data.body.content, "ok");
    assert!(sent.data.connection.tls);

    // IP アドレスの URL でも（SAN に入れてある）
    let sent = go(
        &prepared("GET", &format!("https://127.0.0.1:{port}/"), &[]),
        &t,
    )
    .await
    .unwrap();
    assert_eq!(sent.data.status, 200);

    // 別の CA を足しても通らない
    let other = make_pki();
    let t_other = Transport {
        ca_certs: parse_ca_pem(other.ca_pem.as_bytes()).unwrap(),
        ..Transport::default()
    };
    assert!(go(&req, &t_other).await.is_err());
}

#[tokio::test]
async fn mutual_tls_with_a_pem_client_certificate() {
    let pki = make_pki();
    let port = tls_server(&pki, true).await;
    let url = format!("https://localhost:{port}/");
    let req = prepared("GET", &url, &[]);
    let ca = parse_ca_pem(pki.ca_pem.as_bytes()).unwrap();

    // クライアント証明書がなければ、ハンドシェイクで切られる
    let none = Transport {
        ca_certs: ca.clone(),
        ..Transport::default()
    };
    assert!(go(&req, &none).await.is_err());

    // 証明書と秘密鍵を同じ PEM に並べる
    let pem = format!("{}{}", pki.client_cert_pem, pki.client_key_pem);
    let identity = ClientIdentity::from_pem("localhost", pem.as_bytes()).unwrap();
    assert!(
        !format!("{identity:?}").contains("PRIVATE"),
        "Debug に鍵を出さない"
    );
    let t = Transport {
        ca_certs: ca.clone(),
        client_identities: vec![identity],
        ..Transport::default()
    };
    let sent = go(&req, &t).await.unwrap();
    assert_eq!(sent.data.body.content, "mtls-ok");

    // ホストのパターンに当てはまらないときは証明書を送らない
    let other_host = ClientIdentity::from_pem("*.internal.example", pem.as_bytes()).unwrap();
    let t = Transport {
        ca_certs: ca.clone(),
        client_identities: vec![other_host],
        ..Transport::default()
    };
    assert!(go(&req, &t).await.is_err());

    // `*` やポートつきのパターン
    let any = ClientIdentity::from_pem(&format!("localhost:{port}"), pem.as_bytes()).unwrap();
    let t = Transport {
        ca_certs: ca,
        client_identities: vec![any],
        ..Transport::default()
    };
    assert_eq!(go(&req, &t).await.unwrap().data.body.content, "mtls-ok");
}

#[tokio::test]
async fn mutual_tls_with_a_pkcs12_client_certificate() {
    use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKey, PrivateKeyChain};

    let pki = make_pki();
    let port = tls_server(&pki, true).await;
    let req = prepared("GET", &format!("https://localhost:{port}/"), &[]);
    let ca = parse_ca_pem(pki.ca_pem.as_bytes()).unwrap();

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
    let p12 = store.writer("p12-password").write().unwrap();

    // パスワードが違えば読めない（メッセージにパスワードは出ない）
    let e = ClientIdentity::from_pkcs12("localhost", &p12, "wrong").unwrap_err();
    assert!(e.contains("PKCS#12"), "{e}");
    assert!(!e.contains("wrong"));

    let identity =
        ClientIdentity::from_file_bytes("localhost", "client.p12", &p12, "p12-password").unwrap();
    let t = Transport {
        ca_certs: ca,
        client_identities: vec![identity],
        ..Transport::default()
    };
    assert_eq!(go(&req, &t).await.unwrap().data.body.content, "mtls-ok");
}

#[test]
fn client_identity_errors_are_in_japanese() {
    let e = ClientIdentity::from_file_bytes("*", "c.pem", b"junk", "").unwrap_err();
    assert!(e.contains("PEM"), "{e}");
    let pki = make_pki();
    // 証明書だけで秘密鍵がない
    let e = ClientIdentity::from_pem("*", pki.client_cert_pem.as_bytes()).unwrap_err();
    assert!(e.contains("秘密鍵"), "{e}");
    let e = ClientIdentity::from_pem("*", pki.client_key_pem.as_bytes()).unwrap_err();
    assert!(e.contains("証明書"), "{e}");
}
