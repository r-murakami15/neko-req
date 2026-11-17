//! 設定ファイルの取得（fetch_bytes）。アクセストークンの扱い・リダイレクト・上限・エラー文（M3）。

use std::time::Duration;

use nekoreq_http::fetch_bytes;
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const T: Duration = Duration::from_secs(5);

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[tokio::test]
async fn sends_the_token_only_when_it_is_not_empty() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&s)
        .await;
    let url = u(&format!("{}/x", s.uri()));
    assert_eq!(
        fetch_bytes(&url, Some("tok"), T, 1024).await.unwrap(),
        b"ok"
    );
    fetch_bytes(&url, Some(""), T, 1024).await.unwrap();
    fetch_bytes(&url, None, T, 1024).await.unwrap();
    let reqs = s.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 3);
    assert_eq!(
        reqs[0]
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        "Bearer tok"
    );
    assert!(reqs[1].headers.get("authorization").is_none());
    assert!(reqs[2].headers.get("authorization").is_none());
}

#[tokio::test]
async fn the_token_does_not_follow_a_redirect_to_another_port_or_host() {
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .and(header("authorization", "Bearer SECRET"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", format!("{}/end", b.uri())),
        )
        .mount(&a)
        .await;
    Mock::given(method("GET"))
        .and(path("/end"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .mount(&b)
        .await;
    let body = fetch_bytes(&u(&format!("{}/start", a.uri())), Some("SECRET"), T, 1024)
        .await
        .unwrap();
    assert_eq!(body, b"done");
    let reqs = b.received_requests().await.unwrap();
    assert!(reqs[0].headers.get("authorization").is_none());
    assert!(reqs[0].headers.get("cookie").is_none());
}

#[tokio::test]
async fn errors_never_contain_the_token_the_query_or_the_userinfo() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Bearer SECRET-IN-BODY"))
        .mount(&s)
        .await;
    let host = s.uri().trim_start_matches("http://").to_string();
    let url = u(&format!(
        "http://me:PASSWORD@{host}/p?access_token=QUERYSECRET#frag"
    ));
    let e = fetch_bytes(&url, Some("HEADERSECRET"), T, 1024)
        .await
        .unwrap_err();
    assert_eq!(e.status, Some(403));
    for secret in [
        "PASSWORD",
        "QUERYSECRET",
        "HEADERSECRET",
        "SECRET-IN-BODY",
        "access_token",
    ] {
        assert!(!e.message.contains(secret), "{secret}: {}", e.message);
        assert!(!format!("{e:?}").contains(secret), "{secret}: {e:?}");
    }
    assert!(e.message.contains("アクセストークン"));
    // 接続できないとき
    let dead = u("http://user:PASSWORD@127.0.0.1:1/p?access_token=QUERYSECRET");
    let e = fetch_bytes(&dead, Some("HEADERSECRET"), T, 1024)
        .await
        .unwrap_err();
    for secret in ["PASSWORD", "QUERYSECRET", "HEADERSECRET"] {
        assert!(!e.message.contains(secret), "{secret}: {}", e.message);
    }
}

#[tokio::test]
async fn a_token_with_a_trailing_newline_or_space_gives_a_clean_error_or_works() {
    // 画面から貼り付けたトークンの末尾に改行が付いていることがある
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&s)
        .await;
    let url = u(&format!("{}/x", s.uri()));
    for tok in ["abc\n", "abc ", "abc\r\n", "日本語"] {
        match fetch_bytes(&url, Some(tok), T, 1024).await {
            Ok(_) => eprintln!("token {tok:?}: ok"),
            Err(e) => {
                eprintln!("token {tok:?}: {}", e.message);
                assert!(!e.message.contains(tok.trim()), "{tok:?}: {}", e.message);
                assert!(!e.message.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn the_size_limit_applies_to_chunked_bodies_and_the_timeout_to_slow_servers() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 100_000]))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(3))
                .set_body_string("late"),
        )
        .mount(&s)
        .await;
    let e = fetch_bytes(&u(&format!("{}/big", s.uri())), None, T, 50_000)
        .await
        .unwrap_err();
    assert!(e.message.contains("大きすぎ"), "{}", e.message);
    // ちょうど上限は通る
    assert_eq!(
        fetch_bytes(&u(&format!("{}/big", s.uri())), None, T, 100_000)
            .await
            .unwrap()
            .len(),
        100_000
    );
    let t = std::time::Instant::now();
    let e = fetch_bytes(
        &u(&format!("{}/slow", s.uri())),
        None,
        Duration::from_millis(300),
        1024,
    )
    .await
    .unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(e.status.is_none());
}

#[tokio::test]
async fn too_many_redirects_and_non_http_redirect_targets_are_errors() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/loop"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/file"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "file:///etc/passwd"))
        .mount(&s)
        .await;
    for p in ["loop", "file"] {
        let e = fetch_bytes(&u(&format!("{}/{p}", s.uri())), Some("T"), T, 1024)
            .await
            .unwrap_err();
        assert!(!e.message.is_empty(), "{p}");
    }
    // ループは 10 回で止まる（無限に続けない）
    let n = s
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/loop")
        .count();
    assert!(n <= 12, "{n}");
}
