//! M6 の履歴を `AppState` 経由で通しで確かめる。Tauri は起動しない。
//!
//! - シークレット（環境のシークレット・手で書いた認証・自動付与の SESSION / XSRF-TOKEN・URL のクエリ・ボディ・
//!   エコーされたレスポンス・エラー文）が、履歴にも SQLite のファイル（本体と WAL）にも平文で残らない。
//! - 成功・4xx / 5xx・失敗（接続できない・タイムアウト・中止）が残る。送信していないものは残らない。
//! - 1 MB を超えるボディ、保存しない設定、絞り込み・全文検索、再送信（今の環境）、入力値として保存、cURL、比較。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nekoreq_core::{
    AppSettings, AuthOverride, Body, DiffTag, EnvVar, Environment, HistoryBodyKind, HistoryEntry,
    HistoryQuery, KeyValue, OpKey, ParamIn, Project, RequestSpec, SendOutcome, SendSettings,
    StatusClass, VarValue,
};
use nekoreq_lib::curl::CurlExportMode;
use nekoreq_lib::history::SavedInput;
use nekoreq_lib::send::{RequestFinished, RequestInput, VarOverride};
use nekoreq_lib::state::AppState;
use nekoreq_secrets::MemorySecretStore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// ---- 道具 ----

struct Fx {
    _dir: tempfile::TempDir,
    data: PathBuf,
    state: AppState,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let state = AppState::open_with_secrets(&data, Arc::new(MemorySecretStore::new())).unwrap();
    Fx {
        _dir: dir,
        data,
        state,
    }
}

/// 受け取ったリクエストをそのまま返し、SESSION と XSRF-TOKEN の Set-Cookie も付けるサーバー。
struct Echo;

impl Respond for Echo {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let headers: Vec<(String, String)> = req
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = serde_json::json!({
            "method": req.method.as_str(),
            "url": req.url.to_string(),
            "headers": headers,
            "body": String::from_utf8_lossy(&req.body),
            // 受け取った Set-Cookie の値も本文にエコーする（伏せ字の対象）
            "issued": "NEW-SESS-123 / NEW-XSRF-456",
        });
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .append_header("set-cookie", "SESSION=NEW-SESS-123; Path=/; HttpOnly")
            .append_header("set-cookie", "XSRF-TOKEN=NEW-XSRF-456; Path=/")
            .set_body_string(body.to_string())
    }
}

fn var(name: &str, value: VarValue) -> EnvVar {
    EnvVar {
        name: name.into(),
        value,
        description: String::new(),
        enabled: true,
    }
}

/// プロジェクトと環境（baseUrl とシークレット 4 つ）を作る。
fn project_with_env(f: &Fx, name: &str, base_url: &str) -> (Project, Environment) {
    let project = f.state.project_create(name).unwrap();
    let env = f.state.env_create(&project.id, "ローカル").unwrap();
    let mut e = env.clone();
    e.vars = vec![
        var("baseUrl", VarValue::Plain(base_url.to_string())),
        var("apiKey", VarValue::Secret),
        var("bodySecret", VarValue::Secret),
        var("session", VarValue::Secret),
        var("xsrfToken", VarValue::Secret),
    ];
    let env = f.state.env_save(&e).unwrap();
    for (n, v) in [
        ("apiKey", "API-KEY-SECRET-11"),
        ("bodySecret", "BODY-SECRET-22"),
        ("session", "SESSION-ENV-SECRET-33"),
        ("xsrfToken", "XSRF-ENV-SECRET-44"),
    ] {
        f.state.secret_set(&project.id, &env.id, n, v).unwrap();
    }
    let project = f.state.project_list().unwrap().remove(0);
    (project, env)
}

fn kv(location: ParamIn, name: &str, value: &str) -> KeyValue {
    KeyValue {
        location,
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}

fn adhoc(project: &Project, method: &str, url: &str) -> RequestInput {
    RequestInput {
        project_id: project.id.clone(),
        environment_id: None,
        request: RequestSpec {
            method: method.into(),
            url: url.into(),
            params: vec![],
            body: Body::None,
            auth: None,
        },
        overrides: vec![],
        op_key: None,
    }
}

async fn send_with(
    state: &AppState,
    input: RequestInput,
    settings: Option<SendSettings>,
) -> RequestFinished {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .request_send(input, settings, move |f| {
            let _ = tx.send(f);
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), rx)
        .await
        .expect("finished が届く")
        .unwrap()
}

async fn send(state: &AppState, input: RequestInput) -> RequestFinished {
    send_with(state, input, None).await
}

fn query(text: &str) -> HistoryQuery {
    HistoryQuery {
        text: Some(text.into()),
        ..HistoryQuery::default()
    }
}

fn list(f: &Fx, project: &Project) -> Vec<nekoreq_core::HistoryListItem> {
    f.state
        .history_list(&project.id, &HistoryQuery::default())
        .unwrap()
        .items
}

fn latest(f: &Fx, project: &Project) -> HistoryEntry {
    let id = list(f, project).remove(0).id;
    f.state.history_get(&id).unwrap()
}

/// データフォルダのどのファイル（SQLite・WAL を含む）にも、値がそのまま入っていない。
fn assert_not_on_disk(dir: &Path, values: &[&str]) {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            for e in std::fs::read_dir(p).unwrap() {
                walk(&e.unwrap().path(), out);
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    assert!(!files.is_empty());
    for file in files {
        let bytes = std::fs::read(&file).unwrap();
        for v in values {
            assert!(
                !bytes.windows(v.len()).any(|w| w == v.as_bytes()),
                "{} に {v} が平文で残っている",
                file.display()
            );
        }
    }
}

const SECRETS: [&str; 12] = [
    "API-KEY-SECRET-11",
    "BODY-SECRET-22",
    "SESSION-ENV-SECRET-33",
    "XSRF-ENV-SECRET-44",
    "NEW-SESS-123",
    "NEW-XSRF-456",
    "LITERAL-BEARER-66",
    "HAND-HEADER-77",
    "HAND-COOKIE-88",
    "LITERAL-KEY-99",
    "BASIC-PW-LITERAL-55",
    // Basic の資格情報の base64（neko:BASIC-PW-LITERAL-55）
    "bmVrbzpCQVNJQy1QVy1MSVRFUkFMLTU1",
];

// ---- 伏せ字 ----

#[tokio::test]
async fn secrets_never_reach_history_or_the_sqlite_files() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Echo)
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, "p", &server.uri());

    // A: 環境のシークレット（URL のクエリ・ヘッダー・ボディ）、自動付与の SESSION / XSRF、手で書いた Bearer
    let mut a = adhoc(
        &project,
        "POST",
        "{{baseUrl}}/echo?token={{apiKey}}&plain=abc",
    );
    a.request.params = vec![kv(ParamIn::Header, "X-Key", "{{apiKey}}")];
    a.request.body = Body::Json(r#"{"s":"{{bodySecret}}","name":"ねこ"}"#.into());
    a.request.auth = Some(AuthOverride::Bearer {
        token: "LITERAL-BEARER-66".into(),
    });
    let ra = send(&f.state, a).await;
    assert!(matches!(ra.result, SendOutcome::Success { .. }));

    // B: 手で書いた Authorization ヘッダーと Cookie ヘッダー・API キー（クエリ）
    let mut b = adhoc(&project, "POST", "{{baseUrl}}/echo2");
    b.request.params = vec![
        kv(ParamIn::Header, "Authorization", "Bearer HAND-HEADER-77"),
        kv(
            ParamIn::Header,
            "Cookie",
            "SESSION=HAND-COOKIE-88; theme=dark",
        ),
    ];
    send(&f.state, b).await;
    let mut c = adhoc(&project, "POST", "{{baseUrl}}/echo3");
    c.request.auth = Some(AuthOverride::ApiKey {
        location: nekoreq_core::ApiKeyIn::Query,
        name: "key".into(),
        value: "LITERAL-KEY-99".into(),
    });
    send(&f.state, c).await;
    // D: 認証を選んでいないので、環境の session が Cookie に自動で付く
    send(&f.state, adhoc(&project, "POST", "{{baseUrl}}/echo4")).await;
    // E: 手で書いた Basic のパスワード（Authorization をエコーする応答にも残らない）
    let mut e5 = adhoc(&project, "POST", "{{baseUrl}}/echo5");
    e5.request.auth = Some(AuthOverride::Basic {
        username: "neko".into(),
        password: "BASIC-PW-LITERAL-55".into(),
    });
    send(&f.state, e5).await;

    let items = list(&f, &project);
    assert_eq!(items.len(), 5);
    for item in &items {
        let e = f.state.history_get(&item.id).unwrap();
        let dump = serde_json::to_string(&e).unwrap() + &format!("{e:?}");
        for s in SECRETS {
            assert!(
                !dump.contains(s),
                "履歴 {} に {s} が出ている: {dump}",
                item.url
            );
        }
        // 受け取ったボディにエコーされていた箇所は伏せ字になっている
        assert!(e.response_body.as_deref().unwrap().contains("••••"));
    }
    // 一覧も（URL のクエリ）
    let dump = serde_json::to_string(&items).unwrap();
    for s in SECRETS {
        assert!(!dump.contains(s), "一覧に {s}");
    }
    assert_not_on_disk(&f.data, &SECRETS);

    // A の中身: 展開後の URL は伏せ字、ヘッダーは名前で伏せ字、元の入力は `{{変数}}` のまま
    let ea = f.state.history_get(&items[4].id).unwrap();
    // D の Cookie（SESSION と XSRF-TOKEN）も名前を残して値を伏せる
    let ed = f.state.history_get(&items[1].id).unwrap();
    let cookie_d = ed
        .request_headers
        .iter()
        .find(|h| h.name == "Cookie")
        .unwrap()
        .value
        .clone();
    assert!(
        cookie_d.contains("SESSION=••••") && cookie_d.contains("XSRF-TOKEN=••••"),
        "{cookie_d}"
    );
    assert_eq!(ea.method, "POST");
    assert!(ea.url.ends_with("/echo?token=••••&plain=abc"), "{}", ea.url);
    let h = |name: &str| {
        ea.request_headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.clone())
    };
    assert_eq!(h("X-Key").as_deref(), Some("••••"));
    assert_eq!(h("Authorization").as_deref(), Some("Bearer ••••"));
    assert!(
        h("Cookie").unwrap().contains("XSRF-TOKEN=••••"),
        "{:?}",
        h("Cookie")
    );
    assert_eq!(h("X-XSRF-TOKEN").as_deref(), Some("••••"));
    assert!(h("User-Agent").is_some(), "アプリが付けたヘッダーも残る");
    assert_eq!(ea.request_body_kind, HistoryBodyKind::Text);
    assert_eq!(
        ea.request_body.as_deref(),
        Some(r#"{"s":"••••","name":"ねこ"}"#)
    );
    let input = ea.input.as_ref().unwrap();
    assert_eq!(input.url, "{{baseUrl}}/echo?token={{apiKey}}&plain=abc");
    assert_eq!(
        input.auth,
        Some(AuthOverride::Bearer {
            token: "••••".into()
        }),
        "手で書いた認証の値は履歴でも伏せる"
    );
    assert_eq!(ea.masked_fields, vec!["認証（Bearer のトークン）"]);
    assert_eq!(
        input.body,
        Body::Json(r#"{"s":"{{bodySecret}}","name":"ねこ"}"#.into())
    );
    // 受け取った Set-Cookie
    let set_cookie: Vec<_> = ea
        .response_headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
        .map(|h| h.value.as_str())
        .collect();
    assert_eq!(
        set_cookie,
        vec!["SESSION=••••; Path=/; HttpOnly", "XSRF-TOKEN=••••; Path=/"]
    );
    assert_eq!(ea.status, Some(200));
    assert!(ea.timing.is_some());
    assert_eq!(ea.environment_name.as_deref(), Some("ローカル"));
    assert!(ea.sent_at.ends_with('Z'));
}

#[tokio::test]
async fn failures_are_recorded_without_leaking_secrets() {
    let f = fx();
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&slow)
        .await;
    let (project, _env) = project_with_env(&f, "p", &slow.uri());

    // 接続できない（クエリにシークレット）
    let mut input = adhoc(&project, "GET", "http://127.0.0.1:1/x?token={{apiKey}}");
    input.request.params = vec![kv(ParamIn::Header, "X-Key", "{{apiKey}}")];
    let r = send(&f.state, input).await;
    assert!(matches!(r.result, SendOutcome::Failure { .. }));
    let e = latest(&f, &project);
    assert_eq!(e.status, None);
    assert_eq!(e.error_kind.as_deref(), Some("connect"));
    assert!(
        e.error.as_deref().unwrap().contains("接続できません"),
        "{:?}",
        e.error
    );
    assert!(!format!("{e:?}").contains("API-KEY-SECRET-11"));
    assert_eq!(e.url, "http://127.0.0.1:1/x?token=••••");
    assert!(e.timing.is_none());

    // タイムアウト
    let r = send_with(
        &f.state,
        adhoc(&project, "GET", "{{baseUrl}}/slow"),
        Some(SendSettings {
            timeout_ms: 300,
            ..SendSettings::default()
        }),
    )
    .await;
    assert!(matches!(r.result, SendOutcome::Failure { .. }));
    assert_eq!(latest(&f, &project).error_kind.as_deref(), Some("timeout"));

    // 中止
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = f
        .state
        .request_send(adhoc(&project, "GET", "{{baseUrl}}/slow"), None, move |r| {
            let _ = tx.send(r);
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(f.state.request_cancel(&id));
    rx.await.unwrap();
    let e = latest(&f, &project);
    assert_eq!(e.error_kind.as_deref(), Some("cancelled"));
    assert_eq!(e.status, None);

    let page = f
        .state
        .history_list(
            &project.id,
            &HistoryQuery {
                statuses: vec![StatusClass::Failed],
                ..HistoryQuery::default()
            },
        )
        .unwrap();
    assert_eq!(page.total, 3);
    assert!(page.items.iter().all(|i| i.failed));
    assert_not_on_disk(&f.data, &["API-KEY-SECRET-11"]);
}

#[tokio::test]
async fn requests_that_were_not_sent_are_not_recorded() {
    let f = fx();
    let (project, _env) = project_with_env(&f, "p", "http://127.0.0.1:1");
    // 展開できない変数
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/{{nothing}}")).await;
    assert!(matches!(r.result, SendOutcome::Failure { .. }));
    // ボディのファイルを読めない
    let mut input = adhoc(&project, "POST", "{{baseUrl}}/x");
    input.request.body = Body::Binary("/nonexistent/file.bin".into());
    let r = send(&f.state, input).await;
    assert!(matches!(r.result, SendOutcome::Failure { .. }));
    assert!(list(&f, &project).is_empty());
}

// ---- 設定: 保存しない・1 MB ----

#[tokio::test]
async fn history_can_be_turned_off_and_large_bodies_are_cut_by_setting() {
    let f = fx();
    let server = MockServer::start().await;
    // 1.5 MB の本文。先頭に目印、末尾に別の目印
    let big = format!("HEAD-MARK{}TAIL-MARK", "あ".repeat(500_000));
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/up"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, "p", &server.uri());

    // 既定: 1 MB を超えるボディは先頭だけ
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/big")).await;
    let e = latest(&f, &project);
    let body = e.response_body.unwrap();
    assert_eq!(
        body.len(),
        1024 * 1024 - (1024 * 1024 % 3),
        "文字の途中で切らない"
    );
    assert!(body.starts_with("HEAD-MARK") && !body.contains("TAIL-MARK"));
    assert!(e.response_body_truncated);
    assert_eq!(e.response_size as usize, big.len(), "全体の大きさは残る");

    // 大きなリクエストのボディも同じ
    let mut up = adhoc(&project, "POST", "{{baseUrl}}/up");
    up.request.body = Body::Text(big.clone());
    send(&f.state, up).await;
    let e = latest(&f, &project);
    assert!(e.request_body_truncated);
    assert!(e.request_body.unwrap().len() <= 1024 * 1024);
    assert_eq!(e.request_body_size as usize, big.len());
    // 元の入力のボディも 1 MB を超えるなら残さず、使えなかったものとして知らせる
    assert_eq!(e.input.as_ref().unwrap().body, Body::None);
    assert!(
        e.masked_fields.iter().any(|f| f.starts_with("ボディ")),
        "{:?}",
        e.masked_fields
    );
    let open = f.state.history_open(&e.id).unwrap();
    assert!(
        open.dropped_fields.iter().any(|f| f.starts_with("ボディ")),
        "{:?}",
        open.dropped_fields
    );

    // 「省略しない」: 全体を保存する
    let mut s = f.state.app_settings_get().unwrap();
    s.history.omit_large_bodies = false;
    f.state.app_settings_set(&s).unwrap();
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/big")).await;
    let e = latest(&f, &project);
    assert_eq!(e.response_body.as_deref(), Some(big.as_str()));
    assert!(!e.response_body_truncated);

    // 「保存しない」
    let before = list(&f, &project).len();
    s.history.limit = 0;
    f.state.app_settings_set(&s).unwrap();
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/up")).await;
    assert_eq!(
        list(&f, &project).len(),
        before,
        "保存しない設定では増えない"
    );
    // 設定を変えても、すでにある履歴は消さない
    assert!(before > 0);
}

#[tokio::test]
async fn binary_response_bodies_are_not_stored() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(vec![0x89, b'P', b'N', b'G', 0, 1, 2, 3]),
        )
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, "p", &server.uri());
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/img")).await;
    let e = latest(&f, &project);
    assert!(e.response_binary);
    assert_eq!(e.response_body, None);
    assert_eq!(e.response_size, 8);
    assert_eq!(e.response_content_type.as_deref(), Some("image/png"));
}

// ---- 一覧・絞り込み・全文検索 ----

#[tokio::test]
async fn list_filters_and_japanese_full_text_search() {
    let f = fx();
    let server = MockServer::start().await;
    for (p, status, body) in [
        ("/ok", 200, r#"{"name":"株式会社ネコネット"}"#),
        (
            "/nf",
            404,
            r#"{"detail":"見つかりません","code":"NOT_FOUND"}"#,
        ),
        ("/ng", 500, r#"{"code":"INTERNAL"}"#),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("content-type", "application/json")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
    }
    let (project, env_a) = project_with_env(&f, "p", &server.uri());
    // もう 1 つの環境
    let env_b = f.state.env_create(&project.id, "検証").unwrap();
    let mut e = env_b.clone();
    e.vars = vec![var("baseUrl", VarValue::Plain(server.uri()))];
    let env_b = f.state.env_save(&e).unwrap();

    for p in ["/ok", "/nf", "/ng"] {
        send(
            &f.state,
            adhoc(&project, "GET", &format!("{{{{baseUrl}}}}{p}")),
        )
        .await;
    }
    let mut inb = adhoc(&project, "GET", "{{baseUrl}}/ok");
    inb.environment_id = Some(env_b.id.clone());
    send(&f.state, inb).await;
    let _ = send(&f.state, adhoc(&project, "GET", "http://127.0.0.1:1/dead")).await;

    let all = f
        .state
        .history_list(&project.id, &HistoryQuery::default())
        .unwrap();
    assert_eq!(all.total, 5);
    // 新しい順
    assert_eq!(all.items[0].url, "http://127.0.0.1:1/dead");
    // ステータス
    let by = |s: Vec<StatusClass>| {
        f.state
            .history_list(
                &project.id,
                &HistoryQuery {
                    statuses: s,
                    ..HistoryQuery::default()
                },
            )
            .unwrap()
            .total
    };
    assert_eq!(by(vec![StatusClass::S2xx]), 2);
    assert_eq!(by(vec![StatusClass::S4xx]), 1);
    assert_eq!(by(vec![StatusClass::S5xx, StatusClass::Failed]), 2);
    // 環境
    let env_of = |id: &str| {
        f.state
            .history_list(
                &project.id,
                &HistoryQuery {
                    environment_id: Some(id.into()),
                    ..HistoryQuery::default()
                },
            )
            .unwrap()
            .total
    };
    assert_eq!(env_of(&env_b.id), 1);
    assert_eq!(
        env_of(&env_a.id),
        4,
        "接続できなかったものも、そのときの環境が残る"
    );
    // 全文検索（日本語・URL・3 文字未満）
    let found = |q: &str| f.state.history_list(&project.id, &query(q)).unwrap().total;
    assert_eq!(found("ネコネット"), 2);
    assert_eq!(found("見つかりません"), 1);
    assert_eq!(found("NOT_FOUND"), 1);
    assert_eq!(found("/dead"), 1);
    assert_eq!(found("株式"), 2);
    assert_eq!(found("存在しない"), 0);
    // 期間: 今日（UTC+9 の今日）を含む / 含まない
    let jst = 540;
    let now = chrono::Utc::now() + chrono::Duration::minutes(i64::from(jst));
    let today = now.format("%Y-%m-%d").to_string();
    let tomorrow = (now + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let period = |from: Option<&str>, to: Option<&str>| {
        f.state
            .history_list(
                &project.id,
                &HistoryQuery {
                    from: from.map(Into::into),
                    to: to.map(Into::into),
                    utc_offset_minutes: Some(jst),
                    ..HistoryQuery::default()
                },
            )
            .unwrap()
    };
    let page = period(Some(&today), Some(&today));
    assert_eq!(page.total, 5);
    assert!(
        page.items.iter().all(|i| i.local_date == today),
        "{:?}",
        page.items[0].local_date
    );
    assert_eq!(period(Some(&tomorrow), None).total, 0);
    assert_eq!(period(None, Some("2000-01-01")).total, 0);
    assert!(f
        .state
        .history_list(
            &project.id,
            &HistoryQuery {
                from: Some("2026/11/17".into()),
                ..HistoryQuery::default()
            }
        )
        .is_err());
    // ページング
    let p1 = f
        .state
        .history_list(
            &project.id,
            &HistoryQuery {
                limit: Some(2),
                ..HistoryQuery::default()
            },
        )
        .unwrap();
    assert_eq!((p1.items.len(), p1.total), (2, 5));
    let p2 = f
        .state
        .history_list(
            &project.id,
            &HistoryQuery {
                limit: Some(2),
                cursor: p1.next_cursor.clone(),
                ..HistoryQuery::default()
            },
        )
        .unwrap();
    assert_eq!(p2.items.len(), 2);
    assert_ne!(p1.items[0].id, p2.items[0].id);
    // 別のプロジェクトの履歴は混ざらない
    let other = f.state.project_create("別").unwrap();
    assert_eq!(
        f.state
            .history_list(&other.id, &HistoryQuery::default())
            .unwrap()
            .total,
        0
    );
}

// ---- 削除 ----

#[tokio::test]
async fn delete_clear_and_project_delete_remove_everything() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("消える目印QQ-4242-ZZ"))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, "p", &server.uri());
    for _ in 0..3 {
        send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/a")).await;
    }
    let items = list(&f, &project);
    f.state.history_delete(&items[0].id).unwrap();
    assert!(f.state.history_get(&items[0].id).is_err());
    assert!(f.state.history_delete(&items[0].id).is_err());
    assert_eq!(
        f.state
            .history_list(&project.id, &query("消える目印"))
            .unwrap()
            .total,
        2
    );

    assert_eq!(f.state.history_clear(&project.id).unwrap(), 2);
    assert_eq!(
        f.state
            .history_list(&project.id, &query("消える目印"))
            .unwrap()
            .total,
        0
    );
    assert_not_on_disk(&f.data, &["消える目印QQ-4242-ZZ"]);

    // プロジェクトを消しても
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/a")).await;
    f.state.project_delete(&project.id).unwrap();
    assert_not_on_disk(&f.data, &["消える目印QQ-4242-ZZ"]);
}

// ---- 再送信・編集して開く・入力値として保存 ----

#[tokio::test]
async fn resend_uses_the_current_environment_and_drops_masked_inputs() {
    let f = fx();
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    for s in [&a, &b] {
        Mock::given(method("POST"))
            .respond_with(Echo)
            .mount(s)
            .await;
    }
    let (project, env_a) = project_with_env(&f, "p", &a.uri());
    let env_b = f.state.env_create(&project.id, "検証").unwrap();
    let mut e = env_b.clone();
    e.vars = vec![var("baseUrl", VarValue::Plain(b.uri()))];
    let env_b = f.state.env_save(&e).unwrap();
    assert_eq!(
        f.state.project_list().unwrap()[0]
            .default_environment_id
            .as_deref(),
        Some(env_a.id.as_str())
    );

    let mut input = adhoc(&project, "POST", "{{baseUrl}}/echo");
    input.request.body = Body::Json(r#"{"k":"v"}"#.into());
    input.request.auth = Some(AuthOverride::Bearer {
        token: "LITERAL-BEARER-66".into(),
    });
    input.request.params = vec![kv(ParamIn::Header, "X-Trace", "abc")];
    // 上書きの値は保存されない
    input.overrides = vec![VarOverride {
        name: "unused".into(),
        value: "OVERRIDE-VALUE-5".into(),
    }];
    send(&f.state, input).await;
    assert_eq!(a.received_requests().await.unwrap().len(), 1);
    let first = latest(&f, &project);
    assert_eq!(first.override_names, vec!["unused"]);
    assert!(!serde_json::to_string(&first)
        .unwrap()
        .contains("OVERRIDE-VALUE-5"));

    // 編集して開く: 元の入力（`{{変数}}` のまま）。伏せた認証は外す
    let open = f.state.history_open(&first.id).unwrap();
    assert_eq!(open.request.url, "{{baseUrl}}/echo");
    assert_eq!(open.request.body, Body::Json(r#"{"k":"v"}"#.into()));
    assert_eq!(open.request.auth, None);
    assert_eq!(open.dropped_fields, vec!["認証（Bearer のトークン）"]);
    assert_eq!(open.op_key, None);
    assert_eq!(open.override_names, vec!["unused"]);
    assert_eq!(open.environment_name.as_deref(), Some("ローカル"));

    // 今の環境を「検証」に切り替えて再送信
    f.state.env_select(&project.id, Some(&env_b.id)).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    f.state
        .history_resend(&first.id, None, move |r| {
            let _ = tx.send(r);
        })
        .unwrap();
    let finished = rx.await.unwrap();
    assert!(matches!(finished.result, SendOutcome::Success { .. }));
    let seen = b.received_requests().await.unwrap();
    assert_eq!(seen.len(), 1, "今の環境のサーバーに届く");
    assert_eq!(a.received_requests().await.unwrap().len(), 1);
    assert_eq!(seen[0].url.path(), "/echo");
    assert_eq!(seen[0].body, br#"{"k":"v"}"#);
    assert_eq!(
        seen[0].headers.get("x-trace").unwrap().to_str().unwrap(),
        "abc"
    );
    // 伏せ字の認証（`Bearer ••••`）は送られない
    assert!(seen[0].headers.get("authorization").is_none());
    // 新しい履歴ができて、環境は検証
    let items = list(&f, &project);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].environment_name.as_deref(), Some("検証"));
    assert_eq!(items[1].environment_name.as_deref(), Some("ローカル"));
}

#[tokio::test]
async fn save_as_input_makes_a_draft_for_apis_and_an_adhoc_request_otherwise() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(Echo)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(Echo)
        .mount(&server)
        .await;

    // yaml の API（見本の分割 yaml を取り込む）
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/openapi/tests/fixtures/split"),
        tmp.path(),
    );
    let result = f
        .state
        .openapi_import(nekoreq_lib::openapi::ImportRequest {
            source: nekoreq_lib::openapi::SourceInput::Path {
                path: tmp.path().join("openapi.yaml").display().to_string(),
                allowed_hosts: vec![],
            },
            project_id: None,
            project_name: None,
            selected: None,
            options: nekoreq_core::ImportOptions::default(),
        })
        .await
        .unwrap();
    let nekoreq_lib::openapi::ImportResult::Imported { project, .. } = result else {
        panic!()
    };
    let env = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|e| !e.is_global)
        .unwrap();
    let mut e = env.clone();
    for v in &mut e.vars {
        if v.name == "baseUrl" {
            v.value = VarValue::Plain(server.uri());
        }
    }
    f.state.env_save(&e).unwrap();

    let key = OpKey::Id("listUsers".into());
    let open = f.state.operation_open(&project.id, &key).unwrap();
    let mut req = open.request;
    req.params.push(kv(ParamIn::Query, "q", "ねこ"));
    req.auth = Some(AuthOverride::Bearer {
        token: "LITERAL-BEARER-66".into(),
    });
    send(
        &f.state,
        RequestInput {
            project_id: project.id.clone(),
            environment_id: None,
            request: req,
            overrides: vec![],
            op_key: Some(key.clone()),
        },
    )
    .await;
    let entry = latest(&f, &project);
    assert_eq!(entry.op_key, Some(key.clone()));
    assert_eq!(
        entry.op_summary.as_deref(),
        Some(
            f.state
                .operation_open(&project.id, &key)
                .unwrap()
                .summary
                .as_deref()
                .unwrap()
        )
    );
    let SavedInput::Draft { draft, dropped } =
        f.state.history_save_as_input(&entry.id, None).unwrap()
    else {
        panic!("yaml の API は入力値になるはず")
    };
    assert_eq!(draft.op_key, key);
    assert!(draft
        .params
        .iter()
        .any(|p| p.name == "q" && p.value == "ねこ"));
    assert_eq!(draft.auth, None, "伏せ字の認証は保存しない");
    assert_eq!(dropped, vec!["認証（Bearer のトークン）"]);
    let stored = f.state.draft_get(&project.id, &key).unwrap().unwrap();
    assert_eq!(stored.params, draft.params);

    // 手で作ったもの
    let mut input = adhoc(&project, "POST", "{{baseUrl}}/echo?x=1");
    input.request.body = Body::Json(r#"{"a":1}"#.into());
    send(&f.state, input).await;
    let entry = latest(&f, &project);
    let SavedInput::Adhoc { request, dropped } = f
        .state
        .history_save_as_input(&entry.id, Some("  メモ  "))
        .unwrap()
    else {
        panic!("手で作ったものは AdHocRequest になるはず")
    };
    assert_eq!(request.name, "メモ");
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "{{baseUrl}}/echo?x=1");
    assert_eq!(request.body, Body::Json(r#"{"a":1}"#.into()));
    assert!(dropped.is_empty());
    assert_eq!(f.state.adhoc_list(&project.id).unwrap().len(), 1);
    // 名前がなければ METHOD /path
    let SavedInput::Adhoc { request, .. } = f.state.history_save_as_input(&entry.id, None).unwrap()
    else {
        panic!()
    };
    assert_eq!(request.name, "POST /echo");
}

fn copy_dir(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dest.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}

// ---- cURL としてコピー ----

#[tokio::test]
async fn curl_copy_from_history_and_from_the_current_input() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(Echo)
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, "p", &server.uri());
    let mut input = adhoc(
        &project,
        "POST",
        "{{baseUrl}}/api/v1/auth/password-login?token={{apiKey}}",
    );
    input.request.params = vec![kv(ParamIn::Header, "X-Trace", "t-1")];
    input.request.body = Body::Json(r#"{"email":"a@example.com","k":"{{bodySecret}}"}"#.into());
    input.request.auth = Some(AuthOverride::Bearer {
        token: "LITERAL-BEARER-66".into(),
    });
    send(&f.state, input.clone()).await;
    let entry = latest(&f, &project);

    // 履歴から
    let curl = f.state.history_curl(&entry.id).unwrap();
    assert!(curl.starts_with("curl -X POST "), "{curl}");
    assert!(
        curl.contains("/api/v1/auth/password-login?token=••••"),
        "{curl}"
    );
    assert!(
        curl.contains("-H 'Content-Type: application/json'"),
        "{curl}"
    );
    assert!(curl.contains("-H 'X-Trace: t-1'"));
    assert!(curl.contains("Authorization: Bearer ••••"), "{curl}");
    assert!(
        curl.contains(r#"--data-raw '{"email":"a@example.com","k":"••••"}'"#),
        "{curl}"
    );
    assert!(
        !curl.contains("User-Agent"),
        "アプリが付けた既定のヘッダーは入れない"
    );
    for s in SECRETS {
        assert!(!curl.contains(s), "{s}");
    }
    assert!(!curl.contains("%E2%80%A2"));

    // 今の入力から: 既定は伏せる
    let redacted = f
        .state
        .curl_export(&input, CurlExportMode::Redacted)
        .unwrap();
    for s in SECRETS {
        assert!(!redacted.contains(s), "伏せるはずが {s} がある: {redacted}");
    }
    assert!(redacted.contains("••••"));
    assert!(
        redacted.contains("Authorization: Bearer ••••"),
        "{redacted}"
    );
    assert!(redacted.contains("X-XSRF-TOKEN: ••••"), "{redacted}");
    assert!(redacted.contains(&format!("{}/api/v1/auth/password-login", server.uri())));
    // そのまま
    let raw = f.state.curl_export(&input, CurlExportMode::Raw).unwrap();
    for s in [
        "API-KEY-SECRET-11",
        "BODY-SECRET-22",
        "LITERAL-BEARER-66",
        "NEW-XSRF-456",
    ] {
        assert!(raw.contains(s), "そのままなら {s} が入る: {raw}");
    }
    // 展開できない変数があれば、どれか分かるエラー
    let mut bad = input.clone();
    bad.request.url = "{{baseUrl}}/{{missingVar}}".into();
    let e = f
        .state
        .curl_export(&bad, CurlExportMode::Redacted)
        .unwrap_err();
    assert!(e.to_string().contains("missingVar"), "{e}");
}

// ---- 比較 ----

#[tokio::test]
async fn compare_shows_status_header_and_json_body_differences() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .insert_header("x-version", "1")
                .set_body_string(r#"{"name":"ねこ","count":1,"tags":["a","b"],"same":true}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(
            ResponseTemplate::new(404)
                .insert_header("content-type", "application/json")
                .insert_header("x-version", "2")
                .set_body_string(r#"{"name":"ねこ","count":2,"tags":["a","b"],"same":true}"#),
        )
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, "p", &server.uri());
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/a")).await;
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/b")).await;
    let items = list(&f, &project);
    let (right, left) = (&items[0].id, &items[1].id);
    let d = f.state.history_compare(left, right).unwrap();
    assert_eq!(d.left_status, "200 OK");
    assert_eq!(d.right_status, "404 Not Found");
    assert!(d.status_changed);
    assert!(d.headers_changed);
    assert!(d
        .headers
        .iter()
        .any(|l| l.tag == DiffTag::Delete && l.text == "x-version: 1"));
    assert!(d
        .headers
        .iter()
        .any(|l| l.tag == DiffTag::Insert && l.text == "x-version: 2"));
    // JSON は 1 行 1 項目に整形されるので、変わった項目だけが差になる
    assert!(d.body_changed);
    let del: Vec<_> = d.body.iter().filter(|l| l.tag == DiffTag::Delete).collect();
    let ins: Vec<_> = d.body.iter().filter(|l| l.tag == DiffTag::Insert).collect();
    assert_eq!(del.len(), 1, "{:?}", d.body);
    assert_eq!(ins.len(), 1);
    assert!(del[0].text.contains("\"count\": 1"), "{}", del[0].text);
    assert!(ins[0].text.contains("\"count\": 2"));
    assert!(d.body_note.as_deref().unwrap().contains("JSON"));
    assert_eq!(d.left.id, *left);
    assert_eq!(d.right.status, Some(404));
    // 同じ内容どうしは差がない
    let same = f.state.history_compare(left, left).unwrap();
    assert!(!same.status_changed && !same.headers_changed && !same.body_changed);
    assert!(same.body.is_empty());
    // 存在しない ID
    assert!(f.state.history_compare(left, "none").is_err());
}

#[tokio::test]
async fn default_settings_are_the_documented_ones() {
    let f = fx();
    let s = f.state.app_settings_get().unwrap();
    assert_eq!(s, AppSettings::default());
    assert_eq!(s.history.limit, 1000);
    assert!(s.history.omit_large_bodies);
}
