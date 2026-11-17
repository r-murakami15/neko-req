//! 「OS の設定を使う」プロキシ（既定）のとき、localhost / 127.0.0.1 の API への送信がプロキシに行かないこと。
//!
//! このアプリは自分の PC の localhost の API（Dev Container）を叩くのが主な使い方。会社のネットワークなどで
//! `HTTP_PROXY` があり `NO_PROXY` がない環境でも、ローカルの API はプロキシを通さず直接つなぎたい。
//!
//! 環境変数はプロセス全体のもの（reqwest は最初の送信のときに 1 度だけ読んで覚える）なので、このファイルは
//! テストを 1 つだけにする。

use std::sync::Arc;
use std::time::Duration;

use nekoreq_core::{Body, RequestSpec, SendOutcome};
use nekoreq_lib::send::RequestInput;
use nekoreq_lib::state::AppState;
use nekoreq_secrets::MemorySecretStore;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn localhost_is_never_sent_through_the_environment_proxy() {
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
    for k in [
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
    ] {
        std::env::set_var(k, proxy.uri());
    }
    std::env::remove_var("NO_PROXY");
    std::env::remove_var("no_proxy");

    let dir = tempfile::tempdir().unwrap();
    let state =
        AppState::open_with_secrets(&dir.path().join("data"), Arc::new(MemorySecretStore::new()))
            .unwrap();
    let project = state.project_create("p").unwrap();
    let port = target.address().port();

    let mut via_proxy = Vec::new();
    for host in ["127.0.0.1", "localhost"] {
        let (tx, rx) = tokio::sync::oneshot::channel();
        state
            .request_send(
                RequestInput {
                    project_id: project.id.clone(),
                    environment_id: None,
                    request: RequestSpec {
                        method: "GET".into(),
                        url: format!("http://{host}:{port}/api"),
                        params: vec![],
                        body: Body::None,
                        auth: None,
                    },
                    overrides: vec![],
                    op_key: None,
                },
                None,
                move |r| {
                    let _ = tx.send(r);
                },
            )
            .unwrap();
        let r = tokio::time::timeout(Duration::from_secs(15), rx)
            .await
            .unwrap()
            .unwrap();
        match r.result {
            SendOutcome::Success { response } if response.body.content == "direct" => {}
            SendOutcome::Success { response } => {
                via_proxy.push(format!("{host}: {}", response.body.content))
            }
            SendOutcome::Failure { error } => {
                via_proxy.push(format!("{host}: {}", error.message()))
            }
        }
    }
    let seen = proxy.received_requests().await.unwrap().len();
    assert!(
        via_proxy.is_empty() && seen == 0,
        "プロキシが受け取った: {seen} 件 / {via_proxy:?}"
    );
}
