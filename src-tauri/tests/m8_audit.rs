//! M5〜M7 のテスト担当による独立した検証（`AppState` 経由。Tauri は起動しない）。
//!
//! 実装者のテスト（m5_*・m6_*）と重ならない観点だけを書く。見つけた不具合の再現は、直したあとは通常のテストとして残っている。
//! 残る `#[ignore]` は、公開前に値を置き換えるまで失敗する 1 件だけ（`-- --ignored` で確認できる）。
//!
//! 1. 履歴の伏せ字（さまざまな経路）と SQLite のファイル
//! 2. 履歴の一覧（期間・タイムゾーン・件数の上限・すべて削除）
//! 3. cookieAuth・CSRF（Set-Cookie の形いろいろ・競合・リダイレクト）
//! 4. cURL の取り込み・書き出し
//! 5. 設定と送信（検証・壊れた証明書・プロキシのパスワード・表示上限）
//! 6. タブの復元
//! 7. 自動更新の設定
//! 8. マイグレーション（v1 から順に上げた DB で全機能が動く）

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use nekoreq_core::{
    AppSettings, AuthOverride, AutoUpdateAction, Body, ClientCertSetting, EnvVar, Environment,
    HeaderPair, HistoryBodyKind, HistoryEntry, HistoryQuery, ImportOptions, KeyValue,
    MultipartPart, MultipartValue, OpKey, ParamIn, Project, ProjectSettings, ProxyMode,
    RequestSpec, SendError, SendOutcome, SendSettings, SessionState, SessionTab, SessionTabKind,
    StatusClass, VarValue,
};
use nekoreq_lib::curl::{CurlExportMode, CurlTarget};
use nekoreq_lib::error::AppError;
use nekoreq_lib::openapi::{ImportRequest, ImportResult, SourceInput};
use nekoreq_lib::send::{RequestFinished, RequestInput};
use nekoreq_lib::state::AppState;
use nekoreq_secrets::{MemorySecretStore, SecretKey, SecretStore};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ---- 道具 ----

struct Fx {
    dir: tempfile::TempDir,
    data: PathBuf,
    state: AppState,
    secrets: Arc<MemorySecretStore>,
}

fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let secrets = Arc::new(MemorySecretStore::new());
    let state = AppState::open_with_secrets(&data, secrets.clone()).unwrap();
    Fx {
        dir,
        data,
        state,
        secrets,
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

/// プロジェクトと環境（選んでいる環境）。`baseUrl` と、シークレットの変数（名前と値）を持つ。
fn project_with_env(f: &Fx, base_url: &str, secrets: &[(&str, &str)]) -> (Project, Environment) {
    let project = f.state.project_create("p").unwrap();
    let mut env = f.state.env_create(&project.id, "ローカル").unwrap();
    env.vars = vec![var("baseUrl", VarValue::Plain(base_url.to_string()))];
    for (name, _) in secrets {
        env.vars.push(var(name, VarValue::Secret));
    }
    let env = f.state.env_save(&env).unwrap();
    for (name, value) in secrets {
        f.state
            .secret_set(&project.id, &env.id, name, value)
            .unwrap();
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
    tokio::time::timeout(Duration::from_secs(30), rx)
        .await
        .expect("finished が届く")
        .unwrap()
}

async fn send(state: &AppState, input: RequestInput) -> RequestFinished {
    send_with(state, input, None).await
}

fn response_of(r: &RequestFinished) -> &nekoreq_core::ResponseData {
    match &r.result {
        SendOutcome::Success { response } => response,
        SendOutcome::Failure { error } => panic!("送信できるはず: {error:?}"),
    }
}

fn error_of(r: &RequestFinished) -> &SendError {
    match &r.result {
        SendOutcome::Failure { error } => error,
        SendOutcome::Success { response } => panic!("失敗するはず: {}", response.status),
    }
}

fn all_history(f: &Fx, project: &Project) -> Vec<HistoryEntry> {
    let page = f
        .state
        .history_list(
            &project.id,
            &HistoryQuery {
                limit: Some(500),
                ..HistoryQuery::default()
            },
        )
        .unwrap();
    page.items
        .iter()
        .map(|i| f.state.history_get(&i.id).unwrap())
        .collect()
}

fn latest(f: &Fx, project: &Project) -> HistoryEntry {
    all_history(f, project).remove(0)
}

/// 履歴 1 件の中身（JSON と Debug）。
fn dump(e: &HistoryEntry) -> String {
    serde_json::to_string(e).unwrap() + &format!("{e:?}")
}

fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        for e in std::fs::read_dir(p).unwrap() {
            walk(&e.unwrap().path(), out);
        }
    } else {
        out.push(p.to_path_buf());
    }
}

/// データフォルダのどのファイル（SQLite・WAL を含む）にも、値がそのまま入っていない。
fn assert_not_on_disk(dir: &Path, values: &[&str]) {
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

/// 履歴（一覧・全項目）とディスクに、値が残っていない。
fn assert_clean(f: &Fx, project: &Project, values: &[&str]) {
    for e in all_history(f, project) {
        let d = dump(&e);
        for v in values {
            assert!(
                !d.contains(v),
                "履歴 {} {} に {v} が出ている:\n{d}",
                e.method,
                e.url
            );
        }
    }
    assert_not_on_disk(&f.data, values);
}

/// リクエスト（ヘッダーとボディ）をそのまま JSON にして返すサーバーの応答。
fn echo_json(req: &Request) -> serde_json::Value {
    let headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    serde_json::json!({
        "url": req.url.to_string(),
        "headers": headers,
        "body": String::from_utf8_lossy(&req.body),
    })
}

fn raw_connection(data: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(data.join("data.sqlite")).unwrap()
}

// =====================================================================================
// 1. 履歴の伏せ字
// =====================================================================================

#[tokio::test]
async fn multipart_text_parts_names_and_file_paths_do_not_leak() {
    const S: &str = "MP-SECRET-9f3a71";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(echo_json(req).to_string())
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("mp", S)]);

    // 名前にシークレットを含むファイル
    let files = f.dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join(format!("{S}.txt")), "file-body").unwrap();

    let mut input = adhoc(&project, "POST", "{{baseUrl}}/upload");
    input.request.body = Body::Multipart(vec![
        MultipartPart {
            name: "plain".into(),
            value: MultipartValue::Text("v={{mp}}".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            // パートの名前にシークレット
            name: "n-{{mp}}".into(),
            value: MultipartValue::Text("x".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "up".into(),
            value: MultipartValue::File(format!("{}/{{{{mp}}}}.txt", files.display())),
            enabled: true,
            content_type: Some("text/plain".into()),
        },
    ]);
    let r = send(&f.state, input).await;
    assert_eq!(response_of(&r).status, 200);

    let e = latest(&f, &project);
    assert_eq!(e.request_body_kind, HistoryBodyKind::Multipart);
    assert_eq!(e.request_parts.len(), 3);
    assert!(
        e.request_parts[0].value.contains("••••"),
        "{:?}",
        e.request_parts
    );
    assert!(
        e.request_parts[1].name.contains("••••"),
        "{:?}",
        e.request_parts
    );
    assert!(
        e.request_parts[2].value.contains("••••"),
        "{:?}",
        e.request_parts
    );
    // 本文にエコーされた multipart（filename にもシークレット）も伏せる
    assert!(e.response_body.as_deref().unwrap().contains("••••"));
    assert_clean(&f, &project, &[S]);
}

#[tokio::test]
async fn form_body_with_url_special_characters_does_not_leak_in_any_encoding() {
    // `%`・`+`・`&`・`=`・`/`・空白・非 ASCII を含む
    const S: &str = "pa ss&w=rd/é+100%";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            // 受け取ったフォームを解読して、元の値と URL をそのまま返す
            let decoded: Vec<(String, String)> = url::form_urlencoded::parse(&req.body)
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let mut v = echo_json(req);
            v["decoded"] = serde_json::json!(decoded);
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(v.to_string())
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("pw", S)]);

    let mut input = adhoc(&project, "POST", "{{baseUrl}}/login?next={{pw}}");
    input.request.body = Body::Form(vec![kv(ParamIn::Query, "pw", "{{pw}}")]);
    let r = send(&f.state, input).await;
    assert_eq!(response_of(&r).status, 200);

    let e = latest(&f, &project);
    // サーバーが解読した値は、本文の JSON に生のまま入る（`"decoded":[["pw","…"]]`）。JSON の文字列としてエスケープされる
    // ことはない文字だけなので、そのまま探せる
    assert!(e.response_body.as_deref().unwrap().contains("••••"));
    assert_clean(
        &f,
        &project,
        &[
            S,
            // フォームの形・クエリの形
            "pa+ss%26w%3Drd%2F%C3%A9%2B100%25",
            "pa%20ss%26w%3Drd%2F%C3%A9%2B100%25",
        ],
    );
}

#[tokio::test]
async fn secret_in_a_header_value_with_an_ordinary_name_and_echoed_response_headers() {
    const S: &str = "HDR-SECRET-5521";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            let trace = req
                .headers
                .get("x-trace")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default();
            ResponseTemplate::new(200)
                // ヘッダーの値にもエコーする（名前は普通）
                .insert_header("x-echoed-trace", trace.as_str())
                .insert_header("x-request-id", format!("rid-{trace}").as_str())
                .set_body_string("ok")
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("tok", S)]);
    let mut input = adhoc(&project, "GET", "{{baseUrl}}/h");
    input.request.params = vec![
        kv(ParamIn::Header, "X-Trace", "id={{tok}};more"),
        kv(ParamIn::Header, "Referer", "http://x.example/?t={{tok}}"),
    ];
    let r = send(&f.state, input).await;
    assert_eq!(response_of(&r).status, 200);
    let e = latest(&f, &project);
    let echoed = e
        .response_headers
        .iter()
        .find(|h| h.name == "x-echoed-trace")
        .unwrap();
    assert_eq!(echoed.value, "id=••••;more");
    assert_clean(&f, &project, &[S]);
}

#[tokio::test]
async fn secret_in_a_header_name_is_masked() {
    const S: &str = "NAMESECRET5521";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("tok", S)]);
    let mut input = adhoc(&project, "GET", "{{baseUrl}}/h");
    input.request.params = vec![kv(ParamIn::Header, "X-{{tok}}", "1")];
    let r = send(&f.state, input).await;
    assert_eq!(response_of(&r).status, 200);
    assert_clean(&f, &project, &[S]);
}

#[tokio::test]
async fn secret_in_path_fragment_and_userinfo_of_the_url() {
    const S: &str = "URLPART-SECRET-77";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(echo_json(req).to_string())
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);

    // パスとフラグメント
    let r = send(
        &f.state,
        adhoc(&project, "GET", "{{baseUrl}}/p/{{s}}/q?x=1#frag-{{s}}"),
    )
    .await;
    assert_eq!(response_of(&r).status, 200);
    let e = latest(&f, &project);
    assert!(e.url.contains("/p/••••/q"), "{}", e.url);
    assert_clean(&f, &project, &[S]);
}

#[tokio::test]
async fn secret_in_url_userinfo_is_not_leaked_through_the_basic_header_the_server_echoes() {
    // `http://user:SECRET@host/` は reqwest が Authorization: Basic base64(user:SECRET) にして送る。
    // サーバーがそのヘッダーを本文にエコーしても、履歴に平文（base64 でも）で残らない
    const S: &str = "USERINFO-SECRET-4242";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(echo_json(req).to_string())
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);
    let url = server
        .uri()
        .replace("http://", &format!("http://neko:{S}@"));
    // 環境の変数でなく、URL にシークレットの変数を書く
    let url = url.replace(S, "{{s}}") + "/ui";
    let r = send(&f.state, adhoc(&project, "GET", &url)).await;
    assert_eq!(response_of(&r).status, 200);
    let b64 = base64::engine::general_purpose::STANDARD.encode(format!("neko:{S}"));
    assert!(
        response_of(&r).body.content.contains(&b64),
        "サーバーは Basic を受け取ったはず（前提の確認）: {}",
        response_of(&r).body.content
    );
    assert_clean(&f, &project, &[S, &b64]);
}

#[tokio::test]
async fn redirects_final_url_and_location_header_are_masked() {
    const S: &str = "REDIR-SECRET-31337";
    let f = fx();
    let server = MockServer::start().await;
    let s = S.to_string();
    Mock::given(any())
        .respond_with(move |req: &Request| {
            if req.url.path() == "/start" {
                ResponseTemplate::new(302)
                    .insert_header("location", format!("/landing?token={s}&n=1").as_str())
            } else {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(echo_json(req).to_string())
            }
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);

    // 追う: 最終 URL にシークレット（リクエストがその変数を使っているので、伏せ字の対象になる）
    let r = send(
        &f.state,
        adhoc(&project, "GET", "{{baseUrl}}/start?k={{s}}"),
    )
    .await;
    assert_eq!(response_of(&r).status, 200);
    let e = latest(&f, &project);
    let final_url = e.final_url.as_deref().expect("final_url がある");
    assert!(final_url.contains("token=••••"), "{final_url}");
    assert_clean(&f, &project, &[S]);

    // 追わない: Location ヘッダーにシークレット
    let r = send_with(
        &f.state,
        adhoc(&project, "GET", "{{baseUrl}}/start?k={{s}}"),
        Some(SendSettings {
            follow_redirects: false,
            ..SendSettings::default()
        }),
    )
    .await;
    assert_eq!(response_of(&r).status, 302);
    let e = latest(&f, &project);
    let loc = e
        .response_headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("location"))
        .unwrap();
    assert!(loc.value.contains("token=••••"), "{}", loc.value);
    assert_clean(&f, &project, &[S]);
}

#[tokio::test]
async fn gzip_and_shift_jis_responses_that_echo_the_secret_are_masked() {
    use std::io::Write;
    const S: &str = "秘密鍵ねこ-GZ-SJ-8080";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            if req.url.path() == "/gzip" {
                let mut enc =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                enc.write_all(format!("echo:{body}").as_bytes()).unwrap();
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain; charset=utf-8")
                    .insert_header("content-encoding", "gzip")
                    .set_body_bytes(enc.finish().unwrap())
            } else {
                let text = format!("エコー:{body}");
                let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(&text);
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain; charset=Shift_JIS")
                    .set_body_bytes(bytes.into_owned())
            }
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);
    for p in ["gzip", "sjis"] {
        let mut input = adhoc(&project, "POST", &format!("{{{{baseUrl}}}}/{p}"));
        input.request.body = Body::Text("v={{s}}".into());
        let r = send(&f.state, input).await;
        let resp = response_of(&r);
        assert_eq!(resp.status, 200);
        assert!(
            resp.body.content.contains(S),
            "{p}: デコードできている（前提）"
        );
        let e = latest(&f, &project);
        assert!(
            e.response_body.as_deref().unwrap().contains("••••"),
            "{p}: {:?}",
            e.response_body
        );
    }
    assert_clean(&f, &project, &[S]);
}

/// すべての文字を Unicode エスケープ（バックスラッシュ + u + 16 進 4 桁。UTF-16 の単位ごと）にした形。
fn unicode_escaped(s: &str) -> String {
    let mut out = String::new();
    for unit in s.encode_utf16() {
        out.push(char::from(92u8));
        out.push('u');
        out.push_str(&format!("{unit:04x}"));
    }
    out
}

/// JSON の文字列にエスケープされた形（スラッシュ・引用符・バックスラッシュ・Unicode エスケープ）で、シークレットが応答に入る。
/// 保存した本文を JSON として読み直して、元のシークレットが復元できてはいけない。
#[tokio::test]
async fn json_escaped_echo_of_the_secret_is_masked() {
    let cases: Vec<(&str, String)> = vec![
        // (シークレット, サーバーが返す JSON の本文)
        ("ab/cd-SECRET-1", r#"{"echo":"ab\/cd-SECRET-1"}"#.into()),
        ("q\"uote-SECRET-2", r#"{"echo":"q\"uote-SECRET-2"}"#.into()),
        (
            "back\\slash-SECRET-3",
            r#"{"echo":"back\\slash-SECRET-3"}"#.into(),
        ),
        // 非 ASCII・ASCII を Unicode エスケープする言語（PHP・Python の ensure_ascii など）
        (
            "ねこ-SECRET-4",
            format!(r#"{{"echo":"{}"}}"#, unicode_escaped("ねこ-SECRET-4")),
        ),
        (
            "plainASCII-SECRET-5",
            format!(r#"{{"echo":"{}"}}"#, unicode_escaped("plainASCII-SECRET-5")),
        ),
    ];
    let mut leaked: Vec<String> = Vec::new();
    for (secret, body) in cases {
        let f = fx();
        let server = MockServer::start().await;
        let b = body.clone();
        Mock::given(any())
            .respond_with(move |_: &Request| {
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(b.clone())
            })
            .mount(&server)
            .await;
        let (project, _env) = project_with_env(&f, &server.uri(), &[("s", secret)]);
        let mut input = adhoc(&project, "POST", "{{baseUrl}}/echo");
        input.request.body = Body::Text("{{s}}".into());
        send(&f.state, input).await;
        let e = latest(&f, &project);
        let stored = e.response_body.clone().unwrap();
        // JSON として読み直した文字列に、元のシークレットが含まれていたら漏れている
        let decoded = serde_json::from_str::<serde_json::Value>(&stored)
            .map(|v| v.to_string())
            .unwrap_or_else(|_| stored.clone());
        let decoded_strings = serde_json::from_str::<serde_json::Value>(&stored)
            .ok()
            .and_then(|v| v["echo"].as_str().map(str::to_string))
            .unwrap_or_default();
        eprintln!("CASE {secret:?} => {stored}");
        if stored.contains(secret) || decoded_strings.contains(secret) {
            leaked.push(format!("{secret:?} → 保存された本文 {stored} ({decoded})"));
        }
    }
    assert!(leaked.is_empty(), "漏れた形:\n{}", leaked.join("\n"));
}

/// base64 にした形（Basic 以外）で応答に入る。設計書は「シークレットの値が現れた箇所」なので仕様の範囲外とも読める。
#[tokio::test]
async fn base64_echo_of_a_secret_is_masked() {
    const S: &str = "B64-SECRET-value-1234";
    let f = fx();
    let server = MockServer::start().await;
    let std_b64 = base64::engine::general_purpose::STANDARD.encode(S);
    let url_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(S);
    let b = format!(r#"{{"std":"{std_b64}","url":"{url_b64}"}}"#);
    Mock::given(any())
        .respond_with(move |_: &Request| {
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(b.clone())
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);
    let mut input = adhoc(&project, "POST", "{{baseUrl}}/echo");
    input.request.body = Body::Text("{{s}}".into());
    send(&f.state, input).await;
    assert_clean(&f, &project, &[&std_b64, &url_b64]);
}

#[tokio::test]
async fn secret_straddling_the_one_megabyte_cut_leaves_no_fragment() {
    const S: &str = "CUT-SECRET-0123456789";
    const MB: usize = 1024 * 1024;
    let f = fx();
    let server = MockServer::start().await;
    // 応答は、1 MB の切り口の 7 文字手前からシークレットが始まる
    let mut resp = "x".repeat(MB - 7);
    resp.push_str(S);
    resp.push_str(&"y".repeat(100));
    Mock::given(any())
        .respond_with(move |_: &Request| ResponseTemplate::new(200).set_body_string(resp.clone()))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);
    // 送るボディも同じ切り口
    let mut body = "z".repeat(MB - 7);
    body.push_str("{{s}}");
    body.push_str(&"w".repeat(100));
    let mut input = adhoc(&project, "POST", "{{baseUrl}}/big");
    input.request.body = Body::Text(body);
    let r = send(&f.state, input).await;
    assert_eq!(response_of(&r).status, 200);

    let e = latest(&f, &project);
    assert!(e.response_body_truncated && e.request_body_truncated);
    assert!(e.response_body.as_ref().unwrap().len() <= MB);
    assert!(e.request_body.as_ref().unwrap().len() <= MB);
    // シークレットの先頭の断片（4 文字以上）が切り口に残らない
    for len in 4..S.len() {
        let frag = &S[..len];
        assert!(
            !e.response_body.as_ref().unwrap().ends_with(frag),
            "応答の切り口に {frag}"
        );
        assert!(
            !e.request_body.as_ref().unwrap().ends_with(frag),
            "送ったボディの切り口に {frag}"
        );
    }
    assert_clean(&f, &project, &[S]);
}

#[tokio::test]
async fn secret_straddling_the_ten_megabyte_preview_limit_leaves_no_fragment() {
    const S: &str = "TEN-SECRET-0123456789";
    const MB10: usize = 10 * 1024 * 1024;
    let f = fx();
    let server = MockServer::start().await;
    let mut resp = "x".repeat(MB10 - 8);
    resp.push_str(S);
    resp.push_str(&"y".repeat(64));
    Mock::given(any())
        .respond_with(move |_: &Request| ResponseTemplate::new(200).set_body_string(resp.clone()))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);
    let mut s = AppSettings::default();
    s.history.omit_large_bodies = false;
    f.state.app_settings_set(&s).unwrap();
    let mut input = adhoc(&project, "POST", "{{baseUrl}}/big");
    input.request.body = Body::Text("{{s}}".into());
    send(&f.state, input).await;
    let e = latest(&f, &project);
    let body = e.response_body.unwrap();
    for len in 4..S.len() {
        assert!(!body.ends_with(&S[..len]), "切り口に {}", &S[..len]);
    }
}

#[tokio::test]
async fn error_messages_do_not_contain_secrets_from_userinfo_path_or_query() {
    const S: &str = "ERR-SECRET-88114";
    let f = fx();
    // 閉じたポート
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (project, _env) = project_with_env(&f, &format!("http://127.0.0.1:{port}"), &[("s", S)]);
    for url in [
        format!("http://neko:{{{{s}}}}@127.0.0.1:{port}/p/{{{{s}}}}?k={{{{s}}}}#f{{{{s}}}}"),
        "{{baseUrl}}/x/{{s}}".to_string(),
    ] {
        let r = send(&f.state, adhoc(&project, "GET", &url)).await;
        let e = error_of(&r);
        assert!(!e.message().contains(S), "{}", e.message());
    }
    assert_clean(&f, &project, &[S]);
    for e in all_history(&f, &project) {
        assert_eq!(e.error_kind.as_deref(), Some("connect"));
    }
}

#[tokio::test]
async fn a_secret_with_a_newline_cannot_inject_headers_and_is_not_echoed_in_the_error() {
    const S: &str = "NL-SECRET-3141\r\nX-Evil: 1";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("s", S)]);
    let mut input = adhoc(&project, "GET", "{{baseUrl}}/h");
    input.request.params = vec![kv(ParamIn::Header, "X-Token", "{{s}}")];
    let r = send(&f.state, input).await;
    match &r.result {
        SendOutcome::Failure { error } => {
            assert!(
                !error.message().contains("NL-SECRET-3141"),
                "{}",
                error.message()
            );
        }
        SendOutcome::Success { .. } => {
            // 送れたなら、ヘッダーは増えていない
            let reqs = server.received_requests().await.unwrap();
            assert!(reqs[0].headers.get("x-evil").is_none(), "ヘッダーの注入");
        }
    }
    assert_clean(&f, &project, &["NL-SECRET-3141"]);
}

#[tokio::test]
async fn received_session_values_are_masked_even_when_the_automation_is_off() {
    const SESS: &str = "RECV-SESS-ABC-9981";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|_: &Request| {
            ResponseTemplate::new(200)
                .append_header("set-cookie", format!("SESSION={SESS}; Path=/").as_str())
                .set_body_string(format!("your session is {SESS}"))
        })
        .mount(&server)
        .await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    f.state
        .project_settings_set(
            &project.id,
            &ProjectSettings {
                auto_session: false,
                auto_csrf: false,
            },
        )
        .unwrap();
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/s")).await;
    assert!(r.auto_updates.is_empty(), "自動化はオフ");
    assert!(f
        .secrets
        .get(&SecretKey::new(&project.id, &env.id, "session"))
        .unwrap()
        .is_none());
    let e = latest(&f, &project);
    assert_eq!(e.response_body.as_deref(), Some("your session is ••••"));
    assert_clean(&f, &project, &[SESS]);
}

// =====================================================================================
// 2. 履歴の一覧（期間・タイムゾーン・件数の上限・すべて削除）
// =====================================================================================

/// 履歴 1 件（送信せずに直接入れる用）。
fn mk_entry(project_id: &str, id: &str, sent_at: &str, url: &str) -> HistoryEntry {
    HistoryEntry {
        id: id.to_string(),
        project_id: project_id.to_string(),
        sent_at: sent_at.to_string(),
        environment_id: None,
        environment_name: None,
        op_key: None,
        op_summary: None,
        method: "GET".into(),
        url: url.to_string(),
        final_url: None,
        request_headers: vec![],
        request_body: None,
        request_body_kind: HistoryBodyKind::None,
        request_body_size: 0,
        request_body_truncated: false,
        request_parts: vec![],
        request_file: None,
        status: Some(200),
        status_text: Some("OK".into()),
        response_headers: vec![HeaderPair {
            name: "content-type".into(),
            value: "text/plain".into(),
        }],
        response_content_type: Some("text/plain".into()),
        response_body: Some(format!("body of {id}")),
        response_binary: false,
        response_body_truncated: false,
        response_size: 10,
        timing: None,
        error: None,
        error_kind: None,
        input: None,
        masked_fields: vec![],
        override_names: vec![],
    }
}

/// アプリとは別の接続で履歴を直接入れる（`sent_at` を自由に決められる）。
fn raw_db(f: &Fx) -> nekoreq_storage::Database {
    nekoreq_storage::Database::open(f.data.join("data.sqlite")).unwrap()
}

fn count_rows(f: &Fx, table: &str) -> i64 {
    raw_connection(&f.data)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn range(from: Option<&str>, to: Option<&str>, offset: Option<i32>) -> HistoryQuery {
    HistoryQuery {
        from: from.map(str::to_string),
        to: to.map(str::to_string),
        utc_offset_minutes: offset,
        limit: Some(500),
        ..HistoryQuery::default()
    }
}

fn ids_in(f: &Fx, project: &Project, q: &HistoryQuery) -> Vec<String> {
    f.state
        .history_list(&project.id, q)
        .unwrap()
        .items
        .into_iter()
        .map(|i| i.id)
        .collect()
}

#[test]
fn day_boundaries_follow_the_given_utc_offset() {
    let f = fx();
    let project = f.state.project_create("p").unwrap();
    let db = raw_db(&f);
    // 日本時間（+9）の 11/16 23:59:59.999 / 11/17 00:00:00.000 / 11/17 23:59:59.999 / 11/18 00:00:00.000
    for (id, at) in [
        ("A", "2026-11-16T14:59:59.999Z"),
        ("B", "2026-11-16T15:00:00.000Z"),
        ("C", "2026-11-17T14:59:59.999Z"),
        ("D", "2026-11-17T15:00:00.000Z"),
    ] {
        db.history_insert(&mk_entry(&project.id, id, at, "http://x/"), 0)
            .unwrap();
    }
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    // UTC+9: 17 日は B と C だけ
    let q = range(Some("2026-11-17"), Some("2026-11-17"), Some(540));
    assert_eq!(sorted(ids_in(&f, &project, &q)), ["B", "C"]);
    let q = range(Some("2026-11-17"), None, Some(540));
    assert_eq!(sorted(ids_in(&f, &project, &q)), ["B", "C", "D"]);
    let q = range(None, Some("2026-11-16"), Some(540));
    assert_eq!(ids_in(&f, &project, &q), ["A"]);
    // 一覧の localDate
    let page = f
        .state
        .history_list(&project.id, &range(None, None, Some(540)))
        .unwrap();
    let dates: Vec<(&str, &str)> = page
        .items
        .iter()
        .map(|i| (i.id.as_str(), i.local_date.as_str()))
        .collect();
    assert_eq!(
        dates,
        [
            ("D", "2026-11-18"),
            ("C", "2026-11-17"),
            ("B", "2026-11-17"),
            ("A", "2026-11-16")
        ]
    );
    // UTC: 16 日は A と B
    let q = range(Some("2026-11-16"), Some("2026-11-16"), Some(0));
    assert_eq!(sorted(ids_in(&f, &project, &q)), ["A", "B"]);
    // UTC-8（-480）: 17 日は C と D（D は 11/17 07:00）
    let q = range(Some("2026-11-17"), Some("2026-11-17"), Some(-480));
    assert_eq!(sorted(ids_in(&f, &project, &q)), ["C", "D"]);
    // UTC+14: A は 11/17 04:59
    let page = f
        .state
        .history_list(&project.id, &range(None, None, Some(840)))
        .unwrap();
    assert_eq!(
        page.items.iter().find(|i| i.id == "A").unwrap().local_date,
        "2026-11-17"
    );
    // 範囲外のオフセットは ±14 時間に丸める（エラーにしない）
    for off in [5000, i32::MAX] {
        let page = f
            .state
            .history_list(&project.id, &range(None, None, Some(off)))
            .unwrap();
        assert_eq!(
            page.items.iter().find(|i| i.id == "A").unwrap().local_date,
            "2026-11-17",
            "offset {off}"
        );
    }
    for off in [-5000, i32::MIN] {
        let page = f
            .state
            .history_list(&project.id, &range(None, None, Some(off)))
            .unwrap();
        assert_eq!(
            page.items.iter().find(|i| i.id == "D").unwrap().local_date,
            "2026-11-17",
            "offset {off}"
        );
    }
    // 逆の範囲は 0 件（エラーにしない）。前後の空白は無視する
    let q = range(Some("2026-11-18"), Some("2026-11-16"), Some(540));
    assert!(ids_in(&f, &project, &q).is_empty());
    let q = range(Some(" 2026-11-17 "), Some(" 2026-11-17 "), Some(540));
    assert_eq!(ids_in(&f, &project, &q).len(), 2);
    // 空の日付は絞らない
    let q = range(Some(""), Some("  "), Some(540));
    assert_eq!(ids_in(&f, &project, &q).len(), 4);
}

#[test]
fn bad_dates_are_validation_errors_in_japanese() {
    let f = fx();
    let project = f.state.project_create("p").unwrap();
    for bad in [
        "2026/11/17",
        "abc",
        "2026-02-30",
        "2026-13-01",
        "20261117",
        "２０２６-11-17",
    ] {
        for q in [
            range(Some(bad), None, Some(0)),
            range(None, Some(bad), Some(0)),
        ] {
            let e = f.state.history_list(&project.id, &q).unwrap_err();
            assert!(matches!(e, AppError::Validation(_)), "{bad}: {e:?}");
            assert!(e.to_string().contains("日付を読めません"), "{bad}: {e}");
        }
    }
    // 最大の日付の翌日が作れなくてもパニックしない
    let q = range(None, Some("+262142-12-31"), Some(0));
    assert!(f.state.history_list(&project.id, &q).is_err());
}

#[test]
fn page_size_is_clamped_and_total_ignores_paging() {
    let f = fx();
    let project = f.state.project_create("p").unwrap();
    let db = raw_db(&f);
    for i in 0..30 {
        let at = format!("2026-11-01T00:00:{i:02}.000Z");
        db.history_insert(
            &mk_entry(&project.id, &format!("h{i:02}"), &at, "http://x/"),
            0,
        )
        .unwrap();
    }
    let page = |limit: Option<u32>| {
        f.state
            .history_list(
                &project.id,
                &HistoryQuery {
                    limit,
                    ..HistoryQuery::default()
                },
            )
            .unwrap()
    };
    assert_eq!(page(Some(0)).items.len(), 1, "0 は 1 に丸める");
    assert_eq!(page(Some(10)).items.len(), 10);
    assert_eq!(page(Some(10)).total, 30);
    assert_eq!(page(Some(100_000)).items.len(), 30);
    assert_eq!(page(None).items.len(), 30);
    assert!(page(Some(100_000)).next_cursor.is_none());
}

#[tokio::test]
async fn the_limit_trims_on_the_next_send_and_zero_stops_recording_without_deleting() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[]);
    let db = raw_db(&f);
    for i in 0..1200 {
        let at = format!(
            "2026-10-{:02}T{:02}:{:02}:00.000Z",
            1 + i / 1440,
            (i / 60) % 24,
            i % 60
        );
        db.history_insert(
            &mk_entry(&project.id, &format!("old{i:04}"), &at, "http://x/"),
            0,
        )
        .unwrap();
    }
    assert_eq!(count_rows(&f, "history"), 1200);
    // 件数を減らしても、次の送信までは消えない
    let mut s = AppSettings::default();
    s.history.limit = 1000;
    f.state.app_settings_set(&s).unwrap();
    assert_eq!(count_rows(&f, "history"), 1200);

    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/a")).await;
    assert_eq!(
        count_rows(&f, "history"),
        1000,
        "古いものから消えて 1,000 件"
    );
    assert_eq!(count_rows(&f, "history_fts"), 1000, "全文検索の行も同じ数");
    let newest = latest(&f, &project);
    assert!(newest.url.ends_with("/a"));
    // 一番古い 201 件が消えている
    assert!(f.state.history_get("old0000").is_err());
    assert!(f.state.history_get("old0200").is_err());
    assert!(f.state.history_get("old0201").is_ok());

    // 「保存しない」: 送信は残らず、すでにある履歴も消えない
    s.history.limit = 0;
    f.state.app_settings_set(&s).unwrap();
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/b")).await;
    assert_eq!(count_rows(&f, "history"), 1000);
}

#[tokio::test]
async fn concurrent_sends_keep_exactly_the_limit_and_the_search_index_in_step() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[]);
    let db = raw_db(&f);
    for i in 0..995 {
        let at = format!(
            "2026-10-{:02}T{:02}:{:02}:00.000Z",
            1 + i / 1440,
            (i / 60) % 24,
            i % 60
        );
        db.history_insert(
            &mk_entry(&project.id, &format!("old{i:04}"), &at, "http://x/"),
            0,
        )
        .unwrap();
    }
    let mut handles = Vec::new();
    for n in 0..30 {
        let state = f.state.clone();
        let input = adhoc(&project, "GET", &format!("{{{{baseUrl}}}}/c{n}"));
        handles.push(tokio::spawn(async move { send(&state, input).await }));
    }
    for h in handles {
        response_of(&h.await.unwrap());
    }
    assert_eq!(count_rows(&f, "history"), 1000);
    assert_eq!(count_rows(&f, "history_fts"), 1000);
    // 新しい 30 件は全部残っている（古いものが消えた）
    let page = f
        .state
        .history_list(
            &project.id,
            &HistoryQuery {
                limit: Some(30),
                ..HistoryQuery::default()
            },
        )
        .unwrap();
    assert!(
        page.items.iter().all(|i| i.url.contains("/c")),
        "{:?}",
        page.items.iter().map(|i| &i.url).collect::<Vec<_>>()
    );
    assert_eq!(page.total, 1000);
}

#[test]
fn clear_removes_only_that_project_and_the_search_rows() {
    let f = fx();
    let a = f.state.project_create("a").unwrap();
    let b = f.state.project_create("b").unwrap();
    let db = raw_db(&f);
    for i in 0..5 {
        db.history_insert(
            &mk_entry(
                &a.id,
                &format!("a{i}"),
                &format!("2026-11-01T00:00:0{i}.000Z"),
                "http://a/",
            ),
            0,
        )
        .unwrap();
    }
    for i in 0..3 {
        db.history_insert(
            &mk_entry(
                &b.id,
                &format!("b{i}"),
                &format!("2026-11-01T00:00:0{i}.000Z"),
                "http://b/",
            ),
            0,
        )
        .unwrap();
    }
    assert_eq!(f.state.history_clear(&a.id).unwrap(), 5);
    assert_eq!(
        f.state.history_clear(&a.id).unwrap(),
        0,
        "空でもエラーにしない"
    );
    assert_eq!(f.state.history_clear("no-such-project").unwrap(), 0);
    assert_eq!(count_rows(&f, "history"), 3);
    assert_eq!(count_rows(&f, "history_fts"), 3);
    assert!(f.state.history_get("a0").is_err());
    assert!(f.state.history_get("b0").is_ok());
    // 1 件削除: 2 回目は NotFound
    f.state.history_delete("b0").unwrap();
    assert!(matches!(
        f.state.history_delete("b0").unwrap_err(),
        AppError::NotFound(_)
    ));
    assert_eq!(count_rows(&f, "history_fts"), 2);
    // 検索（全文）もプロジェクトで分かれる
    let q = HistoryQuery {
        text: Some("http://b/".into()),
        ..HistoryQuery::default()
    };
    assert_eq!(f.state.history_list(&b.id, &q).unwrap().total, 2);
    assert_eq!(f.state.history_list(&a.id, &q).unwrap().total, 0);
}

// =====================================================================================
// 3. cookieAuth・CSRF
// =====================================================================================

type Script = Arc<std::sync::Mutex<Vec<String>>>;

/// `/set` はスクリプトの `Set-Cookie` を返し、それ以外は受け取った Cookie と X-XSRF-TOKEN を JSON で返すサーバー。
async fn cookie_server() -> (MockServer, Script) {
    let server = MockServer::start().await;
    let script: Script = Arc::default();
    let s = script.clone();
    Mock::given(any())
        .respond_with(move |req: &Request| {
            if req.url.path() == "/set" {
                let mut t = ResponseTemplate::new(200).set_body_string("set");
                for line in s.lock().unwrap().iter() {
                    t = t.append_header("set-cookie", line.as_str());
                }
                return t;
            }
            let cookies: Vec<String> = req
                .headers
                .get_all("cookie")
                .iter()
                .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
                .collect();
            let xsrf = req
                .headers
                .get("x-xsrf-token")
                .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({"cookies": cookies, "xsrf": xsrf}).to_string())
        })
        .mount(&server)
        .await;
    (server, script)
}

fn set_script(script: &Script, lines: &[&str]) {
    *script.lock().unwrap() = lines.iter().map(|l| l.to_string()).collect();
}

/// `/echo` の応答（サーバーが受け取った Cookie ヘッダーの一覧と X-XSRF-TOKEN）。
async fn echo_seen(f: &Fx, project: &Project, method: &str) -> (Vec<String>, Option<String>) {
    let r = send(&f.state, adhoc(project, method, "{{baseUrl}}/echo")).await;
    let v: serde_json::Value = serde_json::from_str(&response_of(&r).body.content).unwrap();
    let cookies = v["cookies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    (cookies, v["xsrf"].as_str().map(str::to_string))
}

fn stored(f: &Fx, project: &Project, env: &Environment, name: &str) -> Option<String> {
    f.secrets
        .get(&SecretKey::new(&project.id, &env.id, name))
        .unwrap()
}

fn vars_named(f: &Fx, env: &Environment, name: &str) -> Vec<EnvVar> {
    f.state
        .env_list(&env.project_id)
        .unwrap()
        .into_iter()
        .find(|e| e.id == env.id)
        .unwrap()
        .vars
        .into_iter()
        .filter(|v| v.name == name)
        .collect()
}

fn names(r: &RequestFinished) -> Vec<(String, AutoUpdateAction)> {
    r.auto_updates
        .iter()
        .map(|u| (u.name.clone(), u.action))
        .collect()
}

#[tokio::test]
async fn several_set_cookies_with_attributes_update_only_session_and_xsrf() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    set_script(
        &script,
        &[
            "SESSION=SESS-A1; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=3600",
            "XSRF-TOKEN=XS-A1; Path=/; Expires=Wed, 21 Oct 2037 07:28:00 GMT",
            "JSESSIONID=junk; Path=/",
            "theme=dark",
        ],
    );
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(
        names(&r),
        [
            ("session".to_string(), AutoUpdateAction::Updated),
            ("xsrfToken".to_string(), AutoUpdateAction::Updated)
        ]
    );
    assert!(r.auto_update_skipped.is_empty());
    assert_eq!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("SESS-A1")
    );
    assert_eq!(
        stored(&f, &project, &env, "xsrfToken").as_deref(),
        Some("XS-A1")
    );
    // 変数はシークレットとして 1 つずつ。ほかの Cookie は変数にならない
    for n in ["session", "xsrfToken"] {
        let v = vars_named(&f, &env, n);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].value, VarValue::Secret);
    }
    let all: Vec<String> = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|e| e.id == env.id)
        .unwrap()
        .vars
        .into_iter()
        .map(|v| v.name)
        .collect();
    assert_eq!(all, ["baseUrl", "session", "xsrfToken"]);
    // 次の POST: Cookie は 1 本にまとまり、ヘッダーも付く
    let (cookies, xsrf) = echo_seen(&f, &project, "POST").await;
    assert_eq!(cookies.len(), 1, "{cookies:?}");
    assert!(cookies[0].contains("SESSION=SESS-A1"), "{cookies:?}");
    assert!(cookies[0].contains("XSRF-TOKEN=XS-A1"), "{cookies:?}");
    assert_eq!(xsrf.as_deref(), Some("XS-A1"));
    // GET には Cookie だけ
    let (cookies, xsrf) = echo_seen(&f, &project, "GET").await;
    assert!(cookies[0].contains("XSRF-TOKEN=XS-A1"));
    assert_eq!(xsrf, None);
    assert_clean(&f, &project, &["SESS-A1", "XS-A1"]);
}

#[tokio::test]
async fn delete_and_reissue_in_one_response_follow_the_header_order() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    // 先に値を入れる
    set_script(&script, &["SESSION=OLD-S; Path=/"]);
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("OLD-S")
    );

    // 削除のあとに再発行 → 新しい値
    set_script(
        &script,
        &[
            "SESSION=; Path=/; Max-Age=0",
            "SESSION=NEW-S; Path=/; HttpOnly",
        ],
    );
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(
        names(&r),
        [("session".to_string(), AutoUpdateAction::Updated)]
    );
    assert_eq!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("NEW-S")
    );

    // 再発行のあとに削除（Expires が過去）→ 消える。変数の宣言は残る
    set_script(
        &script,
        &[
            "SESSION=NEWER-S; Path=/",
            "SESSION=deleted; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        ],
    );
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(
        names(&r),
        [("session".to_string(), AutoUpdateAction::Cleared)]
    );
    assert_eq!(stored(&f, &project, &env, "session"), None);
    assert_eq!(vars_named(&f, &env, "session").len(), 1);
    let (cookies, _) = echo_seen(&f, &project, "GET").await;
    assert!(
        cookies.iter().all(|c| !c.contains("SESSION=")),
        "消したあとは送らない: {cookies:?}"
    );

    // Max-Age が優先（Expires が未来でも Max-Age=0 なら消す。Max-Age>0 なら Expires が過去でも残す）
    set_script(&script, &["SESSION=M1; Path=/"]);
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    set_script(
        &script,
        &["SESSION=x; Max-Age=0; Expires=Wed, 21 Oct 2037 07:28:00 GMT"],
    );
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(
        names(&r),
        [("session".to_string(), AutoUpdateAction::Cleared)]
    );
    set_script(
        &script,
        &["SESSION=M2; Max-Age=60; Expires=Thu, 01 Jan 1970 00:00:00 GMT"],
    );
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(stored(&f, &project, &env, "session").as_deref(), Some("M2"));
}

#[tokio::test]
async fn cookie_names_are_case_sensitive_and_lowercase_ones_are_ignored() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    set_script(
        &script,
        &["session=lower; Path=/", "Session=Mixed", "xsrf-token=low"],
    );
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert!(r.auto_updates.is_empty(), "{:?}", r.auto_updates);
    assert_eq!(stored(&f, &project, &env, "session"), None);
    // 正しい名前が混じれば、それだけ
    set_script(&script, &["session=lower", "SESSION=UP", "Session=Mixed"]);
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(stored(&f, &project, &env, "session").as_deref(), Some("UP"));
}

#[tokio::test]
async fn awkward_cookie_values_round_trip_exactly() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    let long = "L".repeat(8000);
    let values: Vec<String> = vec![
        "abc==".into(),
        "a=b=c".into(),
        "abc+/==".into(),
        "%3D%3D%20x".into(),
        "\"quoted-value\"".into(),
        "ねこ-utf8".into(),
        long,
    ];
    for v in values {
        set_script(&script, &[&format!("SESSION={v}; Path=/")]);
        send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
        assert_eq!(
            stored(&f, &project, &env, "session").as_deref(),
            Some(v.as_str()),
            "保持した値"
        );
        let (cookies, _) = echo_seen(&f, &project, "GET").await;
        let sent = cookies
            .iter()
            .flat_map(|c| c.split("; "))
            .find_map(|p| p.strip_prefix("SESSION="))
            .unwrap_or_else(|| panic!("SESSION が送られていない: {v:?}"));
        assert_eq!(sent, v, "送り返した値");
    }
}

#[tokio::test]
async fn base_url_spellings_that_name_the_same_origin_attach_and_save_cookies() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let uri = server.uri(); // http://127.0.0.1:PORT
    let (project, env) = project_with_env(&f, &uri, &[]);
    let host_port = uri.trim_start_matches("http://").to_string();
    let spellings = [
        format!("{uri}/"),
        format!("{uri}/api/v1/"),
        uri.to_uppercase(),
        host_port.clone(),
        format!("  {uri}  "),
    ];
    for (i, base) in spellings.iter().enumerate() {
        let mut e = f
            .state
            .env_list(&project.id)
            .unwrap()
            .into_iter()
            .find(|e| e.id == env.id)
            .unwrap();
        for v in &mut e.vars {
            if v.name == "baseUrl" {
                v.value = VarValue::Plain(base.clone());
            }
        }
        f.state.env_save(&e).unwrap();
        let value = format!("SPELL-{i}");
        set_script(&script, &[&format!("SESSION={value}; Path=/")]);
        // 送り先は絶対 URL（baseUrl の書き方に関係なく、origin が同じなら付く・保存される）
        let r = send(&f.state, adhoc(&project, "GET", &format!("{uri}/set"))).await;
        assert_eq!(
            names(&r),
            [("session".to_string(), AutoUpdateAction::Updated)],
            "baseUrl = {base:?}: {:?}",
            r.auto_update_skipped
        );
        let r = send(&f.state, adhoc(&project, "GET", &format!("{uri}/echo"))).await;
        let v: serde_json::Value = serde_json::from_str(&response_of(&r).body.content).unwrap();
        assert!(
            v["cookies"][0]
                .as_str()
                .unwrap_or("")
                .contains(&format!("SESSION={value}")),
            "baseUrl = {base:?}: {v}"
        );
    }
    // origin が違う書き方（localhost と 127.0.0.1）は付けず、保存もしない
    let mut e = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|e| e.id == env.id)
        .unwrap();
    let port = host_port.rsplit(':').next().unwrap();
    for v in &mut e.vars {
        if v.name == "baseUrl" {
            v.value = VarValue::Plain(format!("http://localhost:{port}"));
        }
    }
    f.state.env_save(&e).unwrap();
    set_script(&script, &["SESSION=OTHER-ORIGIN"]);
    let r = send(&f.state, adhoc(&project, "GET", &format!("{uri}/set"))).await;
    assert!(r.auto_updates.is_empty());
    assert!(!r.auto_update_skipped.is_empty());
    assert_ne!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("OTHER-ORIGIN")
    );
    let r = send(&f.state, adhoc(&project, "GET", &format!("{uri}/echo"))).await;
    let v: serde_json::Value = serde_json::from_str(&response_of(&r).body.content).unwrap();
    assert_eq!(
        v["cookies"].as_array().unwrap().len(),
        0,
        "別の origin には付けない: {v}"
    );
}

/// Domain・Path・Secure・SameSite は見ない（送り先の origin が環境の baseUrl と同じかどうかだけで決める）。
/// ブラウザなら捨てる Set-Cookie（別の Domain・合わない Path・http での Secure）も保存する。仕様の記載がないので現状を固定しておく。
#[tokio::test]
async fn set_cookie_domain_path_and_secure_attributes_are_ignored_today() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    set_script(
        &script,
        &["SESSION=DOM; Domain=other.example; Path=/admin; Secure; SameSite=None"],
    );
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    assert_eq!(
        names(&r),
        [("session".to_string(), AutoUpdateAction::Updated)]
    );
    assert_eq!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("DOM")
    );
    // Path=/admin でも、/echo に付く
    let (cookies, _) = echo_seen(&f, &project, "GET").await;
    assert!(cookies[0].contains("SESSION=DOM"), "{cookies:?}");
}

#[tokio::test]
async fn a_set_cookie_on_a_redirect_response_is_saved() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            if req.url.path() == "/login" {
                ResponseTemplate::new(302)
                    .insert_header("location", "/home")
                    .append_header("set-cookie", "SESSION=FROM-302; Path=/; HttpOnly")
                    .append_header("set-cookie", "XSRF-TOKEN=XS-302; Path=/")
            } else {
                let cookie = req
                    .headers
                    .get("cookie")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default();
                ResponseTemplate::new(200).set_body_string(format!("home cookie={cookie}"))
            }
        })
        .mount(&server)
        .await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    let r = send(&f.state, adhoc(&project, "POST", "{{baseUrl}}/login")).await;
    let resp = response_of(&r);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.redirect_count, 1);
    // 302 の Set-Cookie で SESSION が保存され、追従先にも付く（ブラウザと同じ）
    assert_eq!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("FROM-302")
    );
    assert!(
        resp.body.content.contains("SESSION=FROM-302"),
        "{}",
        resp.body.content
    );
}

#[tokio::test]
async fn concurrent_sends_that_each_set_a_session_do_not_corrupt_the_environment() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            let n = req.url.path().trim_start_matches('/').to_string();
            ResponseTemplate::new(200)
                .append_header("set-cookie", format!("SESSION=S-{n}; Path=/").as_str())
                .append_header("set-cookie", format!("XSRF-TOKEN=X-{n}; Path=/").as_str())
                .set_body_string("ok")
        })
        .mount(&server)
        .await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    let mut handles = Vec::new();
    for n in 0..20 {
        let state = f.state.clone();
        let input = adhoc(&project, "GET", &format!("{{{{baseUrl}}}}/{n}"));
        handles.push(tokio::spawn(async move { send(&state, input).await }));
    }
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap());
    }
    for r in &results {
        assert_eq!(response_of(r).status, 200);
        assert!(
            r.auto_update_skipped.is_empty(),
            "{:?}",
            r.auto_update_skipped
        );
    }
    // 宣言は 1 つずつ
    assert_eq!(vars_named(&f, &env, "session").len(), 1);
    assert_eq!(vars_named(&f, &env, "xsrfToken").len(), 1);
    // 保存された値は、どれかの応答の値で、session と xsrfToken が同じ応答のもの（半端に混ざらない）とは限らないが、
    // 少なくとも有効な組のどれか
    let s = stored(&f, &project, &env, "session").unwrap();
    let x = stored(&f, &project, &env, "xsrfToken").unwrap();
    let sn = s.strip_prefix("S-").expect("S-n の形");
    let xn = x.strip_prefix("X-").expect("X-n の形");
    assert!(sn.parse::<u32>().unwrap() < 20 && xn.parse::<u32>().unwrap() < 20);
    // 少なくとも 1 件は「更新した」と報告されている
    assert!(results.iter().any(|r| !r.auto_updates.is_empty()));
}

#[tokio::test]
async fn a_slow_response_that_finishes_last_wins() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            let t = ResponseTemplate::new(200)
                .append_header(
                    "set-cookie",
                    format!("SESSION=S{}; Path=/", req.url.path().replace('/', "-")).as_str(),
                )
                .set_body_string("ok");
            if req.url.path() == "/slow" {
                t.set_delay(Duration::from_millis(300))
            } else {
                t
            }
        })
        .mount(&server)
        .await;
    let (project, env) = project_with_env(&f, &server.uri(), &[]);
    let (a, b) = tokio::join!(
        send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/slow")),
        send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/fast"))
    );
    assert_eq!(response_of(&a).status, 200);
    assert_eq!(response_of(&b).status, 200);
    // 終わった順に反映する: 遅い応答が最後
    assert_eq!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("S-slow")
    );
    assert_eq!(vars_named(&f, &env, "session").len(), 1);
}

#[tokio::test]
async fn manual_cookie_header_and_cookie_param_coexist_with_the_automatic_ones() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[]);
    set_script(
        &script,
        &["SESSION=AUTO-S; Path=/", "XSRF-TOKEN=AUTO-X; Path=/"],
    );
    send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;

    // 手入力の Cookie ヘッダー + Cookie パラメータ + 自動のもの → Cookie ヘッダーは 1 本
    let mut input = adhoc(&project, "POST", "{{baseUrl}}/echo");
    input.request.params = vec![
        kv(ParamIn::Header, "Cookie", "theme=dark"),
        kv(ParamIn::Cookie, "lang", "ja"),
    ];
    let r = send(&f.state, input).await;
    let v: serde_json::Value = serde_json::from_str(&response_of(&r).body.content).unwrap();
    let cookies = v["cookies"].as_array().unwrap();
    assert_eq!(cookies.len(), 1, "{v}");
    let c = cookies[0].as_str().unwrap();
    for want in [
        "theme=dark",
        "lang=ja",
        "SESSION=AUTO-S",
        "XSRF-TOKEN=AUTO-X",
    ] {
        assert!(c.contains(want), "{want} が無い: {c}");
    }
    assert_eq!(v["xsrf"], "AUTO-X");

    // 手入力の SESSION が優先（自動のものを重ねない）。X-XSRF-TOKEN の手入力も優先
    let mut input = adhoc(&project, "POST", "{{baseUrl}}/echo");
    input.request.params = vec![
        kv(ParamIn::Header, "Cookie", "SESSION=MANUAL-S"),
        kv(ParamIn::Header, "X-XSRF-TOKEN", "MANUAL-X"),
    ];
    let r = send(&f.state, input).await;
    let v: serde_json::Value = serde_json::from_str(&response_of(&r).body.content).unwrap();
    let c = v["cookies"][0].as_str().unwrap();
    assert!(
        c.contains("SESSION=MANUAL-S") && !c.contains("AUTO-S"),
        "{c}"
    );
    assert_eq!(v["xsrf"], "MANUAL-X");
    // 履歴: 手で書いた Cookie ヘッダー・X-XSRF-TOKEN の値は残らない
    assert_clean(&f, &project, &["MANUAL-S", "MANUAL-X", "AUTO-S", "AUTO-X"]);
}

#[tokio::test]
async fn disabled_session_automation_leaves_a_manually_set_session_variable_untouched() {
    let f = fx();
    let (server, script) = cookie_server().await;
    let (project, env) = project_with_env(&f, &server.uri(), &[("session", "HAND-SESSION-1")]);
    f.state
        .project_settings_set(
            &project.id,
            &ProjectSettings {
                auto_session: false,
                auto_csrf: true,
            },
        )
        .unwrap();
    set_script(
        &script,
        &["SESSION=SERVER-S; Path=/", "XSRF-TOKEN=SERVER-X; Path=/"],
    );
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/set")).await;
    // CSRF だけ更新される
    assert_eq!(
        names(&r),
        [("xsrfToken".to_string(), AutoUpdateAction::Updated)]
    );
    assert_eq!(
        stored(&f, &project, &env, "session").as_deref(),
        Some("HAND-SESSION-1")
    );
    // SESSION は付かない（自動化オフ）が、XSRF-TOKEN は付く
    let (cookies, _) = echo_seen(&f, &project, "POST").await;
    assert!(
        !cookies.iter().any(|c| c.contains("SESSION=")),
        "{cookies:?}"
    );
    assert!(cookies[0].contains("XSRF-TOKEN=SERVER-X"));
}

// =====================================================================================
// 4. cURL の取り込み・書き出し
// =====================================================================================

const SMALL_SPEC: &str = r#"openapi: 3.0.3
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
      responses: {'200': {description: ok}}
    delete:
      operationId: deleteItem
      parameters:
        - {name: id, in: path, required: true, schema: {type: string}}
      responses: {'204': {description: ok}}
  /items:
    get:
      operationId: listItems
      parameters:
        - {name: tag, in: query, schema: {type: array, items: {type: string}}}
      responses: {'200': {description: ok}}
    post:
      operationId: createItem
      requestBody:
        content:
          application/json:
            schema: {type: object}
      responses: {'201': {description: ok}}
"#;

async fn import_yaml(f: &Fx, yaml: &str) -> Project {
    let entry = f
        .dir
        .path()
        .join(format!("spec-{}.yaml", nekoreq_core::new_id()));
    std::fs::write(&entry, yaml).unwrap();
    import_entry(f, entry).await
}

async fn import_entry(f: &Fx, entry: PathBuf) -> Project {
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

/// 取り込んだプロジェクトの（グローバルでない）環境の baseUrl を書き換える。
fn set_base_url(f: &Fx, project: &Project, base_url: &str) -> Environment {
    let mut env = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|e| !e.is_global)
        .unwrap();
    for v in &mut env.vars {
        if v.name == "baseUrl" {
            v.value = VarValue::Plain(base_url.to_string());
        }
    }
    f.state.env_save(&env).unwrap()
}

fn fixture_entry() -> Option<PathBuf> {
    let dir = std::env::var_os("NEKOREQ_FIXTURE_DIR")?;
    let entry = PathBuf::from(dir).join("openapi.yaml");
    entry.exists().then_some(entry)
}

fn op_key(id: &str) -> OpKey {
    OpKey::Id(id.into())
}

fn path_param<'a>(c: &'a nekoreq_lib::curl::CurlImport, name: &str) -> Option<&'a KeyValue> {
    c.request
        .params
        .iter()
        .find(|p| p.location == ParamIn::Path && p.name == name)
}

#[tokio::test]
async fn real_fixture_trailing_slash_method_spellings_and_encoded_paths() {
    let Some(entry) = fixture_entry() else {
        eprintln!("NEKOREQ_FIXTURE_DIR が無いのでスキップ");
        return;
    };
    let f = fx();
    let project = import_entry(&f, entry).await;
    let parse = |t: &str| f.state.curl_parse(&project.id, t, None).unwrap();

    // 末尾スラッシュ
    let c = parse("curl 'http://localhost:8080/api/v1/video/clients/'");
    assert_eq!(
        c.op_key,
        Some(op_key("getClientList")),
        "{:?}",
        c.request.url
    );
    let c = parse("curl -X DELETE 'http://localhost:8080/api/v1/video/clients/123/'");
    assert_eq!(
        c.op_key,
        Some(op_key("deleteClient")),
        "末尾の / は無視する"
    );
    // method の書き方（小文字・くっつけた形・--request=）
    for text in [
        "curl -X delete http://localhost:8080/api/v1/video/clients/7",
        "curl -XDELETE http://localhost:8080/api/v1/video/clients/7",
        "curl --request=DELETE http://localhost:8080/api/v1/video/clients/7",
        "curl --request DELETE --url http://localhost:8080/api/v1/video/clients/7",
    ] {
        let c = parse(text);
        assert_eq!(c.op_key, Some(op_key("deleteClient")), "{text}");
        assert_eq!(path_param(&c, "clientId").unwrap().value, "7", "{text}");
    }
    // 当てはまらない method（HEAD・OPTIONS・PATCH）は「その他」
    for m in ["HEAD", "OPTIONS", "PATCH"] {
        let c = parse(&format!(
            "curl -X {m} http://localhost:8080/api/v1/video/clients/7"
        ));
        assert_eq!(c.target, CurlTarget::Adhoc, "{m}");
        assert_eq!(c.request.method, m);
    }
    // パーセントエンコードされた日本語の path パラメータ
    let c = parse(
        "curl -X DELETE 'http://localhost:8080/api/v1/video/clients/%E3%83%86%E3%82%B9%E3%83%88'",
    );
    assert_eq!(c.op_key, Some(op_key("deleteClient")));
    assert_eq!(path_param(&c, "clientId").unwrap().value, "テスト");
    // クエリの重複・空の値・値なし
    let c = parse("curl 'http://localhost:8080/api/v1/video/clients?q=a&q=b&q=&flag'");
    assert_eq!(c.op_key, Some(op_key("getClientList")));
    let qs: Vec<(&str, &str)> = c
        .request
        .params
        .iter()
        .filter(|p| p.location == ParamIn::Query && p.enabled)
        .map(|p| (p.name.as_str(), p.value.as_str()))
        .collect();
    assert_eq!(qs, [("q", "a"), ("q", "b"), ("q", ""), ("flag", "")]);
}

#[tokio::test]
async fn percent_encoded_path_values_are_sent_back_as_the_same_path() {
    let f = fx();
    let project = import_yaml(&f, SMALL_SPEC).await;
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    set_base_url(&f, &project, "http://localhost:8080");
    for raw in [
        "a%2Fb%20c",
        "%E3%83%86%E3%82%B9%E3%83%88",
        "100%25",
        "x%3Fy%23z",
    ] {
        let c = f
            .state
            .curl_parse(
                &project.id,
                &format!("curl http://localhost:8080/items/{raw}"),
                None,
            )
            .unwrap();
        assert_eq!(c.op_key, Some(op_key("getItem")), "{raw}");
        server.reset().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let r = send(
            &f.state,
            RequestInput {
                project_id: project.id.clone(),
                environment_id: None,
                request: c.request,
                overrides: vec![nekoreq_lib::send::VarOverride {
                    name: "baseUrl".into(),
                    value: server.uri(),
                }],
                op_key: c.op_key,
            },
        )
        .await;
        assert_eq!(response_of(&r).status, 200, "{raw}");
        let seen = server.received_requests().await.unwrap();
        assert_eq!(seen.len(), 1);
        // サーバーが受け取った path（デコードされる前）が、貼った cURL と同じ
        assert_eq!(seen[0].url.path(), format!("/items/{raw}"), "{raw}");
    }
}

#[tokio::test]
async fn base_url_with_a_path_matches_on_segment_boundaries_and_sends_to_the_right_place() {
    let f = fx();
    let project = import_yaml(&f, SMALL_SPEC).await;
    set_base_url(&f, &project, "http://localhost:8080/api/v1");
    let parse = |t: &str| f.state.curl_parse(&project.id, t, None).unwrap();

    let c = parse("curl http://localhost:8080/api/v1/items/5");
    assert!(c.base_replaced);
    assert_eq!(c.op_key, Some(op_key("getItem")));
    assert_eq!(c.request.url, "{{baseUrl}}/items/{id}");
    assert_eq!(path_param(&c, "id").unwrap().value, "5");
    // ちょうど baseUrl の path だけ
    let c = parse("curl http://localhost:8080/api/v1/items");
    assert_eq!(c.op_key, Some(op_key("listItems")));
    let c = parse("curl http://localhost:8080/api/v1/");
    assert!(c.base_replaced);
    assert_eq!(c.target, CurlTarget::Adhoc);
    // 区切りの途中（/api/v1x）や、baseUrl の path の外は照合しない
    for url in [
        "http://localhost:8080/api/v1x/items/5",
        "http://localhost:8080/items/5",
        "http://localhost:8080/api/items/5",
    ] {
        let c = parse(&format!("curl {url}"));
        assert!(!c.base_replaced, "{url}");
        assert_eq!(c.target, CurlTarget::Adhoc, "{url}");
        assert_eq!(c.request.url, url);
    }

    // そのまま送ると baseUrl の path のあとに付く
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let c = parse("curl 'http://localhost:8080/api/v1/items/5?x=1'");
    let r = send(
        &f.state,
        RequestInput {
            project_id: project.id.clone(),
            environment_id: None,
            request: c.request,
            overrides: vec![nekoreq_lib::send::VarOverride {
                name: "baseUrl".into(),
                value: format!("{}/api/v1", server.uri()),
            }],
            op_key: c.op_key,
        },
    )
    .await;
    assert_eq!(response_of(&r).status, 200);
    let seen = server.received_requests().await.unwrap();
    assert_eq!(seen[0].url.path(), "/api/v1/items/5");
    assert_eq!(seen[0].url.query(), Some("x=1"));
}

#[tokio::test]
async fn duplicate_query_keys_survive_import_send_and_export() {
    let f = fx();
    let project = import_yaml(&f, SMALL_SPEC).await;
    set_base_url(&f, &project, "http://localhost:8080");
    let c = f
        .state
        .curl_parse(
            &project.id,
            "curl 'http://localhost:8080/items?tag=a&tag=b&tag=a&tag=%E3%81%AD'",
            None,
        )
        .unwrap();
    assert_eq!(c.op_key, Some(op_key("listItems")));
    let tags: Vec<&str> = c
        .request
        .params
        .iter()
        .filter(|p| p.location == ParamIn::Query && p.name == "tag" && p.enabled)
        .map(|p| p.value.as_str())
        .collect();
    assert_eq!(tags, ["a", "b", "a", "ね"]);

    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let input = RequestInput {
        project_id: project.id.clone(),
        environment_id: None,
        request: c.request,
        overrides: vec![nekoreq_lib::send::VarOverride {
            name: "baseUrl".into(),
            value: server.uri(),
        }],
        op_key: c.op_key,
    };
    let r = send(&f.state, input.clone()).await;
    assert_eq!(response_of(&r).status, 200);
    let seen = server.received_requests().await.unwrap();
    assert_eq!(
        seen[0].url.query(),
        Some("tag=a&tag=b&tag=a&tag=%E3%81%AD"),
        "順序と重複を保つ"
    );
    // 書き出し（raw）も同じ
    let curl = f.state.curl_export(&input, CurlExportMode::Raw).unwrap();
    assert!(curl.contains("tag=a&tag=b&tag=a&tag=%E3%81%AD"), "{curl}");
}

#[tokio::test]
async fn curl_export_redacts_secrets_everywhere_and_raw_keeps_them() {
    // 引用符・$・空白・非 ASCII・改行を含むシークレット
    const PW: &str = "pw'$HOME \"q\" é\n2";
    const KEY: &str = "KEY-ABC-123";
    const TOK: &str = "TOK-xyz-789";
    let f = fx();
    let server = MockServer::start().await;
    let (project, _env) =
        project_with_env(&f, &server.uri(), &[("pw", PW), ("key", KEY), ("tok", TOK)]);
    let files = f.dir.path().join("up");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join(format!("{KEY}.bin")), "x").unwrap();

    let mut input = adhoc(
        &project,
        "POST",
        &format!(
            "{}/items/{{{{key}}}}?k={{{{key}}}}&p={{{{pw}}}}",
            "{{baseUrl}}"
        ),
    );
    input.request.params = vec![
        kv(ParamIn::Header, "X-Api-Key", "{{key}}"),
        kv(ParamIn::Header, "X-Plain", "visible"),
        kv(ParamIn::Cookie, "c", "{{tok}}"),
    ];
    input.request.auth = Some(AuthOverride::Bearer {
        token: "{{tok}}".into(),
    });
    input.request.body = Body::Multipart(vec![
        MultipartPart {
            name: "pw".into(),
            value: MultipartValue::Text("{{pw}}".into()),
            enabled: true,
            content_type: None,
        },
        MultipartPart {
            name: "file".into(),
            value: MultipartValue::File(format!("{}/{{{{key}}}}.bin", files.display())),
            enabled: true,
            content_type: None,
        },
    ]);

    let redacted = f
        .state
        .curl_export(&input, CurlExportMode::Redacted)
        .unwrap();
    for s in [KEY, TOK, "pw'", "HOME", "é\n2", "é"] {
        // PW は断片で確かめる（引用符の書き換えで途中が変わる）
        assert!(
            !redacted.contains(s),
            "redacted に {s:?} が残っている:\n{redacted}"
        );
    }
    assert!(
        redacted.contains("visible") && redacted.contains("••••"),
        "{redacted}"
    );

    let raw = f.state.curl_export(&input, CurlExportMode::Raw).unwrap();
    for s in [KEY, TOK] {
        assert!(raw.contains(s), "raw に {s} が出ない:\n{raw}");
    }
    assert!(raw.contains("Authorization: Bearer TOK-xyz-789"), "{raw}");
    // 書き出した cURL を読み直すと、ほぼ同じ内容（method・host・path）
    let parsed = nekoreq_curl::parse(&raw).unwrap();
    assert_eq!(parsed.method, "POST");
    assert!(parsed.path.contains(KEY), "{}", parsed.path);
    // 履歴からのコピー（伏せ字済み）にも出ない
    send(&f.state, input.clone()).await;
    let h = latest(&f, &project);
    let from_history = f.state.history_curl(&h.id).unwrap();
    for s in [KEY, TOK, "pw'"] {
        assert!(
            !from_history.contains(s),
            "履歴の cURL に {s} が出ている:\n{from_history}"
        );
    }
}

// =====================================================================================
// 5. 設定と送信
// =====================================================================================

fn set_settings(f: &Fx, edit: impl FnOnce(&mut AppSettings)) -> AppSettings {
    let mut s = f.state.app_settings_get().unwrap();
    edit(&mut s);
    f.state.app_settings_set(&s).unwrap()
}

fn settings_error(f: &Fx, edit: impl FnOnce(&mut AppSettings)) -> String {
    let mut s = AppSettings::default();
    edit(&mut s);
    let e = f.state.app_settings_set(&s).unwrap_err();
    assert!(matches!(e, AppError::Validation(_)), "{e:?}");
    e.to_string()
}

#[test]
fn settings_validation_matrix_and_messages_never_echo_passwords() {
    let f = fx();
    // タイムアウトの境界
    for ok in [0u32, 1, 86_400] {
        set_settings(&f, |s| s.request.timeout_secs = ok);
    }
    for bad in [86_401u32, u32::MAX] {
        let m = settings_error(&f, |s| s.request.timeout_secs = bad);
        assert!(m.contains("タイムアウト"), "{m}");
    }
    // 手動プロキシ
    let manual = |url: &str| {
        let url = url.to_string();
        move |s: &mut AppSettings| {
            s.network.proxy.mode = ProxyMode::Manual;
            s.network.proxy.url = url.clone();
        }
    };
    for bad in [
        "",
        "   ",
        "ftp://proxy:21",
        "socks5://proxy:1080",
        "http://",
        "localhost:3128",
        "proxy.internal:3128",
        "not a url",
        "http://proxy:99999",
        "file:///tmp/x",
    ] {
        let m = settings_error(&f, manual(bad));
        assert!(m.contains("プロキシ"), "{bad:?}: {m}");
    }
    // ユーザー情報つきはメッセージにパスワードを出さない（パースできない形・できる形の両方）
    for bad in [
        "http://u:LEAKPW-1@proxy:99999",
        "http://u:LEAKPW-2@proxy:8080",
        "http://:LEAKPW-3@proxy:8080",
        "https://LEAKPW-4@proxy:8080",
    ] {
        let m = settings_error(&f, manual(bad));
        assert!(!m.contains("LEAKPW"), "{bad}: {m}");
    }
    // 手動でなければ URL は検証しない。前後の空白は除く
    let s = set_settings(&f, |s| {
        s.network.proxy.mode = ProxyMode::System;
        s.network.proxy.url = "  garbage  ".into();
        s.network.proxy.no_proxy = vec![
            "localhost, .internal.example\n10.0.0.0/8".into(),
            " ".into(),
        ];
    });
    assert_eq!(s.network.proxy.url, "garbage");
    assert_eq!(
        s.network.proxy.no_proxy,
        ["localhost", ".internal.example", "10.0.0.0/8"]
    );
    // CA 証明書: 無い・ディレクトリ・空・壊れた PEM・秘密鍵だけの PEM
    let dir = f.dir.path().join("certs");
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.pem");
    std::fs::write(&empty, "").unwrap();
    let broken = dir.join("broken.pem");
    std::fs::write(
        &broken,
        "-----BEGIN CERTIFICATE-----\n!!!not-base64!!!\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    let keyonly = dir.join("key.pem");
    std::fs::write(
        &keyonly,
        "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
    )
    .unwrap();
    for p in [
        "/no/such/ca.pem".to_string(),
        dir.display().to_string(),
        empty.display().to_string(),
        broken.display().to_string(),
        keyonly.display().to_string(),
    ] {
        let m = settings_error(&f, |s| s.network.ca_cert_path = Some(p.clone()));
        assert!(m.contains("CA 証明書"), "{p}: {m}");
    }
    // クライアント証明書: 無い・ディレクトリ・ホストが空・ファイルが空・ID の重複
    let cc = |host: &str, path: &str, id: &str| ClientCertSetting {
        id: id.into(),
        host_pattern: host.into(),
        path: path.into(),
        has_password: false,
    };
    let file = dir.join("c.pem");
    std::fs::write(&file, "x").unwrap();
    let fp = file.display().to_string();
    for (certs, want) in [
        (vec![cc("h", "/no/such.p12", "")], "見つかりません"),
        (
            vec![cc("h", &dir.display().to_string(), "")],
            "見つかりません",
        ),
        (vec![cc("", &fp, "")], "ホストのパターン"),
        (vec![cc("h", "", "")], "ファイルを選んで"),
        (vec![cc("a", &fp, "same"), cc("b", &fp, "same")], "重なって"),
    ] {
        let m = settings_error(&f, |s| s.network.client_certs = certs.clone());
        assert!(m.contains(want), "{want}: {m}");
    }
    // 保存に失敗しても、前の設定が残る
    assert_eq!(
        f.state.app_settings_get().unwrap().network.proxy.url,
        "garbage"
    );
}

/// 保存された設定の JSON が一部でも読めないと、設定全体（プロキシ・証明書を含む）が黙って既定に戻る。
#[test]
fn one_bad_field_in_the_stored_settings_does_not_reset_the_valid_ones() {
    let f = fx();
    let conn = raw_connection(&f.data);
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES ('app', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [r#"{"request":{"timeoutSecs":-5},"network":{"proxy":{"mode":"manual","url":"http://p:3128"}},"history":{"limit":5000}}"#],
    )
    .unwrap();
    let s = f.state.app_settings_get().unwrap();
    assert_eq!(s.network.proxy.mode, ProxyMode::Manual, "{s:?}");
    assert_eq!(s.history.limit, 5000);
}

#[test]
fn broken_stored_settings_fall_back_to_defaults_without_failing_the_app() {
    let f = fx();
    let conn = raw_connection(&f.data);
    for json in [
        "",
        "not json",
        "[]",
        "null",
        "{\"request\":{\"timeoutSecs\":\"x\"}}",
    ] {
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('app', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [json],
        )
        .unwrap();
        assert_eq!(
            f.state.app_settings_get().unwrap(),
            AppSettings::default(),
            "{json:?}"
        );
    }
}

fn make_p12(password: &str) -> Vec<u8> {
    use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKey, PrivateKeyChain};
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let chain = PrivateKeyChain::new(
        b"id".to_vec(),
        PrivateKey::from_der(&ck.signing_key.serialize_der()).unwrap(),
        vec![Certificate::from_der(ck.cert.der()).unwrap()],
    );
    let mut store = KeyStore::new();
    store.add_entry("client", KeyStoreEntry::PrivateKeyChain(chain));
    store.writer(password).write().unwrap()
}

#[tokio::test]
async fn broken_client_certificates_fail_the_send_in_japanese_without_passwords_and_are_not_recorded(
) {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[]);
    let dir = f.dir.path().join("cc");
    std::fs::create_dir_all(&dir).unwrap();
    let p12 = make_p12("RIGHT-PW-77");
    let mut truncated = p12.clone();
    truncated.truncate(p12.len() / 2);
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("garbage.p12", b"this is not a p12".to_vec(), ""),
        ("empty.pfx", vec![], ""),
        ("truncated.p12", truncated, "RIGHT-PW-77"),
        ("empty.pem", vec![], ""),
        (
            "cert-only.pem",
            b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_vec(),
            "",
        ),
        ("wrong-password.p12", p12.clone(), "WRONG-PW-88"),
        // パスワードを設定していない（空で開く）
        ("needs-password.p12", p12.clone(), ""),
    ];
    for (name, bytes, password) in cases {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let s = set_settings(&f, |s| {
            s.network.client_certs = vec![ClientCertSetting {
                id: "cert1".into(),
                host_pattern: "127.0.0.1".into(),
                path: path.display().to_string(),
                has_password: false,
            }]
        });
        assert_eq!(s.network.client_certs[0].id, "cert1");
        if !password.is_empty() {
            f.state.client_cert_password_set("cert1", password).unwrap();
        } else {
            f.state.client_cert_password_set("cert1", "").unwrap();
        }
        let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/x")).await;
        let e = error_of(&r);
        let m = e.message();
        assert!(m.contains("クライアント証明書"), "{name}: {m}");
        assert!(
            !m.contains("RIGHT-PW-77") && !m.contains("WRONG-PW-88"),
            "{name}: {m}"
        );
    }
    // 送っていないので履歴に残らない
    assert!(all_history(&f, &project).is_empty());
    assert_not_on_disk(&f.data, &["RIGHT-PW-77", "WRONG-PW-88"]);

    // ホストのパターンに当てはまらない壊れた証明書は、読まない（送れる）
    set_settings(&f, |s| {
        s.network.client_certs[0].host_pattern = "*.internal.example".into()
    });
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/x")).await;
    assert_eq!(response_of(&r).status, 200);
    // 正しいパスワードなら読める（http なので証明書は使われないが、送れる）
    let good = dir.join("good.p12");
    std::fs::write(&good, &p12).unwrap();
    set_settings(&f, |s| {
        s.network.client_certs[0].host_pattern = "127.0.0.1".into();
        s.network.client_certs[0].path = good.display().to_string();
    });
    f.state
        .client_cert_password_set("cert1", "RIGHT-PW-77")
        .unwrap();
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/x")).await;
    assert_eq!(response_of(&r).status, 200);
}

#[tokio::test]
async fn an_invalid_stored_proxy_url_fails_the_send_without_leaking_the_password() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[]);
    // 画面からは保存できない値（設定の JSON を直接書き換えた場合）
    let conn = raw_connection(&f.data);
    for url in [
        "http://u:LEAKPW-9@[bad",
        "http://",
        "::::",
        "http://u:LEAKPW-9@exa mple.com:8080",
    ] {
        let json =
            serde_json::json!({"network": {"proxy": {"mode": "manual", "url": url}}}).to_string();
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('app', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [json],
        )
        .unwrap();
        let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/x")).await;
        let m = error_of(&r).message().to_string();
        assert!(!m.contains("LEAKPW"), "{url}: {m}");
    }
    // 履歴にも出ない（設定の JSON は、このテストが直接書いたので SQLite には当然ある）
    for e in all_history(&f, &project) {
        assert!(!dump(&e).contains("LEAKPW"), "{}", dump(&e));
    }
}

#[tokio::test]
async fn proxy_credentials_with_special_characters_are_sent_as_utf8_basic() {
    let f = fx();
    let (project, _env) = project_with_env(&f, "http://target.invalid:8123", &[]);
    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("via-proxy"))
        .mount(&proxy)
        .await;
    const PW: &str = "p@ss:w/rd é+%";
    set_settings(&f, |s| {
        s.network.proxy.mode = ProxyMode::Manual;
        s.network.proxy.url = proxy.uri();
        s.network.proxy.username = "néko".into();
    });
    f.state.proxy_password_set(PW).unwrap();
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/api")).await;
    assert_eq!(response_of(&r).body.content, "via-proxy");
    let seen = &proxy.received_requests().await.unwrap()[0];
    let auth = seen
        .headers
        .get("proxy-authorization")
        .unwrap()
        .to_str()
        .unwrap();
    let want = base64::engine::general_purpose::STANDARD.encode(format!("néko:{PW}"));
    assert_eq!(auth, format!("Basic {want}"));
    // 履歴にも SQLite にも残らない（Basic の base64 も）
    assert_clean(&f, &project, &[PW, &want]);
}

#[tokio::test]
async fn display_limit_boundaries_are_exact_and_multibyte_safe() {
    const MB10: usize = 10 * 1024 * 1024;
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            let body = match req.url.path() {
                "/exact" => "a".repeat(MB10),
                "/plus1" => "a".repeat(MB10 + 1),
                // 切り口（10 MB）が 2 バイト文字の真ん中
                _ => format!("{}é{}", "a".repeat(MB10 - 1), "b".repeat(10)),
            };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain; charset=utf-8")
                .set_body_string(body)
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[]);
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/exact")).await;
    let b = &response_of(&r).body;
    assert!(!b.truncated);
    assert_eq!((b.content.len(), b.size as usize), (MB10, MB10));
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/plus1")).await;
    let b = &response_of(&r).body;
    assert!(b.truncated);
    assert_eq!((b.content.len(), b.size as usize), (MB10, MB10 + 1));
    let r = send(&f.state, adhoc(&project, "GET", "{{baseUrl}}/multibyte")).await;
    let b = &response_of(&r).body;
    assert!(b.truncated);
    assert!(
        b.content.len() <= MB10 && b.content.len() >= MB10 - 1,
        "{}",
        b.content.len()
    );
    // 履歴は 1 MB で切る（切り口でパニックしない）
    let e = latest(&f, &project);
    assert!(e.response_body_truncated);
    assert!(e.response_body.unwrap().len() <= 1024 * 1024);
}

// =====================================================================================
// 6. タブの復元
// =====================================================================================

fn tab(id: &str, kind: SessionTabKind, request: Option<RequestSpec>) -> SessionTab {
    SessionTab {
        id: id.into(),
        kind,
        op_key: None,
        adhoc_id: None,
        title: format!("タブ {id}"),
        request,
        dirty: true,
        view_state: None,
        dropped_fields: vec![],
    }
}

fn spec_with(params: Vec<KeyValue>, auth: Option<AuthOverride>) -> RequestSpec {
    RequestSpec {
        method: "POST".into(),
        url: "{{baseUrl}}/x".into(),
        params,
        body: Body::None,
        auth,
    }
}

fn disabled(mut kv: KeyValue) -> KeyValue {
    kv.enabled = false;
    kv
}

#[test]
fn session_load_never_fails_on_broken_or_old_stored_data() {
    let f = fx();
    let project = f.state.project_create("p").unwrap();
    let conn = raw_connection(&f.data);
    let deep = "[".repeat(200_000);
    let cases: Vec<(&str, String)> = vec![
        ("空文字", String::new()),
        ("JSON ではない", "not json".into()),
        ("null", "null".into()),
        ("配列", "[]".into()),
        ("空のオブジェクト", "{}".into()),
        ("tabs が文字列", r#"{"tabs":"x"}"#.into()),
        ("必須の項目がないタブ", r#"{"tabs":[{"id":"1"}]}"#.into()),
        (
            "知らない kind（新しい版で保存したデータ）",
            r#"{"tabs":[{"id":"t","kind":"future","title":"x","dirty":false}]}"#.into(),
        ),
        (
            "activeTabId が数値",
            r#"{"tabs":[],"activeTabId":5}"#.into(),
        ),
        ("BOM つき", "\u{feff}{\"tabs\":[]}".into()),
        ("深すぎる入れ子", deep),
        (
            "知らない項目つき（新しい版）",
            r#"{"tabs":[],"version":2,"extra":{"a":1}}"#.into(),
        ),
    ];
    for (name, json) in cases {
        conn.execute(
            "INSERT INTO sessions (project_id, json, updated_at) VALUES (?1, ?2, 'x')
             ON CONFLICT(project_id) DO UPDATE SET json = excluded.json",
            rusqlite::params![project.id, json],
        )
        .unwrap();
        let loaded = f.state.session_load(&project.id);
        assert!(loaded.is_ok(), "{name}: {loaded:?}");
    }
    // 保存し直せば、また読める
    let saved = f
        .state
        .session_save(&project.id, &SessionState::default())
        .unwrap();
    assert!(saved.saved_at.is_some());
    assert!(f.state.session_load(&project.id).unwrap().is_some());
    // 読めないタブ（知らない kind など）があっても、読めるタブは復元する（読めなかった数は標準エラーに記録する）
    conn.execute(
        "UPDATE sessions SET json = ?1 WHERE project_id = ?2",
        rusqlite::params![
            r#"{"tabs":[{"id":"ok","kind":"new","title":"t","dirty":false,"request":null},{"id":"t","kind":"future","title":"x","dirty":false}]}"#,
            project.id
        ],
    )
    .unwrap();
    let loaded = f.state.session_load(&project.id).unwrap().unwrap();
    let ids: Vec<&str> = loaded.tabs.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["ok"]);
}

#[test]
fn hand_written_auth_in_unusual_forms_is_not_saved() {
    let f = fx();
    let project = f.state.project_create("p").unwrap();
    let secrets = [
        "SESS-S1", "SESS-S2", "SESS-S3", "SESS-S4", "SESS-S5", "SESS-S6", "SESS-S7", "SESS-S8",
        "SESS-S9", "SESS-S10",
    ];
    let tabs = vec![
        // 大文字小文字・空白・無効の行・Proxy-Authorization・複数の Cookie
        tab(
            "a",
            SessionTabKind::New,
            Some(spec_with(
                vec![
                    kv(ParamIn::Header, "authorization", "Bearer SESS-S1"),
                    kv(ParamIn::Header, " X-Xsrf-Token ", "SESS-S2"),
                    kv(ParamIn::Header, "Proxy-Authorization", "Basic SESS-S3"),
                    kv(ParamIn::Header, "Cookie", "a=SESS-S4; b=SESS-S5"),
                    disabled(kv(ParamIn::Cookie, "c", "SESS-S6")),
                    disabled(kv(ParamIn::Header, "Authorization", "Bearer SESS-S7")),
                    // テンプレートは残る
                    kv(ParamIn::Header, "Authorization", "Bearer {{tok}}"),
                    kv(ParamIn::Header, "X-Keep", "visible"),
                ],
                Some(AuthOverride::ApiKey {
                    location: nekoreq_core::ApiKeyIn::Cookie,
                    name: "k".into(),
                    value: "SESS-S8".into(),
                }),
            )),
        ),
        tab(
            "b",
            SessionTabKind::New,
            Some(spec_with(
                vec![],
                Some(AuthOverride::Basic {
                    username: "user-keep".into(),
                    password: "SESS-S9".into(),
                }),
            )),
        ),
        tab(
            "c",
            SessionTabKind::New,
            Some(spec_with(
                vec![],
                Some(AuthOverride::Bearer {
                    token: "  SESS-S10  ".into(),
                }),
            )),
        ),
    ];
    let saved = f
        .state
        .session_save(
            &project.id,
            &SessionState {
                tabs,
                active_tab_id: Some("a".into()),
                saved_at: None,
            },
        )
        .unwrap();
    let loaded = f.state.session_load(&project.id).unwrap().unwrap();
    assert_eq!(saved.tabs, loaded.tabs);
    let json = serde_json::to_string(&loaded).unwrap();
    for s in secrets {
        assert!(!json.contains(s), "復元されるデータに {s}: {json}");
    }
    assert_not_on_disk(&f.data, &secrets);
    // 残るもの
    assert!(json.contains("visible") && json.contains("{{tok}}") && json.contains("user-keep"));
    assert_eq!(
        loaded.tabs[0].dropped_fields.len(),
        7,
        "{:?}",
        loaded.tabs[0].dropped_fields
    );
    assert_eq!(
        loaded.tabs[1].dropped_fields,
        ["認証（Basic のパスワード）"]
    );
    assert_eq!(loaded.tabs[2].dropped_fields, ["認証（Bearer のトークン）"]);
}

/// 読み込んだタブ（`droppedFields` つき）をそのまま保存し直すと、`droppedFields` が空になり、
/// 次の起動では「認証の値は復元されません」の知らせが出ないまま、認証が空のタブが開く。
#[test]
fn dropped_fields_survive_a_load_and_save_round_trip() {
    let f = fx();
    let project = f.state.project_create("p").unwrap();
    let t = tab(
        "a",
        SessionTabKind::New,
        Some(spec_with(
            vec![],
            Some(AuthOverride::Bearer {
                token: "SESS-ROUND".into(),
            }),
        )),
    );
    f.state
        .session_save(
            &project.id,
            &SessionState {
                tabs: vec![t],
                active_tab_id: Some("a".into()),
                saved_at: None,
            },
        )
        .unwrap();
    let loaded = f.state.session_load(&project.id).unwrap().unwrap();
    assert_eq!(loaded.tabs[0].dropped_fields, ["認証（Bearer のトークン）"]);
    // 画面は、読み込んだ状態のまま（認証の値は空のまま）保存する
    f.state.session_save(&project.id, &loaded).unwrap();
    let again = f.state.session_load(&project.id).unwrap().unwrap();
    assert_eq!(
        again.tabs[0].dropped_fields,
        ["認証（Bearer のトークン）"],
        "2 回目の起動でも、認証の値が空になっていることを知らせる"
    );
}

#[test]
fn sessions_are_per_project_survive_reopen_and_go_with_the_project() {
    let f = fx();
    let a = f.state.project_create("a").unwrap();
    let b = f.state.project_create("b").unwrap();
    let state_a = SessionState {
        tabs: vec![tab(
            "a1",
            SessionTabKind::New,
            Some(spec_with(vec![], None)),
        )],
        active_tab_id: Some("a1".into()),
        saved_at: None,
    };
    f.state.session_save(&a.id, &state_a).unwrap();
    assert!(f.state.session_load(&b.id).unwrap().is_none());
    assert!(matches!(
        f.state.session_save("no-such", &state_a).unwrap_err(),
        AppError::NotFound(_)
    ));
    assert!(f.state.session_load("no-such").unwrap().is_none());
    // 開き直し
    let Fx {
        dir,
        data,
        state,
        secrets,
    } = f;
    drop(state);
    let reopened = AppState::open_with_secrets(&data, secrets.clone()).unwrap();
    let loaded = reopened.session_load(&a.id).unwrap().unwrap();
    assert_eq!(loaded.tabs.len(), 1);
    assert_eq!(loaded.active_tab_id.as_deref(), Some("a1"));
    // プロジェクトを消すと一緒に消える
    reopened.project_delete(&a.id).unwrap();
    assert!(reopened.session_load(&a.id).unwrap().is_none());
    let n: i64 = raw_connection(&data)
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    drop(dir);
}

#[test]
fn two_hundred_large_tabs_save_and_load_in_reasonable_time() {
    let f = fx();
    let project = f.state.project_create("p").unwrap();
    let mut tabs = Vec::new();
    for i in 0..200 {
        let mut spec = spec_with(vec![kv(ParamIn::Query, "q", &i.to_string())], None);
        spec.body = Body::Json(format!("{{\"data\":\"{}\"}}", "x".repeat(256 * 1024)));
        tabs.push(tab(&format!("t{i}"), SessionTabKind::New, Some(spec)));
    }
    let state = SessionState {
        tabs,
        active_tab_id: Some("t0".into()),
        saved_at: None,
    };
    let t = std::time::Instant::now();
    f.state.session_save(&project.id, &state).unwrap();
    let save = t.elapsed();
    let t = std::time::Instant::now();
    let loaded = f.state.session_load(&project.id).unwrap().unwrap();
    let load = t.elapsed();
    eprintln!("200 タブ × 256 KB: 保存 {save:?} / 読み込み {load:?}");
    assert_eq!(loaded.tabs.len(), 200);
    // 起動 2 秒以内の予算（デバッグビルドなので 4 倍の余裕を見る）
    assert!(load < Duration::from_secs(8), "読み込み {load:?}");
    assert!(save < Duration::from_secs(8), "保存 {save:?}");
    // 201 個は断る
    let mut too_many = loaded.clone();
    too_many.tabs.push(too_many.tabs[0].clone());
    assert!(matches!(
        f.state.session_save(&project.id, &too_many).unwrap_err(),
        AppError::Validation(_)
    ));
    // 断られたら前の状態が残る
    assert_eq!(
        f.state
            .session_load(&project.id)
            .unwrap()
            .unwrap()
            .tabs
            .len(),
        200
    );
}

// =====================================================================================
// 7. 自動更新
// =====================================================================================

fn tauri_conf() -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn updater_endpoint_has_no_credentials_and_the_webview_cannot_call_the_updater() {
    let conf = tauri_conf();
    for e in conf["plugins"]["updater"]["endpoints"].as_array().unwrap() {
        let u = url::Url::parse(e.as_str().unwrap()).unwrap();
        assert_eq!(u.scheme(), "https");
        // エラー文（reqwest）に URL が出ても、トークンが入らないように、URL にユーザー情報・クエリを持たせない
        assert!(u.username().is_empty() && u.password().is_none(), "{u}");
        assert!(u.query().is_none() && u.fragment().is_none(), "{u}");
    }
    // 画面（WebView）に updater の権限を渡していない
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities");
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!text.contains("updater"), "capabilities に updater がある");
        assert!(!text.contains("process:"), "capabilities に process がある");
        assert!(!text.contains("shell:"), "capabilities に shell がある");
        assert!(!text.contains("fs:"), "capabilities に fs がある");
    }
    // CSP が外部への接続を許していない
    let csp = conf["app"]["security"]["csp"].as_str().unwrap();
    assert!(!csp.contains("unsafe-eval"), "{csp}");
    assert!(
        !csp.contains("https:") && !csp.contains("http://*"),
        "{csp}"
    );
}

/// リリースの前に置き換える値（pubkey = REPLACE_WITH_UPDATER_PUBLIC_KEY、endpoint = OWNER/REPO）が残っていないか。
#[test]
fn updater_config_has_no_placeholders_left() {
    let conf = tauri_conf();
    let updater = &conf["plugins"]["updater"];
    let pubkey = updater["pubkey"].as_str().unwrap();
    assert!(!pubkey.contains("REPLACE"), "{pubkey}");
    // minisign の公開鍵は base64（2 行のテキストを base64 にしたもの）
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(pubkey)
        .expect("pubkey は base64");
    assert!(String::from_utf8_lossy(&decoded).contains("minisign public key"));
    for e in updater["endpoints"].as_array().unwrap() {
        let e = e.as_str().unwrap();
        assert!(!e.contains("OWNER") && !e.contains("REPO"), "{e}");
    }
}

// =====================================================================================
// 8. マイグレーション（v1 から順に上げた DB で全機能が動く）
// =====================================================================================

/// `crates/storage/src/migrations.rs` の `const V1..Vn: &str = r#"..."#;` を取り出す。
fn migration_sql() -> Vec<String> {
    let src = include_str!("../../crates/storage/src/migrations.rs");
    let mut out = Vec::new();
    for n in 1.. {
        let head = format!("const V{n}: &str = r#\"");
        let Some(start) = src.find(&head) else { break };
        let rest = &src[start + head.len()..];
        let end = rest.find("\"#;").expect("raw string の終わり");
        out.push(rest[..end].to_string());
    }
    assert!(out.len() >= 6, "{}", out.len());
    out
}

/// 版 `version` の DB を作り、その版のスキーマで古いデータを入れる。
fn old_db(path: &Path, version: usize, base_url: &str) {
    let sql = migration_sql();
    let conn = rusqlite::Connection::open(path).unwrap();
    for v in 1..=version {
        conn.execute_batch(&sql[v - 1]).unwrap();
    }
    conn.execute_batch(&format!("PRAGMA user_version = {version};"))
        .unwrap();
    let ts = "2026-01-01T00:00:00.000Z";
    if version >= 5 {
        conn.execute(
            "INSERT INTO projects (id, name, default_environment_id, created_at, updated_at, settings_json)
             VALUES ('p1', '旧プロジェクト', 'e1', ?1, ?1, '{\"autoCsrf\":false}')",
            [ts],
        )
        .unwrap();
    } else {
        conn.execute(
            "INSERT INTO projects (id, name, default_environment_id, created_at, updated_at)
             VALUES ('p1', '旧プロジェクト', 'e1', ?1, ?1)",
            [ts],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at)
         VALUES ('e1', 'p1', '旧ローカル', 1, ?1, ?1)",
        [ts],
    )
    .unwrap();
    if version >= 2 {
        // v2 以降の DB では、アプリがプロジェクトを作るときにグローバル環境も作っていた
        conn.execute(
            "INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at, is_global)
             VALUES ('g1', 'p1', 'グローバル', 0, ?1, ?1, 1)",
            [ts],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO env_vars (environment_id, name, kind, value, description, sort_order)
         VALUES ('e1', 'baseUrl', 'plain', ?1, '', 0), ('e1', 'session', 'secret', NULL, '旧', 1)",
        [base_url],
    )
    .unwrap();
    let adhoc_sql = if version >= 3 {
        "INSERT INTO adhoc_requests (id, project_id, name, method, url, params_json, body_json, auth_json, created_at, updated_at, description)
         VALUES ('a1', 'p1', '旧 adhoc', 'GET', '{{baseUrl}}/old', '[]', '{\"kind\":\"none\"}', NULL, ?1, ?1, '旧の説明')"
    } else {
        "INSERT INTO adhoc_requests (id, project_id, name, method, url, params_json, body_json, auth_json, created_at, updated_at)
         VALUES ('a1', 'p1', '旧 adhoc', 'GET', '{{baseUrl}}/old', '[]', '{\"kind\":\"none\"}', NULL, ?1, ?1)"
    };
    conn.execute(adhoc_sql, [ts]).unwrap();
    conn.execute(
        "INSERT INTO request_drafts (id, project_id, op_key, params_json, body_json, auth_json, updated_at)
         VALUES ('d1', 'p1', 'id:oldOp', '[]', '{\"kind\":\"json\",\"value\":\"{\\\"old\\\":1}\"}', NULL, ?1)",
        [ts],
    )
    .unwrap();
    if version <= 5 {
        // v1〜v5 の history（書き込む処理はなかったが、表はあった）
        conn.execute(
            "INSERT INTO history (id, project_id, sent_at, method, url) VALUES ('h-old', 'p1', ?1, 'GET', 'http://old.invalid/')",
            [ts],
        )
        .unwrap();
    }
}

#[tokio::test]
async fn every_old_schema_version_migrates_and_all_features_work() {
    for version in 1..=migration_sql().len() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(|req: &Request| {
                let cookie = req
                    .headers
                    .get("cookie")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default();
                ResponseTemplate::new(200)
                    .append_header("set-cookie", "SESSION=NEW-AFTER-MIGRATION; Path=/")
                    .append_header("x-seen-cookie", cookie.as_str())
                    .set_body_string("ok")
            })
            .mount(&server)
            .await;
        old_db(&data.join("data.sqlite"), version, &server.uri());

        let secrets = Arc::new(MemorySecretStore::new());
        secrets
            .set(&SecretKey::new("p1", "e1", "session"), "OLD-SESSION")
            .unwrap();
        let state = AppState::open_with_secrets(&data, secrets.clone())
            .unwrap_or_else(|e| panic!("v{version} から開けない: {e}"));
        let ctx = format!("v{version} から");

        // バージョン・整合性
        let info = state.app_info("t", "t").unwrap();
        assert_eq!(info.schema_version as usize, migration_sql().len(), "{ctx}");
        {
            let conn = raw_connection(&data);
            let ok: String = conn
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .unwrap();
            assert_eq!(ok, "ok", "{ctx}");
            let fk: i64 = conn
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(fk, 0, "{ctx}");
        }

        // 古いデータが残っている
        let projects = state.project_list().unwrap();
        assert_eq!(projects.len(), 1, "{ctx}");
        let project = &projects[0];
        assert_eq!(project.name, "旧プロジェクト");
        assert_eq!(
            project.default_environment_id.as_deref(),
            Some("e1"),
            "{ctx}"
        );
        let envs = state.env_list("p1").unwrap();
        assert_eq!(
            envs.iter().filter(|e| e.is_global).count(),
            1,
            "{ctx}: グローバルは 1 つ"
        );
        let e1 = envs.iter().find(|e| e.id == "e1").unwrap();
        assert_eq!(e1.name, "旧ローカル");
        assert!(e1
            .vars
            .iter()
            .any(|v| v.name == "session" && v.value == VarValue::Secret));
        assert!(e1.vars.iter().all(|v| v.enabled), "{ctx}: 既存の変数は有効");
        let adhocs = state.adhoc_list("p1").unwrap();
        assert_eq!(adhocs.len(), 1, "{ctx}");
        assert_eq!(adhocs[0].name, "旧 adhoc");
        assert_eq!(
            adhocs[0].description,
            if version >= 3 { "旧の説明" } else { "" },
            "{ctx}"
        );
        let draft = state.draft_get("p1", &OpKey::Id("oldOp".into())).unwrap();
        assert!(draft.is_some(), "{ctx}: 入力値が残る");
        // v5 の設定は残り、それ以前はどちらもオン
        assert!(project.settings.auto_session, "{ctx}");
        assert_eq!(project.settings.auto_csrf, version < 5, "{ctx}");
        // 古い履歴の表は作り直される（行は消える）
        assert_eq!(
            state
                .history_list("p1", &HistoryQuery::default())
                .unwrap()
                .total,
            0,
            "{ctx}"
        );

        // 送信（シークレットの session が Cookie に付き、Set-Cookie で更新される）→ 履歴
        let r = send(&state, {
            RequestInput {
                project_id: "p1".into(),
                environment_id: None,
                request: RequestSpec {
                    method: "GET".into(),
                    url: "{{baseUrl}}/old".into(),
                    params: vec![],
                    body: Body::None,
                    auth: None,
                },
                overrides: vec![],
                op_key: None,
            }
        })
        .await;
        let resp = response_of(&r);
        let seen = resp
            .headers
            .iter()
            .find(|h| h.name == "x-seen-cookie")
            .map(|h| h.value.clone())
            .unwrap_or_default();
        assert!(seen.contains("SESSION=OLD-SESSION"), "{ctx}: {seen}");
        assert_eq!(
            secrets
                .get(&SecretKey::new("p1", "e1", "session"))
                .unwrap()
                .as_deref(),
            Some("NEW-AFTER-MIGRATION"),
            "{ctx}"
        );
        let page = state
            .history_list(
                "p1",
                &HistoryQuery {
                    text: Some("/old".into()),
                    statuses: vec![StatusClass::S2xx],
                    ..HistoryQuery::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 1, "{ctx}");
        assert!(
            state
                .history_curl(&page.items[0].id)
                .unwrap()
                .contains("/old"),
            "{ctx}"
        );
        // 設定・タブ・プロジェクトの設定
        let mut s = state.app_settings_get().unwrap();
        s.history.limit = 5000;
        state.app_settings_set(&s).unwrap();
        state
            .session_save(
                "p1",
                &SessionState {
                    tabs: vec![tab("t", SessionTabKind::New, Some(spec_with(vec![], None)))],
                    active_tab_id: Some("t".into()),
                    saved_at: None,
                },
            )
            .unwrap();
        assert!(state.session_load("p1").unwrap().is_some(), "{ctx}");
        state
            .project_settings_set("p1", &ProjectSettings::default())
            .unwrap();
        // 環境・グローバルも作れる
        state.env_create("p1", "新しい環境").unwrap();
        assert_eq!(state.history_clear("p1").unwrap(), 1, "{ctx}");
        state.project_delete("p1").unwrap();
        assert!(state.project_list().unwrap().is_empty(), "{ctx}");
        assert_not_on_disk(&data, &["OLD-SESSION", "NEW-AFTER-MIGRATION"]);
    }
}

// =====================================================================================
// 1'. 前後に空白・改行が付いたシークレット（コピー & ペーストでよくある）
// =====================================================================================

/// 値の前後に空白が付いたシークレット（`" TOKEN "`）。ヘッダーの値は HTTP で前後の空白が落ちるので、サーバーが
/// エコーするのは `TOKEN`（空白なし）。履歴の伏せ字は空白つきの文字列を探すため、エコーされた箇所が取り残される。
#[tokio::test]
async fn secret_with_surrounding_spaces_is_masked_when_the_server_echoes_the_trimmed_header() {
    const CORE: &str = "PAD-SECRET-9921";
    let padded = format!("  {CORE}  ");
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(echo_json(req).to_string())
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("k", padded.as_str())]);
    let mut input = adhoc(&project, "GET", "{{baseUrl}}/x");
    input.request.params = vec![kv(ParamIn::Header, "X-Key", "{{k}}")];
    let r = send(&f.state, input).await;
    assert_eq!(response_of(&r).status, 200);
    assert!(
        response_of(&r).body.content.contains(CORE),
        "サーバーはヘッダーを受け取ってエコーしたはず（前提）"
    );
    assert_clean(&f, &project, &[CORE]);
}

#[tokio::test]
async fn a_secret_ending_with_a_newline_fails_the_send_without_echoing_it() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("k", "NL-TOKEN-5150\n")]);
    for (loc_header, auth) in [(true, false), (false, true)] {
        let mut input = adhoc(&project, "GET", "{{baseUrl}}/x");
        if loc_header {
            input.request.params = vec![kv(ParamIn::Header, "X-Key", "{{k}}")];
        }
        if auth {
            input.request.auth = Some(AuthOverride::Bearer {
                token: "{{k}}".into(),
            });
        }
        let r = send(&f.state, input).await;
        // 送れない（改行を含むヘッダー値）。メッセージにシークレットを出さない
        if let SendOutcome::Failure { error } = &r.result {
            assert!(
                !error.message().contains("NL-TOKEN-5150"),
                "{}",
                error.message()
            );
        }
    }
    assert_clean(&f, &project, &["NL-TOKEN-5150"]);
}

/// updater プラグインの確認で起きるエラー文を、モックのランタイムで確かめる（アプリの `update_check` は
/// `format!("更新を確認できませんでした: {e}")` でそのまま画面に出すので、プラグインのエラー文がそのまま見える）。
#[tokio::test]
async fn updater_check_errors_do_not_expose_a_token_in_the_endpoint() {
    use tauri::test::{mock_builder, mock_context, noop_assets};
    use tauri_plugin_updater::UpdaterExt;

    const TOKEN: &str = "ENDPOINT-TOKEN-4711";
    let mut context = mock_context(noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IHRlc3Q=",
            "endpoints": ["https://example.invalid/latest.json"],
        }),
    );
    let app = mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    let handle = app.handle().clone();

    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| match req.url.path() {
            "/notjson" => ResponseTemplate::new(200).set_body_string("<html>not json</html>"),
            "/nojson-fields" => ResponseTemplate::new(200).set_body_string("{}"),
            "/empty" => ResponseTemplate::new(204),
            _ => ResponseTemplate::new(404),
        })
        .mount(&server)
        .await;
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut messages: Vec<(String, String)> = Vec::new();
    for url in [
        format!("{}/missing?token={TOKEN}", server.uri()),
        format!("{}/notjson?token={TOKEN}", server.uri()),
        format!("{}/nojson-fields?token={TOKEN}", server.uri()),
        format!("http://user:{TOKEN}@127.0.0.1:{closed}/latest.json?token={TOKEN}"),
        format!("http://127.0.0.1:{closed}/latest.json?token={TOKEN}"),
    ] {
        let updater = handle
            .updater_builder()
            .endpoints(vec![url.parse().unwrap()])
            .unwrap()
            .build()
            .unwrap();
        match updater.check().await {
            // アプリが画面とログに出す文（`update_check` と同じ作り方）
            Err(err) => messages.push((url, nekoreq_lib::updater::check_error_message(&err))),
            Ok(_) => panic!("確認は失敗するはず: {url}"),
        }
    }
    // 204 は「更新なし」
    let updater = handle
        .updater_builder()
        .endpoints(vec![format!("{}/empty?token={TOKEN}", server.uri())
            .parse()
            .unwrap()])
        .unwrap()
        .build()
        .unwrap();
    assert!(updater.check().await.unwrap().is_none());

    let leaked: Vec<&(String, String)> =
        messages.iter().filter(|(_, m)| m.contains(TOKEN)).collect();
    assert!(
        leaked.is_empty(),
        "エラー文に URL のトークンが出ている（画面にそのまま出る）:\n{}",
        leaked
            .iter()
            .map(|(u, m)| format!("{u}\n  → {m}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[tokio::test]
async fn secret_echoed_in_the_response_content_type_is_masked() {
    const S: &str = "CT-SECRET-6063";
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(|req: &Request| {
            let trace = req
                .headers
                .get("x-trace")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default();
            ResponseTemplate::new(200)
                .insert_header(
                    "content-type",
                    format!("text/plain; x-trace={trace}").as_str(),
                )
                .set_body_string("ok")
        })
        .mount(&server)
        .await;
    let (project, _env) = project_with_env(&f, &server.uri(), &[("tok", S)]);
    let mut input = adhoc(&project, "GET", "{{baseUrl}}/ct");
    input.request.params = vec![kv(ParamIn::Header, "X-Trace", "{{tok}}")];
    let r = send(&f.state, input).await;
    assert_eq!(response_of(&r).status, 200);
    let e = latest(&f, &project);
    assert!(
        !e.response_content_type
            .clone()
            .unwrap_or_default()
            .contains(S),
        "response_content_type に残っている: {:?}",
        e.response_content_type
    );
    assert_clean(&f, &project, &[S]);
}
