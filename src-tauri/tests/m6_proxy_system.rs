//! 設定の「OS の設定を使う」プロキシ（既定）。環境変数（`HTTP_PROXY`）はプロセス全体のものなので、
//! このファイルのテストは 1 つだけにする（macOS・Windows のシステム設定の読み取りは reqwest の
//! `system-proxy` 機能で、ここでは環境変数だけを確かめる）。

use std::sync::Arc;
use std::time::Duration;

use nekoreq_core::{Body, ProxyMode, RequestSpec, SendOutcome};
use nekoreq_lib::send::RequestInput;
use nekoreq_lib::state::AppState;
use nekoreq_secrets::MemorySecretStore;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn default_proxy_mode_follows_the_environment_and_none_ignores_it() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("via-env-proxy"))
        .mount(&proxy)
        .await;
    std::env::set_var("HTTP_PROXY", proxy.uri());
    std::env::set_var("http_proxy", proxy.uri());
    std::env::remove_var("NO_PROXY");
    std::env::remove_var("no_proxy");

    let dir = tempfile::tempdir().unwrap();
    let state =
        AppState::open_with_secrets(&dir.path().join("data"), Arc::new(MemorySecretStore::new()))
            .unwrap();
    let project = state.project_create("p").unwrap();
    assert_eq!(
        state.app_settings_get().unwrap().network.proxy.mode,
        ProxyMode::System,
        "既定は OS の設定"
    );
    let input = || RequestInput {
        project_id: project.id.clone(),
        environment_id: None,
        request: RequestSpec {
            method: "GET".into(),
            url: "http://target.invalid:8123/env".into(),
            params: vec![],
            body: Body::None,
            auth: None,
        },
        overrides: vec![],
        op_key: None,
    };
    let send = |state: AppState| async move {
        let (tx, rx) = tokio::sync::oneshot::channel();
        state
            .request_send(input(), None, move |r| {
                let _ = tx.send(r);
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .unwrap()
            .unwrap()
    };

    let r = send(state.clone()).await;
    let SendOutcome::Success { response } = r.result else {
        panic!("{:?}", r.result)
    };
    assert_eq!(response.body.content, "via-env-proxy");
    assert_eq!(proxy.received_requests().await.unwrap().len(), 1);

    // 「使わない」は環境変数があっても直接つなぐ（名前を引けずに失敗する）
    let mut s = state.app_settings_get().unwrap();
    s.network.proxy.mode = ProxyMode::None;
    state.app_settings_set(&s).unwrap();
    let r = send(state.clone()).await;
    assert!(matches!(r.result, SendOutcome::Failure { .. }));
    assert_eq!(proxy.received_requests().await.unwrap().len(), 1);
}
