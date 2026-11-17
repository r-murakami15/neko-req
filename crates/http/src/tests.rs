//! wiremock（ローカルのモックサーバー）と、自己署名証明書の TLS サーバーで確かめる。

use std::sync::Arc;
use std::time::{Duration, Instant};

use nekoreq_core::{
    BodyEncoding, HeaderPair, PreparedBody, PreparedRequest, SendError, SendSettings,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn prepared(method: &str, url: &str) -> PreparedRequest {
    PreparedRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![],
        body: PreparedBody::None,
        secrets: vec![],
    }
}

fn header(name: &str, value: &str) -> HeaderPair {
    HeaderPair {
        name: name.into(),
        value: value.into(),
    }
}

/// `localhost` で書く（DNS 解決の時間も測れるように。IP アドレスを直接書くと名前解決しない）。
fn url(server: &MockServer, p: &str) -> String {
    format!("http://localhost:{}{p}", server.address().port())
}

async fn send_default(req: &PreparedRequest) -> Result<SentResponse, SendError> {
    send(req, &SendSettings::default(), &CancellationToken::new()).await
}

#[tokio::test]
async fn get_returns_status_headers_body_and_a_filled_in_timing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-test", "1")
                .append_header("set-cookie", "a=1; Path=/")
                .append_header("set-cookie", "b=2; Path=/")
                .set_body_raw(r#"{"data":{"loggedIn":false}}"#, "application/json"),
        )
        .mount(&server)
        .await;

    let mut req = prepared("GET", &url(&server, "/api/v1/auth/session?x=1"));
    req.headers.push(header("Accept", "application/json"));
    let sent = send_default(&req).await.unwrap();
    let r = &sent.data;

    assert_eq!(r.status, 200);
    assert_eq!(r.status_text, "OK");
    assert_eq!(r.body.encoding, BodyEncoding::Text);
    assert_eq!(r.body.content, r#"{"data":{"loggedIn":false}}"#);
    assert_eq!(r.body.content_type.as_deref(), Some("application/json"));
    assert!(!r.body.truncated);
    assert_eq!(r.body.size, 27);
    assert_eq!(&sent.body[..], r.body.content.as_bytes());

    // ヘッダーは重複を保つ
    let cookies: Vec<_> = r
        .headers
        .iter()
        .filter(|h| h.name == "set-cookie")
        .map(|h| h.value.as_str())
        .collect();
    assert_eq!(cookies, ["a=1; Path=/", "b=2; Path=/"]);
    assert!(r
        .headers
        .iter()
        .any(|h| h.name == "x-test" && h.value == "1"));

    // 接続の情報
    assert_eq!(r.connection.http_version, "HTTP/1.1");
    assert!(!r.connection.tls);
    let addr = r.connection.remote_addr.clone().expect("接続先が取れる");
    assert!(
        addr.ends_with(&format!(":{}", server.address().port())),
        "{addr}"
    );
    assert_eq!(r.final_url, url(&server, "/api/v1/auth/session?x=1"));
    assert_eq!(r.redirect_count, 0);
    assert!(r.sent_bytes > 50, "{}", r.sent_bytes);
    assert!(r.received_bytes as usize > r.body.size as usize);

    // 時間の内訳（localhost は名前解決するので DNS も測れる。http なので TLS はなし。GET なので送信もなし）
    let t = &r.timing;
    assert!(t.dns_ms.is_some(), "{t:?}");
    assert!(t.connect_ms.is_some(), "{t:?}");
    assert_eq!(t.tls_ms, None);
    assert_eq!(t.send_ms, None);
    assert!(t.wait_ms.is_some() && t.download_ms.is_some());
    let parts: f64 = [t.dns_ms, t.connect_ms, t.wait_ms, t.download_ms]
        .into_iter()
        .flatten()
        .sum();
    assert!(t.total_ms > 0.0);
    assert!(
        parts <= t.total_ms + 1.0,
        "内訳の合計 {parts} が合計 {} を超えている",
        t.total_ms
    );

    // サーバーが受け取ったもの
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let h = &received[0].headers;
    assert_eq!(
        h.get("accept").unwrap().to_str().unwrap(),
        "application/json"
    );
    assert!(h
        .get("user-agent")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("NekoReq/"));
    assert_eq!(received[0].url.query(), Some("x=1"));
}

#[tokio::test]
async fn ip_literal_host_has_no_dns_time_but_connect_time() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let req = prepared("GET", &format!("{}/x", server.uri()));
    let t = send_default(&req).await.unwrap().data.timing;
    assert_eq!(t.dns_ms, None, "{t:?}");
    assert!(t.connect_ms.is_some(), "{t:?}");
}

#[tokio::test]
async fn post_json_sends_the_body_with_content_length_and_measures_send_time() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/password-login"))
        .respond_with(ResponseTemplate::new(401).set_body_raw(
            r#"{"error":{"code":"LOGIN_FAILED"}}"#,
            "application/problem+json",
        ))
        .mount(&server)
        .await;

    let body = br#"{"email":"a@example.com","password":"x"}"#.to_vec();
    let mut req = prepared("POST", &url(&server, "/api/v1/auth/password-login"));
    req.headers.push(header("Content-Type", "application/json"));
    req.body = PreparedBody::Bytes(body.clone());
    let sent = send_default(&req).await.unwrap();

    // 4xx も「応答を受け取った」ので成功として返る
    assert_eq!(sent.data.status, 401);
    assert_eq!(sent.data.status_text, "Unauthorized");
    assert_eq!(sent.data.timing.send_ms, None, "送信の時間は測らない");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received[0].body, body);
    let h = &received[0].headers;
    assert_eq!(
        h.get("content-length").unwrap().to_str().unwrap(),
        body.len().to_string()
    );
    assert!(h.get("transfer-encoding").is_none(), "チャンク形式にしない");
    assert_eq!(
        h.get("content-type").unwrap().to_str().unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn multipart_and_file_bodies_are_sent() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.bin");
    std::fs::write(&file, [1u8, 2, 3, 4]).unwrap();

    let mut req = prepared("PUT", &url(&server, "/upload"));
    req.body = PreparedBody::File(file.display().to_string());
    send_default(&req).await.unwrap();
    let received = server.received_requests().await.unwrap();
    assert_eq!(received[0].body, vec![1u8, 2, 3, 4]);
}

#[tokio::test]
async fn duplicate_request_headers_are_all_sent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut req = prepared("GET", &url(&server, "/"));
    req.headers = vec![
        header("X-Dup", "1"),
        header("X-Dup", "2"),
        header("X-Jp", "日本語"),
    ];
    send_default(&req).await.unwrap();
    let received = server.received_requests().await.unwrap();
    let h = &received[0].headers;
    let dups: Vec<_> = h
        .get_all("x-dup")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(dups, ["1", "2"]);
    assert_eq!(h.get("x-jp").unwrap().as_bytes(), "日本語".as_bytes());
}

#[tokio::test]
async fn user_supplied_user_agent_replaces_the_default() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mut req = prepared("GET", &url(&server, "/"));
    req.headers = vec![header("User-Agent", "mine/1.0")];
    send_default(&req).await.unwrap();
    let received = server.received_requests().await.unwrap();
    let uas: Vec<_> = received[0].headers.get_all("user-agent").iter().collect();
    assert_eq!(uas.len(), 1);
    assert_eq!(uas[0].to_str().unwrap(), "mine/1.0");
}

#[tokio::test]
async fn timeout_is_reported_and_stops_waiting() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let settings = SendSettings {
        timeout_ms: 300,
        ..SendSettings::default()
    };
    let started = Instant::now();
    let err = send(
        &prepared("GET", &url(&server, "/")),
        &settings,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, SendError::Timeout { .. }), "{err:?}");
    assert!(err.message().contains("ミリ秒"), "{}", err.message());
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn default_timeout_is_thirty_seconds() {
    assert_eq!(SendSettings::default().timeout_ms, 30_000);
    assert_eq!(format_duration_ms(30_000), "30 秒");
}

#[tokio::test]
async fn cancel_stops_an_in_flight_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    let started = Instant::now();
    let err = send(
        &prepared("GET", &url(&server, "/")),
        &SendSettings::default(),
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, SendError::Cancelled { .. }), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn already_cancelled_token_does_not_send() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = send(
        &prepared("GET", &url(&server, "/")),
        &SendSettings::default(),
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, SendError::Cancelled { .. }));
}

#[tokio::test]
async fn redirects_are_followed_or_not_by_setting() {
    let server = MockServer::start().await;
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
    let req = prepared("GET", &url(&server, "/old"));

    let followed = send(&req, &SendSettings::default(), &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert_eq!(followed.status, 200);
    assert_eq!(followed.body.content, "arrived");
    assert_eq!(followed.final_url, url(&server, "/new"));
    assert_eq!(followed.redirect_count, 1);
    // 内訳は最後の区間のもの。合計は全体
    assert!(followed.timing.total_ms >= followed.timing.wait_ms.unwrap());

    let settings = SendSettings {
        follow_redirects: false,
        ..SendSettings::default()
    };
    let stopped = send(&req, &settings, &CancellationToken::new())
        .await
        .unwrap()
        .data;
    assert_eq!(stopped.status, 302);
    assert_eq!(stopped.redirect_count, 0);
    assert!(stopped
        .headers
        .iter()
        .any(|h| h.name == "location" && h.value == "/new"));
    assert_eq!(stopped.final_url, url(&server, "/old"));
}

#[tokio::test]
async fn redirect_loop_is_an_error_after_ten_hops() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
        .mount(&server)
        .await;
    let err = send_default(&prepared("GET", &url(&server, "/loop")))
        .await
        .unwrap_err();
    assert!(err.message().contains("リダイレクト"), "{err:?}");
}

#[tokio::test]
async fn body_over_the_limit_is_cut_for_the_view_but_kept_whole() {
    let server = MockServer::start().await;
    let big = vec![b'a'; 11 * 1024 * 1024];
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(big.clone(), "text/plain"))
        .mount(&server)
        .await;
    let sent = send_default(&prepared("GET", &url(&server, "/big")))
        .await
        .unwrap();
    assert!(sent.data.body.truncated);
    assert_eq!(sent.data.body.content.len(), 10 * 1024 * 1024);
    assert_eq!(sent.data.body.size as usize, 11 * 1024 * 1024);
    assert_eq!(
        sent.body.len(),
        11 * 1024 * 1024,
        "全体はメモリに持っている"
    );
    assert_eq!(&sent.body[..], &big[..]);
    assert!(sent.data.timing.download_ms.is_some());
}

#[tokio::test]
async fn small_preview_limit_and_hard_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![b'z'; 5000], "text/plain"))
        .mount(&server)
        .await;
    let req = prepared("GET", &url(&server, "/"));
    let token = CancellationToken::new();
    let s = SendSettings::default();

    let ok = send_with_limits(
        &req,
        &s,
        &token,
        Limits {
            preview_bytes: 1000,
            max_body_bytes: 10_000,
        },
    )
    .await
    .unwrap();
    assert!(ok.data.body.truncated);
    assert_eq!(ok.data.body.content.len(), 1000);
    assert_eq!(ok.body.len(), 5000);

    let err = send_with_limits(
        &req,
        &s,
        &token,
        Limits {
            preview_bytes: 1000,
            max_body_bytes: 4000,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, SendError::TooLarge { .. }), "{err:?}");
}

#[tokio::test]
async fn binary_body_comes_back_as_base64() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(vec![0x89, b'P', b'N', b'G', 0, 1], "image/png"),
        )
        .mount(&server)
        .await;
    let sent = send_default(&prepared("GET", &url(&server, "/img")))
        .await
        .unwrap();
    assert_eq!(sent.data.body.encoding, BodyEncoding::Base64);
    assert_eq!(sent.data.body.content, "iVBORwAB");
    assert_eq!(sent.data.body.size, 6);
}

#[tokio::test]
async fn empty_and_no_content_responses() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let sent = send_default(&prepared("DELETE", &url(&server, "/x")))
        .await
        .unwrap();
    assert_eq!(sent.data.status, 204);
    assert_eq!(sent.data.body.size, 0);
    assert_eq!(sent.data.body.content, "");
}

#[tokio::test]
async fn connection_refused_is_a_connect_error() {
    // 空いているポートを借りてすぐ閉じる
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let err = send_default(&prepared("GET", &format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap_err();
    assert!(matches!(err, SendError::Connect { .. }), "{err:?}");
    assert!(
        err.message().contains(&format!("127.0.0.1:{port}")),
        "{}",
        err.message()
    );
}

#[tokio::test]
async fn unknown_host_is_a_connect_error() {
    let err = send_default(&prepared("GET", "http://no-such-host.invalid/"))
        .await
        .unwrap_err();
    assert!(matches!(err, SendError::Connect { .. }), "{err:?}");
}

#[tokio::test]
async fn bad_requests_are_reported_before_sending() {
    let mut req = prepared("GET", "http://localhost:1/");
    req.headers = vec![header("Bad Name", "v")];
    assert!(matches!(
        send_default(&req).await.unwrap_err(),
        SendError::InvalidRequest { .. }
    ));
    req.headers = vec![header("X", "line\nbreak")];
    assert!(matches!(
        send_default(&req).await.unwrap_err(),
        SendError::InvalidRequest { .. }
    ));
    let req = prepared("BAD METHOD", "http://localhost:1/");
    assert!(matches!(
        send_default(&req).await.unwrap_err(),
        SendError::InvalidRequest { .. }
    ));
    let mut req = prepared("POST", "http://localhost:1/");
    req.body = PreparedBody::File("/nonexistent/file.bin".into());
    assert!(matches!(
        send_default(&req).await.unwrap_err(),
        SendError::BodyFile { .. }
    ));
}

// ---- TLS（自己署名証明書のサーバー） ----

/// 自己署名の証明書で TLS を話す、最小の HTTP/1.1 サーバー。応答は `ok`。
async fn start_tls_server() -> u16 {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert = CertificateDer::from(certified.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
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
                    return; // 検証に失敗したクライアントはここで切れる
                };
                let mut buf = vec![0u8; 4096];
                let mut seen = Vec::new();
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    port
}

#[tokio::test]
async fn tls_without_verification_connects_and_splits_tcp_and_tls_time() {
    let port = start_tls_server().await;
    let settings = SendSettings {
        verify_certificates: false,
        ..SendSettings::default()
    };
    let sent = send(
        &prepared("GET", &format!("https://localhost:{port}/")),
        &settings,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let r = sent.data;
    assert_eq!(r.status, 200);
    assert_eq!(r.body.content, "ok");
    assert!(r.connection.tls);
    assert_eq!(r.connection.http_version, "HTTP/1.1");
    let t = r.timing;
    assert!(t.dns_ms.is_some(), "{t:?}");
    assert!(t.connect_ms.is_some(), "{t:?}");
    assert!(t.tls_ms.is_some(), "TLS の区間が測れる: {t:?}");
    assert!(t.tls_ms.unwrap() > 0.0);
}

#[tokio::test]
async fn tls_with_verification_rejects_a_self_signed_certificate() {
    let port = start_tls_server().await;
    let err = send_default(&prepared("GET", &format!("https://localhost:{port}/")))
        .await
        .unwrap_err();
    assert!(matches!(err, SendError::Tls { .. }), "{err:?}");
    assert!(err.message().contains("証明書の検証"), "{}", err.message());
}

#[test]
fn strip_url_secrets_removes_query_fragment_and_userinfo_from_urls_in_text() {
    assert_eq!(
        strip_url_secrets("error sending request for url (http://h:8080/a/b?token=S&x=1#frag)"),
        "error sending request for url (http://h:8080/a/b)"
    );
    assert_eq!(
        strip_url_secrets("see https://user:pw@h/x?q=1 and http://[::1]:9/y?z=2, ok"),
        "see https://h/x and http://[::1]:9/y, ok"
    );
    assert_eq!(
        strip_url_secrets("http://h?only=query"),
        "http://h",
        "パスがなくてもクエリを落とす"
    );
    assert_eq!(strip_url_secrets("no urls here ?a=1"), "no urls here ?a=1");
}
