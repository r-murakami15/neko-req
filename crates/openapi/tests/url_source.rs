//! URL 方式（wiremock）。アクセストークンはヘッダーで送り、入口と別のホストを指す参照は読む前に確認を求める。

mod common;

use common::fixture;
use nekoreq_openapi::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn serve_file(server: &MockServer, url_path: &str, file: &str, token: Option<&str>) {
    let body = std::fs::read_to_string(fixture(file)).unwrap();
    let mut m = Mock::given(method("GET")).and(path(url_path.to_string()));
    if let Some(t) = token {
        m = m.and(header("authorization", format!("Bearer {t}").as_str()));
    }
    m.respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

async fn serve_split(server: &MockServer, token: Option<&str>) {
    for (p, f) in [
        ("/api/openapi.yaml", "split/openapi.yaml"),
        ("/api/paths/users.yaml", "split/paths/users.yaml"),
        ("/api/paths/tree.yaml", "split/paths/tree.yaml"),
        ("/api/common/schemas.yaml", "split/common/schemas.yaml"),
        ("/api/common/responses.yaml", "split/common/responses.yaml"),
    ] {
        serve_file(server, p, f, token).await;
    }
}

fn params(url: String, token: Option<&str>, allowed: Vec<String>) -> LoadParams {
    LoadParams {
        entry: LoadEntry::Url(url),
        token: token.map(String::from),
        allowed_hosts: allowed,
        root: None,
        transport: None,
    }
}

#[tokio::test]
async fn url_entry_reads_relative_refs_and_sends_the_token() {
    let server = MockServer::start().await;
    serve_split(&server, Some("s3cret")).await;
    let r = load(&params(
        format!("{}/api/openapi.yaml", server.uri()),
        Some("s3cret"),
        vec![],
    ))
    .await;
    let LoadResult::Loaded(l) = r else {
        panic!("読めるはず")
    };
    assert_eq!(l.operations.len(), 6);
    assert_eq!(l.files.len(), 5);
    // 表示は入口からの相対 URL
    let o = l
        .operations
        .iter()
        .find(|o| o.op_key.to_string() == "listUsers")
        .unwrap();
    assert_eq!(o.source_file.as_deref(), Some("paths/users.yaml"));
    assert!(
        l.files.iter().all(|f| f.modified_at.is_none()),
        "URL は更新日時を持たない"
    );
    // すべてのリクエストにトークンが付いている
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 5);
    assert!(reqs
        .iter()
        .all(|r| r.headers.get("authorization").is_some()));
}

#[tokio::test]
async fn wrong_or_missing_token_stops_with_a_helpful_message() {
    let server = MockServer::start().await;
    serve_split(&server, Some("right")).await;
    let r = load(&params(
        format!("{}/api/openapi.yaml", server.uri()),
        None,
        vec![],
    ))
    .await;
    let LoadResult::Blocked(p) = r else { panic!() };
    let LoadProblem::Unreadable { reason, .. } = &p[0] else {
        panic!("{p:?}")
    };
    assert!(reason.contains("404"), "{reason}");
    assert!(reason.contains("アクセストークン"), "{reason}");
    // トークンは問題の文に出さない
    let r = load(&params(
        format!("{}/api/openapi.yaml", server.uri()),
        Some("wrong-token-value"),
        vec![],
    ))
    .await;
    let LoadResult::Blocked(p) = r else { panic!() };
    assert!(!p[0].describe().contains("wrong-token-value"));
}

#[tokio::test]
async fn missing_remote_file_is_listed_with_the_referencing_line() {
    let server = MockServer::start().await;
    serve_file(&server, "/api/openapi.yaml", "missing/openapi.yaml", None).await;
    let r = load(&params(
        format!("{}/api/openapi.yaml", server.uri()),
        None,
        vec![],
    ))
    .await;
    let LoadResult::Blocked(p) = r else { panic!() };
    assert_eq!(p.len(), 2, "{p:#?}");
    let LoadProblem::MissingFile {
        file,
        referenced_from,
        ..
    } = &p[0]
    else {
        panic!()
    };
    assert_eq!(file, "paths/gone.yaml");
    assert_eq!(referenced_from.len(), 2);
    assert_eq!(referenced_from[0].file, "openapi.yaml");
    assert!(referenced_from[0].line.is_some());
}

#[tokio::test]
async fn references_to_another_host_wait_for_confirmation_and_get_no_token() {
    let main = MockServer::start().await;
    let other = MockServer::start().await;
    // 入口は別のホストの yaml を参照する
    let entry = format!(
        "openapi: 3.0.3\ninfo: {{title: F, version: '1'}}\npaths:\n  /a:\n    $ref: '{}/shared.yaml#/paths/~1a'\n",
        other.uri()
    );
    Mock::given(method("GET"))
        .and(path("/api/openapi.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(entry))
        .mount(&main)
        .await;
    Mock::given(method("GET"))
        .and(path("/shared.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "paths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200':\n          description: ok\n",
        ))
        .mount(&other)
        .await;

    let url = format!("{}/api/openapi.yaml", main.uri());
    // 1 回目: 確認待ち（別のホストにはリクエストしない）
    let r = load(&params(url.clone(), Some("tok"), vec![])).await;
    let LoadResult::NeedsConfirmation(foreign) = r else {
        panic!("確認待ちのはず")
    };
    assert_eq!(foreign.len(), 1);
    let host = other.uri();
    assert_eq!(foreign[0].host, host);
    assert_eq!(foreign[0].referenced_from.file, "openapi.yaml");
    assert_eq!(foreign[0].referenced_from.line, Some(5));
    assert!(other.received_requests().await.unwrap().is_empty());

    // 確認したホストを渡すと読める。トークンは入口のホストにだけ送る
    let r = load(&params(url, Some("tok"), vec![host])).await;
    let LoadResult::Loaded(l) = r else {
        panic!("確認後は読めるはず")
    };
    assert_eq!(l.operations.len(), 1);
    let reqs = other.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert!(
        reqs[0].headers.get("authorization").is_none(),
        "別のホストにトークンを送らない"
    );
    let reqs = main.received_requests().await.unwrap();
    assert!(reqs
        .iter()
        .all(|r| r.headers.get("authorization").is_some()));
}

#[tokio::test]
async fn a_local_entry_referencing_a_url_also_needs_confirmation() {
    let other = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("openapi.yaml");
    std::fs::write(
        &f,
        format!("openapi: 3.0.3\ninfo: {{title: L, version: '1'}}\npaths:\n  /a:\n    $ref: '{}/x.yaml#/p'\n", other.uri()),
    )
    .unwrap();
    let r = common::load_path(&f).await;
    assert!(matches!(r, LoadResult::NeedsConfirmation(_)));
    assert!(other.received_requests().await.unwrap().is_empty());
}

const ENTRY_HEAD: &str = "openapi: 3.0.3\ninfo: {title: R, version: '1'}\n";
const GET_A: &str = "get:\n  operationId: getA\n  responses:\n    '200': {description: ok}\n";

#[tokio::test]
async fn relative_refs_after_a_redirect_resolve_against_the_final_url() {
    // 決定: 入口が 30x で別ホストへ飛ぶのは許可（GitHub raw など）。相対 $ref は実際に取得した URL が基準。
    // トークンは付けない。リダイレクト先の origin の参照は確認なしで読む
    let main = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/blob/openapi.yaml"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/raw/v1/openapi.yaml", other.uri())),
        )
        .mount(&main)
        .await;
    Mock::given(method("GET"))
        .and(path("/raw/v1/openapi.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "{ENTRY_HEAD}paths:\n  /a:\n    $ref: './parts/a.yaml'\n"
        )))
        .mount(&other)
        .await;
    Mock::given(method("GET"))
        .and(path("/raw/v1/parts/a.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(GET_A))
        .mount(&other)
        .await;
    let r = load(&params(
        format!("{}/blob/openapi.yaml", main.uri()),
        Some("tok"),
        vec![],
    ))
    .await;
    let LoadResult::Loaded(l) = r else {
        panic!("リダイレクト先を基準に読めるはず")
    };
    assert_eq!(l.operations.len(), 1);
    // 元の入口のホストには相対参照を取りに行かない。トークンは入口にだけ
    let m = main.received_requests().await.unwrap();
    assert_eq!(m.len(), 1);
    assert!(m[0].headers.get("authorization").is_some());
    let o = other.received_requests().await.unwrap();
    assert_eq!(o.len(), 2);
    assert!(o.iter().all(|r| r.headers.get("authorization").is_none()));
}

#[tokio::test]
async fn confirmed_hosts_are_scheme_host_and_port_origins() {
    let main = MockServer::start().await;
    let other = MockServer::start().await;
    let other_host = other.uri().trim_start_matches("http://").to_string();
    Mock::given(method("GET"))
        .and(path("/api/openapi.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "{ENTRY_HEAD}paths:\n  /a:\n    $ref: '{}/x.yaml'\n",
            other.uri()
        )))
        .mount(&main)
        .await;
    Mock::given(method("GET"))
        .and(path("/x.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(GET_A))
        .mount(&other)
        .await;
    let url = format!("{}/api/openapi.yaml", main.uri());
    // http の参照は、https で確認した（スキームのない古い表記を含む）ホストでは通らない
    for allowed in [
        format!("https://{other_host}"),
        other_host.clone(),
        // ポートだけ違う
        "http://127.0.0.1:1".to_string(),
    ] {
        let r = load(&params(url.clone(), None, vec![allowed.clone()])).await;
        let LoadResult::NeedsConfirmation(f) = r else {
            panic!("{allowed} では確認が要るはず")
        };
        assert_eq!(f[0].host, other.uri());
    }
    assert!(other.received_requests().await.unwrap().is_empty());
    // 同じ origin なら大文字小文字を区別せず通る
    let r = load(&params(url, None, vec![other.uri().to_uppercase()])).await;
    assert!(matches!(r, LoadResult::Loaded(_)));
}

#[tokio::test]
async fn userinfo_in_the_entry_url_is_refused_with_a_hint_and_no_request() {
    let main = MockServer::start().await;
    let host = main.uri().trim_start_matches("http://").to_string();
    let r = load(&params(format!("http://u:PW@{host}/x.yaml"), None, vec![])).await;
    let LoadResult::Blocked(p) = r else { panic!() };
    let text = p.iter().map(LoadProblem::describe).collect::<String>();
    assert!(text.contains("トークン") && !text.contains("PW"), "{text}");
    assert!(main.received_requests().await.unwrap().is_empty());
}

#[test]
fn debug_of_load_params_hides_the_token() {
    let p = params("https://h.example/x.yaml".into(), Some("TOKEN-XYZ"), vec![]);
    assert!(!format!("{p:?}").contains("TOKEN-XYZ"));
}
