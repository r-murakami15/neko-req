//! 実際の API（ホストの Dev Container）に対して、M5 の完了条件を通しで確かめる。`#[ignore]`。
//!
//! 「パスワードログイン → メール認証コードログイン → 取引先登録（POST）が、セッションやトークンを手でコピーせずに通る」。
//!
//! 実行（API とメールの受け口（Mailpit）がホストで動いているとき。コンテナから `host.docker.internal` で届く）:
//!
//! ```text
//! NEKOREQ_REAL_EMAIL=... NEKOREQ_REAL_PASSWORD=... \
//!   cargo test -p nekoreq --test real_api_m5 -- --ignored --nocapture
//! ```
//!
//! 環境変数:
//! - `NEKOREQ_REAL_EMAIL` / `NEKOREQ_REAL_PASSWORD`（必須）: ログインするユーザー。コードには書かない。
//! - `NEKOREQ_REAL_API_URL`: API の場所。既定は `http://host.docker.internal:8080`。
//! - `NEKOREQ_MAILPIT_URL`: メールの受け口（Mailpit）。既定は `http://host.docker.internal:8025`。
//!   バックエンドの docker-compose.yml では、SES で送ったメールが MiniStack から Mailpit に中継される。
//!   認証コードは Mailpit の API（`/api/v1/search`・`/api/v1/message/<ID>`）から自動で取る。
//! - `NEKOREQ_REAL_EMAIL_CODE`: 認証コードを手で渡すとき（Mailpit が使えない環境）。ログインの 1 段階目のあとに
//!   取れないので、先に 1 回流してコードを調べて渡す（コードは 1 段階目ごとに作り直されるため、
//!   この指定はコードが固定の環境用）。
//! - `NEKOREQ_FIXTURE_DIR`: yaml の置き場所。既定は `/fixtures/license-api`。
//! - `NEKOREQ_REAL_KEEP=1`: 登録した取引先を最後に削除しない。
//!
//! バックエンドは `SPRING_PROFILES_ACTIVE=dev` だと CSRF が無効（XSRF-TOKEN も出ない）で、取引先 API が認証不要になる。
//! CSRF の自動付与まで確かめるときは、dev 以外のプロファイルで起動する（このテストはどちらでも通る。
//! XSRF-TOKEN を受け取ったかどうかを出力する）。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nekoreq_core::{
    AutoUpdateAction, Body, ImportOptions, KeyValue, OpKey, ParamIn, Project, RequestSpec,
    SendOutcome, VarValue,
};
use nekoreq_lib::openapi::{ImportRequest, ImportResult, SourceInput};
use nekoreq_lib::send::{RequestFinished, RequestInput};
use nekoreq_lib::state::AppState;
use nekoreq_secrets::{MemorySecretStore, SecretKey, SecretStore};

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("環境変数 {name} が必要です（認証情報はコードに書かず、環境変数で渡します）")
    })
}

fn optional(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

async fn send(state: &AppState, input: RequestInput) -> RequestFinished {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .request_send(input, None, move |f| {
            let _ = tx.send(f);
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), rx)
        .await
        .expect("finished が届く")
        .unwrap()
}

fn response(f: &RequestFinished) -> &nekoreq_core::ResponseData {
    match &f.result {
        SendOutcome::Success { response } => response,
        SendOutcome::Failure { error } => panic!("送信できるはず: {error:?}"),
    }
}

fn adhoc_get(project: &Project, url: &str, query: Vec<(&str, &str)>) -> RequestInput {
    RequestInput {
        project_id: project.id.clone(),
        environment_id: None,
        request: RequestSpec {
            method: "GET".into(),
            url: url.into(),
            params: query
                .into_iter()
                .map(|(k, v)| KeyValue {
                    location: ParamIn::Query,
                    name: k.into(),
                    value: v.into(),
                    enabled: true,
                })
                .collect(),
            body: Body::None,
            auth: None,
        },
        overrides: vec![],
        op_key: None,
    }
}

// ---- Mailpit から認証コードを取る ----

/// 6 桁の数字（前後が数字でないもの）を探す。
fn find_code(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 6 {
                return Some(text[start..i].to_string());
            }
        } else {
            i += 1;
        }
    }
    None
}

fn json_body(f: &RequestFinished) -> serde_json::Value {
    serde_json::from_str(&response(f).body.content).unwrap_or_default()
}

/// 宛先 `email` のメールの ID の一覧（新しいものが先）。
async fn mailpit_ids(state: &AppState, project: &Project, base: &str, email: &str) -> Vec<String> {
    let f = send(
        state,
        adhoc_get(
            project,
            &format!("{base}/api/v1/search"),
            vec![("query", &format!("to:{email}")), ("limit", "20")],
        ),
    )
    .await;
    assert_eq!(response(&f).status, 200, "Mailpit に届きません: {base}");
    json_body(&f)["messages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m["ID"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// `known` にないメールが届くのを待って、本文から 6 桁のコードを取る。
async fn wait_for_code(
    state: &AppState,
    project: &Project,
    base: &str,
    email: &str,
    known: &HashSet<String>,
) -> String {
    for _ in 0..40 {
        let ids = mailpit_ids(state, project, base, email).await;
        if let Some(id) = ids.iter().find(|id| !known.contains(*id)) {
            let f = send(
                state,
                adhoc_get(project, &format!("{base}/api/v1/message/{id}"), vec![]),
            )
            .await;
            let msg = json_body(&f);
            let text = msg["Text"]
                .as_str()
                .filter(|t| !t.trim().is_empty())
                .or_else(|| msg["HTML"].as_str())
                .unwrap_or_default();
            if let Some(code) = find_code(text) {
                return code;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("認証コードのメールが届きませんでした（Mailpit: {base}）。NEKOREQ_REAL_EMAIL_CODE で手で渡せます");
}

#[test]
fn find_code_reads_a_six_digit_run_only() {
    assert_eq!(
        find_code("様\n\n123456\n\n有効期限 2026年11月11日 10:10（10分間）"),
        Some("123456".into())
    );
    assert_eq!(find_code("1234567 12345 987654"), Some("987654".into()));
    assert_eq!(find_code("12345 1234567"), None);
}

/// Mailpit の API（`/api/v1/search`・`/api/v1/message/<ID>`）に合わせたモックで、コードの取り方を確かめる。
#[tokio::test]
async fn mailpit_helper_waits_for_a_new_message_and_reads_the_code() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    // 最初は古いメールだけ。2 回目の検索から新しいメールが見える
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .and(query_param("query", "to:user@example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"messages":[{"ID":"old1"}]}"#))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"messages":[{"ID":"new2"},{"ID":"old1"}]}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/message/new2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"ID":"new2","Text":"利用者 様\n\nログインの認証コードをお知らせします。\n\n482913\n\n有効期限：2026年11月17日 10:10（10分間）"}"#,
        ))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let state =
        AppState::open_with_secrets(&dir.path().join("data"), Arc::new(MemorySecretStore::new()))
            .unwrap();
    let project = state.project_create("P").unwrap();
    let base = server.uri();
    let known: HashSet<String> = mailpit_ids(&state, &project, &base, "user@example.com")
        .await
        .into_iter()
        .collect();
    assert_eq!(known, HashSet::from(["old1".to_string()]));
    let code = wait_for_code(&state, &project, &base, "user@example.com", &known).await;
    assert_eq!(code, "482913");
}

// ---- 本体 ----

#[tokio::test]
#[ignore = "ホストで API（NEKOREQ_REAL_API_URL）と Mailpit が動いているときだけ。NEKOREQ_REAL_EMAIL / NEKOREQ_REAL_PASSWORD が要る"]
async fn real_api_password_login_email_code_login_and_create_client() {
    let email = required("NEKOREQ_REAL_EMAIL");
    let password = required("NEKOREQ_REAL_PASSWORD");
    let api = optional("NEKOREQ_REAL_API_URL", "http://host.docker.internal:8080");
    let mailpit = optional("NEKOREQ_MAILPIT_URL", "http://host.docker.internal:8025");
    let manual_code = std::env::var("NEKOREQ_REAL_EMAIL_CODE")
        .ok()
        .filter(|c| !c.trim().is_empty());
    let fixture = PathBuf::from(optional("NEKOREQ_FIXTURE_DIR", "/fixtures/license-api"))
        .join("openapi.yaml");

    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::new());
    let state = AppState::open_with_secrets(&dir.path().join("data"), secrets.clone()).unwrap();
    let ImportResult::Imported { project, .. } = state
        .openapi_import(ImportRequest {
            source: SourceInput::Path {
                path: fixture.display().to_string(),
                allowed_hosts: vec![],
            },
            project_id: None,
            project_name: None,
            selected: None,
            options: ImportOptions::default(),
        })
        .await
        .unwrap()
    else {
        panic!("{} を取り込めません", fixture.display())
    };
    // servers から作った環境の baseUrl を、コンテナから届く場所にする
    let mut env = state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|e| !e.is_global)
        .expect("servers から環境ができている");
    for v in &mut env.vars {
        if v.name == "baseUrl" {
            v.value = VarValue::Plain(api.clone());
        }
    }
    let env = state.env_save(&env).unwrap();
    let key = |id: &str| OpKey::Id(id.into());
    let op = |id: &str, body: Option<Body>| {
        let open = state.operation_open(&project.id, &key(id)).unwrap();
        let mut request = open.request;
        if let Some(b) = body {
            request.body = b;
        }
        RequestInput {
            project_id: project.id.clone(),
            environment_id: None,
            request,
            overrides: vec![],
            op_key: Some(key(id)),
        }
    };
    let secret = |name: &str| {
        secrets
            .get(&SecretKey::new(&project.id, &env.id, name))
            .unwrap()
    };

    // 1. ログイン状態の取得（XSRF-TOKEN が発行される環境ではここで保持する）
    let r = send(&state, op("getSession", None)).await;
    assert_eq!(response(&r).status, 200, "{}", response(&r).body.content);
    let got_token = r.auto_updates.iter().any(|u| u.name == "xsrfToken");
    println!(
        "getSession: XSRF-TOKEN {}（dev プロファイルでは出ない）",
        if got_token {
            "を保持した"
        } else {
            "は出なかった"
        }
    );

    // 2. パスワードログイン（1 段階目）。届くメールを見分けるため、先に今ある ID を控える
    let known: HashSet<String> = if manual_code.is_some() {
        HashSet::new()
    } else {
        mailpit_ids(&state, &project, &mailpit, &email)
            .await
            .into_iter()
            .collect()
    };
    let body = serde_json::json!({"email": email, "password": password}).to_string();
    let r = send(&state, op("createPasswordLogin", Some(Body::Json(body)))).await;
    assert_eq!(
        response(&r).status,
        200,
        "ログインの 1 段階目: {}",
        response(&r).body.content
    );
    assert!(
        r.auto_updates
            .iter()
            .any(|u| u.name == "session" && u.action == AutoUpdateAction::Updated),
        "SESSION を保持するはず: {:?}",
        r.auto_updates
    );
    let pending = secret("session").expect("session を保持した");

    // 3. メール認証コードログイン（2 段階目）
    let code = match manual_code {
        Some(c) => c,
        None => wait_for_code(&state, &project, &mailpit, &email, &known).await,
    };
    let body = serde_json::json!({ "code": code }).to_string();
    let r = send(&state, op("createEmailCodeLogin", Some(Body::Json(body)))).await;
    assert_eq!(
        response(&r).status,
        200,
        "ログインの 2 段階目: {}",
        response(&r).body.content
    );
    let session = secret("session").expect("session を保持した");
    assert_ne!(session, pending, "ログインでセッション ID が作り直される");
    println!(
        "email-code-login: 更新した変数 = {:?}",
        r.auto_updates
            .iter()
            .map(|u| u.name.as_str())
            .collect::<Vec<_>>()
    );

    // ログイン状態になっている
    let r = send(&state, op("getSession", None)).await;
    assert!(
        response(&r).body.content.contains("\"authenticated\":true")
            || response(&r)
                .body
                .content
                .contains("\"authenticated\": true"),
        "ログインできていない: {}",
        response(&r).body.content
    );

    // 4. 取引先登録（POST）。セッションもトークンも手でコピーしていない
    let name = format!(
        "NekoReq M5 テスト {}",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let body = serde_json::json!({"clientName": name, "contractType": "DIRECT"}).to_string();
    let input = op("createClient", Some(Body::Json(body)));
    let preview = state.request_preview(&input).unwrap();
    println!(
        "自動で付くヘッダー: {:?}",
        preview
            .auto_headers
            .iter()
            .map(|h| (&h.name, &h.value, &h.source))
            .collect::<Vec<_>>()
    );
    let r = send(&state, input).await;
    let created = response(&r);
    println!("createClient: {} {}", created.status, created.body.content);
    assert_eq!(
        created.status, 200,
        "取引先を登録できるはず: {}",
        created.body.content
    );

    // 後始末（登録した取引先を消す。NEKOREQ_REAL_KEEP=1 なら残す）
    if std::env::var("NEKOREQ_REAL_KEEP").ok().as_deref() != Some("1") {
        let body: serde_json::Value =
            serde_json::from_str(&created.body.content).unwrap_or_default();
        if let Some(id) = body["data"]["clientId"].as_i64() {
            let mut del = op("deleteClient", None);
            del.request.params = vec![KeyValue {
                location: ParamIn::Path,
                name: "clientId".into(),
                value: id.to_string(),
                enabled: true,
            }];
            let r = send(&state, del).await;
            println!("deleteClient: {}", response(&r).status);
        }
    }

    // 値がファイルにも画面向けの結果にも出ていない
    let mut leaked = Vec::new();
    for entry in walk(dir.path()) {
        let bytes = std::fs::read(&entry).unwrap();
        if bytes
            .windows(session.len())
            .any(|w| w == session.as_bytes())
        {
            leaked.push(entry);
        }
    }
    assert!(leaked.is_empty(), "session が平文で残っている: {leaked:?}");
    assert!(!format!("{r:?}").contains(&session));
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
