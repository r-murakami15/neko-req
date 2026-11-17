//! 「OS の設定を使う」プロキシで、環境変数に書かれた認証（`http://user:pw@host:port`）がプロキシへ送られ、
//! localhost は直接つなぐ。環境変数はプロセス全体のものなので、このファイルのテストは 1 つだけにする。

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
async fn system_proxy_keeps_its_credentials_and_skips_loopback() {
    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("via-env-proxy"))
        .mount(&proxy)
        .await;
    let target = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("direct"))
        .mount(&target)
        .await;
    let with_auth = proxy.uri().replace("http://", "http://neko:p%40ss@");
    for k in ["HTTP_PROXY", "http_proxy"] {
        std::env::set_var(k, &with_auth);
    }
    std::env::remove_var("NO_PROXY");
    std::env::remove_var("no_proxy");
    let transport = Transport {
        proxy: ProxyChoice::System,
        ..Transport::default()
    };
    let go = |url: String| {
        let transport = transport.clone();
        async move {
            send_with_transport(
                &req(&url),
                &SendSettings::default(),
                &CancellationToken::new(),
                &transport,
            )
            .await
            .unwrap()
        }
    };
    let sent = go("http://example.invalid:8123/env".into()).await;
    assert_eq!(sent.data.body.content, "via-env-proxy");
    let seen = proxy.received_requests().await.unwrap();
    assert_eq!(seen.len(), 1);
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("neko:p@ss")
    );
    assert_eq!(
        seen[0]
            .headers
            .get("proxy-authorization")
            .map(|v| v.to_str().unwrap().to_string()),
        Some(want)
    );

    let port = target.address().port();
    for host in ["127.0.0.1", "localhost"] {
        let sent = go(format!("http://{host}:{port}/local")).await;
        assert_eq!(sent.data.body.content, "direct", "{host}");
    }
    assert_eq!(proxy.received_requests().await.unwrap().len(), 1);
}
