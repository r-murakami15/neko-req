//! プロキシの設定に関わらず、localhost・127.0.0.1 は直接つなぐ（手動のプロキシ）。
//! 「OS の設定を使う」のほうは app の結合テスト（`m8_audit_proxy_env`）と `proxy_system_auth` が確かめる。

use base64::Engine as _;
use nekoreq_core::{HeaderPair, PreparedBody, PreparedRequest, SendSettings};
use nekoreq_http::{send_with_transport, CancellationToken, ProxyChoice, Transport};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req(url: &str) -> PreparedRequest {
    PreparedRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Vec::<HeaderPair>::new(),
        body: PreparedBody::None,
        secrets: vec![],
    }
}

#[tokio::test]
async fn manual_proxy_is_not_used_for_loopback_destinations() {
    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("via-proxy"))
        .mount(&proxy)
        .await;
    let target = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("direct"))
        .mount(&target)
        .await;
    let transport = Transport {
        proxy: ProxyChoice::Manual {
            url: proxy.uri(),
            username: Some("neko".into()),
            password: Some("pw".into()),
            no_proxy: vec![],
        },
        ..Transport::default()
    };
    let port = target.address().port();
    for host in ["127.0.0.1", "localhost"] {
        let sent = send_with_transport(
            &req(&format!("http://{host}:{port}/x")),
            &SendSettings::default(),
            &CancellationToken::new(),
            &transport,
        )
        .await
        .unwrap();
        assert_eq!(sent.data.body.content, "direct", "{host}");
    }
    assert!(proxy.received_requests().await.unwrap().is_empty());

    // ほかの宛先はプロキシを通る（認証つき）
    let sent = send_with_transport(
        &req("http://example.invalid:8123/y"),
        &SendSettings::default(),
        &CancellationToken::new(),
        &transport,
    )
    .await
    .unwrap();
    assert_eq!(sent.data.body.content, "via-proxy");
    let seen = proxy.received_requests().await.unwrap();
    assert_eq!(seen.len(), 1);
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("neko:pw")
    );
    assert_eq!(
        seen[0]
            .headers
            .get("proxy-authorization")
            .map(|v| v.to_str().unwrap().to_string()),
        Some(want)
    );
}
