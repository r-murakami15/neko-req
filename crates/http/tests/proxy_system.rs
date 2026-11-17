//! 「OS の設定を使う」プロキシ。環境変数（`HTTP_PROXY`）を読む動きを確かめる。
//! 環境変数はプロセス全体のものなので、このファイルのテストは 1 つだけにする（macOS・Windows の
//! システム設定の読み取りは reqwest の `system-proxy` 機能で、ここでは環境変数だけを確かめる）。

use nekoreq_core::{HeaderPair, PreparedBody, PreparedRequest, SendSettings};
use nekoreq_http::{send_with_transport, CancellationToken, ProxyChoice, Transport};
use wiremock::matchers::method;
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
async fn system_mode_follows_the_environment_and_none_ignores_it() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("via-env-proxy"))
        .mount(&proxy)
        .await;
    std::env::set_var("HTTP_PROXY", proxy.uri());
    std::env::set_var("http_proxy", proxy.uri());
    std::env::remove_var("NO_PROXY");
    std::env::remove_var("no_proxy");

    let go = |choice: ProxyChoice| {
        let r = req("http://target.invalid:8123/env");
        async move {
            send_with_transport(
                &r,
                &SendSettings::default(),
                &CancellationToken::new(),
                &Transport {
                    proxy: choice,
                    ..Transport::default()
                },
            )
            .await
        }
    };

    let sent = go(ProxyChoice::System).await.unwrap();
    assert_eq!(sent.data.body.content, "via-env-proxy");
    assert_eq!(proxy.received_requests().await.unwrap().len(), 1);

    // 「使わない」は環境変数があっても直接つなぐ（名前を引けずに失敗する）
    assert!(go(ProxyChoice::None).await.is_err());
    assert_eq!(proxy.received_requests().await.unwrap().len(), 1);
}
