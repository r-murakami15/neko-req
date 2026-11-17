//! M6 の cURL 取り込み（`curl_parse`）を `AppState` 経由で確かめる。完了条件「cURL が該当する API に当てはまる」。
//!
//! 実際の API 定義（`NEKOREQ_FIXTURE_DIR`。コンテナの `/fixtures/license-api`）を取り込んだプロジェクトに対する
//! テストは、無い環境ではスキップする。そのほかは、テストの中で作る小さな yaml で確かめる。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nekoreq_core::{
    AuthOverride, Body, EnvVar, ImportOptions, MultipartValue, OpKey, ParamIn, Project,
    SendOutcome, VarValue,
};
use nekoreq_curl::WarningKind;
use nekoreq_lib::curl::{CurlImport, CurlTarget};
use nekoreq_lib::openapi::{ImportRequest, ImportResult, SourceInput};
use nekoreq_lib::send::{RequestInput, VarOverride};
use nekoreq_lib::state::AppState;
use nekoreq_secrets::MemorySecretStore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Fx {
    _dir: tempfile::TempDir,
    state: AppState,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let state =
        AppState::open_with_secrets(&dir.path().join("data"), Arc::new(MemorySecretStore::new()))
            .unwrap();
    Fx { _dir: dir, state }
}

async fn import(f: &Fx, entry: PathBuf) -> Project {
    let r = f
        .state
        .openapi_import(ImportRequest {
            source: SourceInput::Path {
                path: entry.display().to_string(),
                allowed_hosts: vec![],
            },
            project_id: None,
            project_name: None,
            selected: None,
            options: ImportOptions::default(),
        })
        .await
        .unwrap();
    match r {
        ImportResult::Imported { project, .. } => *project,
        other => panic!("取り込めるはず: {other:?}"),
    }
}

fn fixture_entry() -> Option<PathBuf> {
    let dir = std::env::var_os("NEKOREQ_FIXTURE_DIR")?;
    let entry = PathBuf::from(dir).join("openapi.yaml");
    entry.exists().then_some(entry)
}

fn param<'a>(
    c: &'a CurlImport,
    location: ParamIn,
    name: &str,
) -> Option<&'a nekoreq_core::KeyValue> {
    c.request
        .params
        .iter()
        .find(|p| p.location == location && p.name == name)
}

fn key(id: &str) -> OpKey {
    OpKey::Id(id.into())
}

// ---- 実際の API 定義 ----

#[tokio::test]
async fn real_fixture_curl_matches_the_documented_apis() {
    let Some(entry) = fixture_entry() else {
        eprintln!("NEKOREQ_FIXTURE_DIR が無いのでスキップ");
        return;
    };
    let f = fx();
    let project = import(&f, entry).await;
    let parse = |text: &str| f.state.curl_parse(&project.id, text, None).unwrap();

    // 設計書の完了条件: 取引先一覧取得
    let c = parse("curl -X GET 'http://localhost:8080/api/v1/video/clients?q=Netflix'");
    assert_eq!(c.target, CurlTarget::Operation);
    assert_eq!(c.op_key, Some(key("getClientList")));
    assert_eq!(c.matched_path.as_deref(), Some("/api/v1/video/clients"));
    assert!(c.base_replaced);
    assert_eq!(c.request.url, "{{baseUrl}}/api/v1/video/clients");
    assert_eq!(c.request.method, "GET");
    let q = param(&c, ParamIn::Query, "q").expect("q が入る");
    assert_eq!(q.value, "Netflix");
    assert!(q.enabled);
    assert!(c.environment_id.is_some(), "baseUrl が同じ環境");
    assert!(c.summary.is_some());
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);

    // path パラメータ付き（取引先削除。この path に GET の API はない）
    let c = parse("curl -X DELETE http://localhost:8080/api/v1/video/clients/123");
    assert_eq!(c.target, CurlTarget::Operation);
    assert_eq!(c.op_key, Some(key("deleteClient")));
    assert_eq!(
        c.matched_path.as_deref(),
        Some("/api/v1/video/clients/{clientId}")
    );
    assert_eq!(c.request.url, "{{baseUrl}}/api/v1/video/clients/{clientId}");
    let id = param(&c, ParamIn::Path, "clientId").expect("path パラメータ");
    assert_eq!((id.value.as_str(), id.enabled), ("123", true));

    // 取引先詳細（GET）。path パラメータは 123
    let c = parse("curl 'http://localhost:8080/api/v1/video/clients/123/detail-view'");
    assert_eq!(c.target, CurlTarget::Operation);
    assert_eq!(c.op_key, Some(key("getClientDetail")));
    assert_eq!(param(&c, ParamIn::Path, "clientId").unwrap().value, "123");

    // method が合わなければ「その他」
    let c = parse("curl http://localhost:8080/api/v1/video/clients/123");
    assert_eq!(c.target, CurlTarget::Adhoc);
    assert_eq!(c.op_key, None);
    assert!(c.base_replaced);
    assert_eq!(c.request.url, "{{baseUrl}}/api/v1/video/clients/123");
    assert_eq!(c.suggested_name, "GET /api/v1/video/clients/123");

    // 固定の区間が多いほうを優先（payees/save は payees/{payeeId} より優先）。method が違えば {payeeId} のほう
    let c = parse("curl -X PUT http://localhost:8080/api/v1/video/payees/save -H 'Content-Type: application/json' -d '{}'");
    assert_eq!(c.target, CurlTarget::Operation);
    assert_eq!(c.op_key, Some(key("savePayee")));
    assert_eq!(c.matched_path.as_deref(), Some("/api/v1/video/payees/save"));
    assert_eq!(c.request.body, Body::Json("{}".into()));
    let c = parse("curl http://localhost:8080/api/v1/video/payees/save");
    assert_eq!(c.op_key, Some(key("getPayeeDetail")));
    assert_eq!(param(&c, ParamIn::Path, "payeeId").unwrap().value, "save");

    // ログイン（JSON ボディ）
    let c = parse(
        "curl -X POST http://localhost:8080/api/v1/auth/password-login \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"email\":\"user@example.com\",\"password\":\"x\"}'",
    );
    assert_eq!(c.op_key, Some(key("createPasswordLogin")));
    assert_eq!(
        c.request.body,
        Body::Json(r#"{"email":"user@example.com","password":"x"}"#.into())
    );

    // 当てはまらない path / 別のホストは「その他」
    let c = parse("curl http://localhost:8080/no/such/path");
    assert_eq!(c.target, CurlTarget::Adhoc);
    assert!(c.base_replaced);
    let c = parse("curl https://api.other.example.com/api/v1/video/clients");
    assert_eq!(c.target, CurlTarget::Adhoc);
    assert!(!c.base_replaced);
    assert_eq!(
        c.request.url,
        "https://api.other.example.com/api/v1/video/clients"
    );
    assert!(
        c.warnings
            .iter()
            .any(|w| w.message.contains("getClientList")
                || w.message.contains("/api/v1/video/clients")),
        "{:?}",
        c.warnings
    );
}

#[tokio::test]
async fn real_fixture_chrome_style_curl() {
    let Some(entry) = fixture_entry() else {
        return;
    };
    let f = fx();
    let project = import(&f, entry).await;
    let c = f
        .state
        .curl_parse(
            &project.id,
            r#"curl 'http://localhost:8080/api/v1/video/clients?q=Netflix' \
  -H 'accept: application/json' \
  -H 'accept-encoding: gzip, deflate, br, zstd' \
  -H 'x-xsrf-token: abc' \
  -b 'SESSION=sess1; XSRF-TOKEN=abc' \
  --compressed -k"#,
            None,
        )
        .unwrap();
    assert_eq!(c.op_key, Some(key("getClientList")));
    let enc = param(&c, ParamIn::Header, "accept-encoding").unwrap();
    assert_eq!(enc.value, "gzip, deflate, br", "zstd は外す");
    assert!(c
        .warnings
        .iter()
        .any(|w| w.kind == WarningKind::Adjusted && w.message.contains("zstd")));
    // 無視したオプションが警告に出る
    assert!(
        c.warnings.iter().any(|w| w.kind == WarningKind::Ignored),
        "{:?}",
        c.warnings
    );
    assert_eq!(
        param(&c, ParamIn::Cookie, "SESSION").unwrap().value,
        "sess1"
    );
    assert_eq!(
        param(&c, ParamIn::Header, "x-xsrf-token").unwrap().value,
        "abc"
    );
}

// ---- 小さな yaml ----

const SPEC: &str = r#"openapi: 3.0.3
info: {title: T, version: '1'}
servers:
  - url: http://localhost:8080
    description: local
paths:
  /items/{id}:
    get:
      operationId: getItem
      summary: 1 件取得
      parameters:
        - {name: id, in: path, required: true, schema: {type: string}}
        - {name: expand, in: query, schema: {type: string}}
        - {name: limit, in: query, schema: {type: integer}}
      responses: {'200': {description: ok}}
    delete:
      operationId: deleteItem
      parameters:
        - {name: id, in: path, required: true, schema: {type: string}}
      responses: {'204': {description: ok}}
  /items/search:
    get:
      operationId: searchItems
      responses: {'200': {description: ok}}
  /items:
    post:
      operationId: createItem
      requestBody:
        content:
          application/json:
            schema: {type: object}
      responses: {'201': {description: ok}}
  /upload:
    post:
      operationId: upload
      requestBody:
        content:
          multipart/form-data:
            schema: {type: object, properties: {file: {type: string, format: binary}, name: {type: string}}}
      responses: {'200': {description: ok}}
"#;

async fn small_project(f: &Fx) -> (Project, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let entry = tmp.path().join("openapi.yaml");
    std::fs::write(&entry, SPEC).unwrap();
    let p = import(f, entry).await;
    (p, tmp)
}

#[tokio::test]
async fn literal_segments_win_and_query_params_fill_the_apis_rows() {
    let f = fx();
    let (p, _tmp) = small_project(&f).await;
    let parse = |t: &str| f.state.curl_parse(&p.id, t, None).unwrap();

    // 固定の文字が多いほう
    let c = parse("curl http://localhost:8080/items/search");
    assert_eq!(c.op_key, Some(key("searchItems")));

    // path パラメータ・クエリ・yaml のほかのパラメータは無効の行で並ぶ
    let c = parse("curl 'http://localhost:8080/items/42?expand=owner&unknown=1'");
    assert_eq!(c.op_key, Some(key("getItem")));
    assert_eq!(c.summary.as_deref(), Some("1 件取得"));
    assert_eq!(param(&c, ParamIn::Path, "id").unwrap().value, "42");
    let expand = param(&c, ParamIn::Query, "expand").unwrap();
    assert_eq!((expand.value.as_str(), expand.enabled), ("owner", true));
    let unknown = param(&c, ParamIn::Query, "unknown").unwrap();
    assert!(unknown.enabled, "yaml にないパラメータも入力として残す");
    let limit = param(&c, ParamIn::Query, "limit").unwrap();
    assert!(!limit.enabled, "yaml にあって cURL にないものは無効の行");
    // 同じ名前の行が重ならない
    assert_eq!(
        c.request
            .params
            .iter()
            .filter(|p| p.location == ParamIn::Query && p.name == "expand")
            .count(),
        1
    );

    // パーセントエンコードされた path の値はデコードされる
    let c = parse("curl 'http://localhost:8080/items/a%2Fb%20c'");
    assert_eq!(param(&c, ParamIn::Path, "id").unwrap().value, "a/b c");

    // method で分かれる
    assert_eq!(
        parse("curl -X DELETE http://localhost:8080/items/9").op_key,
        Some(key("deleteItem"))
    );
    assert_eq!(
        parse("curl -X PATCH http://localhost:8080/items/9").target,
        CurlTarget::Adhoc
    );
    // 末尾の / は無視する
    assert_eq!(
        parse("curl http://localhost:8080/items/search/").op_key,
        Some(key("searchItems"))
    );
    // `-d` があれば method は POST（createItem）
    let c = parse(
        "curl http://localhost:8080/items -H 'Content-Type: application/json' --json '{\"a\":1}'",
    );
    assert_eq!(c.op_key, Some(key("createItem")));
    assert!(matches!(c.request.body, Body::Json(_)));
    // multipart（`;type=` を持つ）
    let c = parse(
        "curl http://localhost:8080/upload -F 'name=neko' -F 'file=@/tmp/a.png;type=image/png'",
    );
    assert_eq!(c.op_key, Some(key("upload")));
    let Body::Multipart(parts) = &c.request.body else {
        panic!("{:?}", c.request.body)
    };
    assert_eq!(parts[1].value, MultipartValue::File("/tmp/a.png".into()));
    assert_eq!(parts[1].content_type.as_deref(), Some("image/png"));
    assert!(
        !c.warnings.iter().any(|w| w.message.contains("type=")),
        "`;type=` は保持するので警告は出ない: {:?}",
        c.warnings
    );
}

#[tokio::test]
async fn auth_and_adhoc_urls() {
    let f = fx();
    let (p, _tmp) = small_project(&f).await;
    let c = f
        .state
        .curl_parse(
            &p.id,
            "curl -u neko:pw http://localhost:8080/items/1 -H 'Authorization: Bearer tok'",
            None,
        )
        .unwrap();
    // Authorization ヘッダーがあれば -u は使われない（curl と同じ）
    assert_eq!(
        c.request.auth,
        Some(AuthOverride::Bearer {
            token: "tok".into()
        })
    );
    assert!(c.warnings.iter().any(|w| w.kind == WarningKind::Adjusted));
    let c = f
        .state
        .curl_parse(&p.id, "curl -u neko:pw http://localhost:8080/items/1", None)
        .unwrap();
    assert_eq!(
        c.request.auth,
        Some(AuthOverride::Basic {
            username: "neko".into(),
            password: "pw".into()
        })
    );
    // 「その他」は取り込んだ URL そのまま（baseUrl に当てはまらなければ）
    let c = f
        .state
        .curl_parse(
            &p.id,
            "curl -X POST https://example.org:8443/a/b?x=1 -d 'k=v'",
            None,
        )
        .unwrap();
    assert_eq!(c.target, CurlTarget::Adhoc);
    assert_eq!(c.request.url, "https://example.org:8443/a/b");
    assert_eq!(param(&c, ParamIn::Query, "x").unwrap().value, "1");
    assert_eq!(c.suggested_name, "POST /a/b");
    // 解析できないものは Validation
    assert!(f.state.curl_parse(&p.id, "", None).is_err());
    assert!(f.state.curl_parse(&p.id, "curl 'http://x", None).is_err());
    assert!(f
        .state
        .curl_parse("no-such-project", "curl http://x", None)
        .is_err());
}

#[tokio::test]
async fn base_url_is_matched_against_the_given_environment_first_then_the_others() {
    let f = fx();
    let (p, _tmp) = small_project(&f).await;
    let local = f
        .state
        .env_list(&p.id)
        .unwrap()
        .into_iter()
        .find(|e| !e.is_global)
        .unwrap();
    let staging = f.state.env_create(&p.id, "検証").unwrap();
    let mut e = staging.clone();
    e.vars = vec![EnvVar {
        name: "baseUrl".into(),
        value: VarValue::Plain("https://staging.example.com/".into()),
        description: String::new(),
        enabled: true,
    }];
    let staging = f.state.env_save(&e).unwrap();

    // 選んでいる環境（ローカル）には当てはまらないが、検証の baseUrl には当てはまる
    let c = f
        .state
        .curl_parse(&p.id, "curl https://staging.example.com/items/7", None)
        .unwrap();
    assert_eq!(c.target, CurlTarget::Operation);
    assert_eq!(c.environment_id.as_deref(), Some(staging.id.as_str()));
    assert_eq!(c.request.url, "{{baseUrl}}/items/{id}");
    let c = f
        .state
        .curl_parse(
            &p.id,
            "curl http://localhost:8080/items/7",
            Some(&staging.id),
        )
        .unwrap();
    assert_eq!(c.environment_id.as_deref(), Some(local.id.as_str()));

    // baseUrl にパスがある環境（`/v1` を除いて照合する）
    let prefixed = f.state.env_create(&p.id, "プレフィックス").unwrap();
    let mut e = prefixed.clone();
    e.vars = vec![EnvVar {
        name: "baseUrl".into(),
        value: VarValue::Plain("https://gw.example.com/v1".into()),
        description: String::new(),
        enabled: true,
    }];
    f.state.env_save(&e).unwrap();
    let c = f
        .state
        .curl_parse(&p.id, "curl https://gw.example.com/v1/items/search", None)
        .unwrap();
    assert_eq!(c.op_key, Some(key("searchItems")));
    assert_eq!(
        c.request.url,
        "{{baseUrl}}/items/{id}".replace("/items/{id}", "/items/search")
    );
    // `/v1` の外の path は当てはまらない
    let c = f
        .state
        .curl_parse(&p.id, "curl https://gw.example.com/v2/items/search", None)
        .unwrap();
    assert_eq!(c.target, CurlTarget::Adhoc);
    assert!(!c.base_replaced);
}

#[tokio::test]
async fn the_imported_request_can_be_sent_as_is() {
    let f = fx();
    let (p, _tmp) = small_project(&f).await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/items/42"))
        .respond_with(ResponseTemplate::new(200).set_body_string("found"))
        .mount(&server)
        .await;
    let c = f
        .state
        .curl_parse(
            &p.id,
            "curl 'http://localhost:8080/items/42?expand=owner'",
            None,
        )
        .unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    f.state
        .request_send(
            RequestInput {
                project_id: p.id.clone(),
                environment_id: None,
                request: c.request,
                overrides: vec![VarOverride {
                    name: "baseUrl".into(),
                    value: server.uri(),
                }],
                op_key: c.op_key,
            },
            None,
            move |r| {
                let _ = tx.send(r);
            },
        )
        .unwrap();
    let r = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap();
    let SendOutcome::Success { response } = r.result else {
        panic!("{:?}", r.result)
    };
    assert_eq!(response.body.content, "found");
    let seen = server.received_requests().await.unwrap();
    assert_eq!(seen[0].url.query(), Some("expand=owner"));
}
