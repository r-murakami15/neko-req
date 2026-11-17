//! リダイレクトの途中のレスポンスの `Set-Cookie`（SESSION・XSRF-TOKEN）を、同じ origin の追従先に付ける。
//! 途中のレスポンスの `Set-Cookie` は `SentResponse.redirect_hops` で返る。

use nekoreq_core::{HeaderPair, PreparedBody, PreparedRequest, SendSettings};
use nekoreq_http::{send_with_transport, CancellationToken, RedirectCookies, Transport};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn req(method: &str, url: &str, headers: &[(&str, &str)]) -> PreparedRequest {
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

async fn server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|r: &Request| {
            let h = |n: &str| {
                r.headers
                    .get(n)
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default()
            };
            match r.url.path() {
                "/login" => ResponseTemplate::new(307)
                    .insert_header("location", "/home")
                    .append_header("set-cookie", "SESSION=NEW-S; Path=/; HttpOnly")
                    .append_header("set-cookie", "XSRF-TOKEN=NEW-X; Path=/")
                    .append_header("set-cookie", "other=1"),
                "/logout" => ResponseTemplate::new(302)
                    .insert_header("location", "/home")
                    .append_header("set-cookie", "SESSION=; Max-Age=0; Path=/"),
                _ => ResponseTemplate::new(200).set_body_string(format!(
                    "cookie={}|xsrf={}",
                    h("cookie"),
                    h("x-xsrf-token")
                )),
            }
        })
        .mount(&server)
        .await;
    server
}

async fn go(r: &PreparedRequest, rc: Option<RedirectCookies>) -> nekoreq_http::SentResponse {
    send_with_transport(
        r,
        &SendSettings::default(),
        &CancellationToken::new(),
        &Transport {
            redirect_cookies: rc,
            ..Transport::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn new_session_and_csrf_from_a_redirect_are_sent_to_the_same_origin() {
    let s = server().await;
    let rc = Some(RedirectCookies {
        origin: s.uri(),
        session: true,
        csrf: true,
    });
    let sent = go(
        &req(
            "POST",
            &format!("{}/login", s.uri()),
            &[
                ("Cookie", "SESSION=OLD-S; theme=dark"),
                ("X-XSRF-TOKEN", "OLD-X"),
            ],
        ),
        rc.clone(),
    )
    .await;
    // 307 はメソッドもそのまま。Cookie は新しい値に置き換わり、ほかの Cookie は残る
    let body = sent.data.body.content;
    assert!(body.contains("SESSION=NEW-S"), "{body}");
    assert!(body.contains("XSRF-TOKEN=NEW-X"), "{body}");
    assert!(
        body.contains("theme=dark") && !body.contains("OLD-S"),
        "{body}"
    );
    assert!(body.ends_with("xsrf=NEW-X"), "{body}");
    // 途中のレスポンスの Set-Cookie は、呼び出し側が読めるように返る
    assert_eq!(sent.redirect_hops.len(), 1);
    assert!(sent.redirect_hops[0].url.ends_with("/login"));
    assert_eq!(sent.redirect_hops[0].set_cookies.len(), 3);

    // Cookie がなかったリクエストにも付く（最初のログイン）
    let sent = go(&req("POST", &format!("{}/login", s.uri()), &[]), rc.clone()).await;
    assert!(sent.data.body.content.contains("SESSION=NEW-S"));

    // 消す指示（Max-Age=0）なら外す
    let sent = go(
        &req(
            "GET",
            &format!("{}/logout", s.uri()),
            &[("Cookie", "SESSION=OLD-S; a=b")],
        ),
        rc.clone(),
    )
    .await;
    assert_eq!(sent.data.body.content, "cookie=a=b|xsrf=");

    // 設定がなければ何もしない（これまでどおり）
    let sent = go(
        &req(
            "GET",
            &format!("{}/login", s.uri()),
            &[("Cookie", "SESSION=OLD-S")],
        ),
        None,
    )
    .await;
    assert!(sent.data.body.content.contains("SESSION=OLD-S"));
    // SESSION の自動化だけオフなら、XSRF-TOKEN だけ更新する
    let sent = go(
        &req(
            "GET",
            &format!("{}/login", s.uri()),
            &[("Cookie", "SESSION=OLD-S")],
        ),
        Some(RedirectCookies {
            origin: s.uri(),
            session: false,
            csrf: true,
        }),
    )
    .await;
    let body = sent.data.body.content;
    assert!(
        body.contains("SESSION=OLD-S") && body.contains("XSRF-TOKEN=NEW-X"),
        "{body}"
    );
}

#[tokio::test]
async fn cookies_are_not_added_when_the_origin_is_not_the_automation_target() {
    let s = server().await;
    let sent = go(
        &req("GET", &format!("{}/login", s.uri()), &[]),
        Some(RedirectCookies {
            origin: "http://other.example:1".into(),
            session: true,
            csrf: true,
        }),
    )
    .await;
    assert_eq!(sent.data.body.content, "cookie=|xsrf=");
}
