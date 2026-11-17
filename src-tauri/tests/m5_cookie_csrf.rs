//! M5（cookieAuth と CSRF の自動化）を `AppState` 経由で通しで確かめる。Tauri は起動しない。
//!
//! Spring Security（Cookie 方式の CSRF）と Spring Session 風のサーバーを wiremock で作り、完了条件
//! 「パスワードログイン → メール認証コードログイン → 取引先登録（POST）が、セッションやトークンを手でコピーせずに通る」
//! を再現する。サーバーは次のように振る舞う（実際のバックエンドの SecurityConfig・AuthController に合わせた）:
//!
//! - GET 以外は、Cookie `XSRF-TOKEN` と ヘッダー `X-XSRF-TOKEN` の両方がそろって同じ値で、さらにサーバーが最後に発行した
//!   トークンと同じでなければ 403（FORBIDDEN）。
//! - `GET /api/v1/auth/session` が `XSRF-TOKEN` を発行する（Cookie に値がなければ）。
//! - パスワードログイン（1 段階目）で SESSION を発行し、メール認証コードログイン（2 段階目）は 1 段階目の SESSION を
//!   Cookie で受け取れないと 409。2 段階目でセッション ID とトークンを作り直す（ログイン後にトークンが変わる）。
//! - ログアウトは `SESSION=; Max-Age=0` で Cookie を消す。
//! - ログインの API は yaml では `security: []`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nekoreq_core::{
    AutoUpdateAction, Body, EnvVar, Environment, ImportOptions, KeyValue, OpKey, ParamIn, Project,
    ProjectSettings, RequestSpec, SendOutcome, VarValue,
};
use nekoreq_lib::openapi::{ImportRequest, ImportResult, SourceInput};
use nekoreq_lib::send::{RequestFinished, RequestInput};
use nekoreq_lib::state::AppState;
use nekoreq_secrets::{MemorySecretStore, SecretError, SecretKey, SecretStore};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

// ---- Spring 風のサーバー ----

const EMAIL: &str = "user@example.com";
const PASSWORD: &str = "pw-ok-123";
const CODE: &str = "246810";

#[derive(Default)]
struct Sess {
    code_pending: bool,
    logged_in: bool,
}

#[derive(Default)]
struct SpringState {
    counter: u32,
    /// サーバーが最後に発行した CSRF トークン。
    xsrf: Option<String>,
    sessions: HashMap<String, Sess>,
    /// 受け取ったリクエスト（method, path, Cookie ヘッダー, X-XSRF-TOKEN ヘッダー）。
    seen: Vec<Seen>,
}

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    cookie: Option<String>,
    xsrf_header: Option<String>,
}

#[derive(Clone, Default)]
struct Spring {
    state: Arc<Mutex<SpringState>>,
}

impl Spring {
    fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    fn last(&self) -> Seen {
        self.seen().last().cloned().expect("リクエストがある")
    }

    fn current_xsrf(&self) -> Option<String> {
        self.state.lock().unwrap().xsrf.clone()
    }
}

fn cookies_of(req: &Request) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(v) = req.headers.get("cookie").and_then(|v| v.to_str().ok()) {
        for pair in v.split(';') {
            if let Some((k, val)) = pair.trim().split_once('=') {
                out.insert(k.trim().to_string(), val.trim().to_string());
            }
        }
    }
    out
}

fn problem(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("content-type", "application/problem+json")
        .set_body_string(format!(r#"{{"status":{status},"code":"{code}"}}"#))
}

impl Respond for Spring {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.state.lock().unwrap();
        let method = req.method.as_str().to_string();
        let path = req.url.path().to_string();
        let cookies = cookies_of(req);
        let header = req
            .headers
            .get("x-xsrf-token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let seen = Seen {
            method: method.clone(),
            path: path.clone(),
            cookie: req
                .headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            xsrf_header: header.clone(),
        };

        let json = |body: &str| {
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(body.to_string())
        };
        let next_id = |prefix: &str, st: &mut SpringState| {
            st.counter += 1;
            format!("{prefix}-{}-secretvalue", st.counter)
        };
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let session_id = cookies.get("SESSION").cloned();

        // CSRF（GET・HEAD・OPTIONS 以外）: Cookie とヘッダーの両方がそろって、最後に発行した値と同じ
        let safe = matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS");
        let csrf_ok = safe
            || match (&header, cookies.get("XSRF-TOKEN"), &st.xsrf) {
                (Some(h), Some(c), Some(current)) => h == c && c == current,
                _ => false,
            };
        let resp = if !csrf_ok {
            problem(403, "FORBIDDEN")
        } else {
            match (method.as_str(), path.as_str()) {
                ("GET", "/api/v1/auth/session") => {
                    let logged_in = session_id
                        .as_ref()
                        .and_then(|id| st.sessions.get(id))
                        .is_some_and(|s| s.logged_in);
                    let mut r = json(&format!(r#"{{"authenticated":{logged_in}}}"#));
                    // Cookie に XSRF-TOKEN がなければ発行する
                    if !cookies.contains_key("XSRF-TOKEN") {
                        let t = next_id("XSRFV", &mut st);
                        st.xsrf = Some(t.clone());
                        r = r.append_header(
                            "set-cookie",
                            format!("XSRF-TOKEN={t}; Path=/; SameSite=Lax"),
                        );
                    }
                    r
                }
                ("POST", "/api/v1/auth/password-login") => {
                    if body["email"] == EMAIL && body["password"] == PASSWORD {
                        let id = next_id("SESSV", &mut st);
                        st.sessions.insert(
                            id.clone(),
                            Sess {
                                code_pending: true,
                                logged_in: false,
                            },
                        );
                        json(r#"{"data":{"expiresAt":"2026-11-17T10:10:00Z"}}"#).append_header(
                            "set-cookie",
                            format!("SESSION={id}; Path=/; HttpOnly; SameSite=Lax"),
                        )
                    } else {
                        problem(401, "LOGIN_FAILED")
                    }
                }
                ("POST", "/api/v1/auth/email-code-login") => {
                    let pending = session_id
                        .as_ref()
                        .and_then(|id| st.sessions.get(id))
                        .is_some_and(|s| s.code_pending);
                    if !pending {
                        problem(409, "INVALID_STATE")
                    } else if body["code"] != CODE {
                        problem(401, "LOGIN_FAILED")
                    } else {
                        // セッション ID を作り直し、CSRF トークンも作り直す
                        let old = session_id.clone().unwrap();
                        st.sessions.remove(&old);
                        let id = next_id("SESSV", &mut st);
                        st.sessions.insert(
                            id.clone(),
                            Sess {
                                code_pending: false,
                                logged_in: true,
                            },
                        );
                        let t = next_id("XSRFV", &mut st);
                        st.xsrf = Some(t.clone());
                        json(r#"{"data":{"authenticated":true}}"#)
                            .append_header(
                                "set-cookie",
                                format!("SESSION={id}; Path=/; HttpOnly; SameSite=Lax"),
                            )
                            .append_header(
                                "set-cookie",
                                format!("XSRF-TOKEN={t}; Path=/; SameSite=Lax"),
                            )
                    }
                }
                ("POST", "/api/v1/video/clients") => {
                    let logged_in = session_id
                        .as_ref()
                        .and_then(|id| st.sessions.get(id))
                        .is_some_and(|s| s.logged_in);
                    if logged_in {
                        json(r#"{"data":{"clientId":8,"status":"ACTIVE"}}"#)
                    } else {
                        problem(401, "UNAUTHENTICATED")
                    }
                }
                ("DELETE", "/api/v1/auth/session") => {
                    if let Some(id) = &session_id {
                        st.sessions.remove(id);
                    }
                    json(r#"{"data":{"authenticated":false}}"#).append_header(
                        "set-cookie",
                        "SESSION=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/",
                    )
                }
                _ => ResponseTemplate::new(404),
            }
        };
        st.seen.push(seen);
        resp
    }
}

async fn spring_server() -> (MockServer, Spring) {
    let server = MockServer::start().await;
    let spring = Spring::default();
    Mock::given(any())
        .respond_with(spring.clone())
        .mount(&server)
        .await;
    (server, spring)
}

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

const YAML: &str = r#"openapi: 3.0.3
info: {title: Spring Like API, version: '1'}
servers:
  - url: http://localhost:8080
    description: Local
components:
  securitySchemes:
    cookieAuth: {type: apiKey, in: cookie, name: SESSION}
security:
  - cookieAuth: []
paths:
  /api/v1/auth/session:
    get:
      operationId: getSession
      security: []
      responses: {'200': {description: ok}}
    delete:
      operationId: deleteSession
      responses: {'200': {description: ok}}
  /api/v1/auth/password-login:
    post:
      operationId: createPasswordLogin
      security: []
      requestBody:
        content:
          application/json:
            example: {email: user@example.com, password: pw-ok-123}
            schema: {type: object}
      responses: {'200': {description: ok}}
  /api/v1/auth/email-code-login:
    post:
      operationId: createEmailCodeLogin
      security: []
      requestBody:
        content:
          application/json:
            example: {code: '246810'}
            schema: {type: object}
      responses: {'200': {description: ok}}
  /api/v1/video/clients:
    post:
      operationId: createClient
      requestBody:
        content:
          application/json:
            example: {clientName: 新しい取引先, contractType: DIRECT}
            schema: {type: object}
      responses: {'200': {description: ok}}
"#;

/// yaml から取り込んだプロジェクトと、baseUrl をモックサーバーにした環境（選んでいる環境）。
async fn imported(f: &Fx, base_url: &str) -> (Project, Environment) {
    let entry = f.dir.path().join("spring-like.yaml");
    std::fs::write(&entry, YAML).unwrap();
    let result = f
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
    let ImportResult::Imported { project, .. } = result else {
        panic!("取り込めるはず")
    };
    let env = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|e| e.name == "Local")
        .unwrap();
    let mut env = env;
    for v in &mut env.vars {
        if v.name == "baseUrl" {
            v.value = VarValue::Plain(base_url.to_string());
        }
    }
    let env = f.state.env_save(&env).unwrap();
    let project = f.state.project_list().unwrap().remove(0);
    assert_eq!(
        project.default_environment_id.as_deref(),
        Some(env.id.as_str()),
        "最初の環境が選ばれている"
    );
    (project, env)
}

fn key(id: &str) -> OpKey {
    OpKey::Id(id.into())
}

fn op_input(f: &Fx, project: &Project, op: &str) -> RequestInput {
    let open = f.state.operation_open(&project.id, &key(op)).unwrap();
    RequestInput {
        project_id: project.id.clone(),
        environment_id: None,
        request: open.request,
        overrides: vec![],
        op_key: Some(key(op)),
    }
}

async fn send(state: &AppState, input: RequestInput) -> RequestFinished {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .request_send(input, None, move |f| {
            let _ = tx.send(f);
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("finished が届く")
        .unwrap()
}

fn status(f: &RequestFinished) -> u16 {
    match &f.result {
        SendOutcome::Success { response } => response.status,
        SendOutcome::Failure { error } => panic!("送信できるはず: {error:?}"),
    }
}

fn updated_names(f: &RequestFinished) -> Vec<(String, AutoUpdateAction)> {
    f.auto_updates
        .iter()
        .map(|u| (u.name.clone(), u.action))
        .collect()
}

fn secret(f: &Fx, project: &Project, env: &Environment, name: &str) -> Option<String> {
    f.secrets
        .get(&SecretKey::new(&project.id, &env.id, name))
        .unwrap()
}

fn env_var(f: &Fx, env: &Environment, name: &str) -> Option<EnvVar> {
    f.state
        .env_list(&env.project_id)
        .unwrap()
        .into_iter()
        .find(|e| e.id == env.id)
        .unwrap()
        .vars
        .into_iter()
        .find(|v| v.name == name)
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

// ---- 完了条件: ログインから取引先登録まで ----

#[tokio::test]
async fn login_then_email_code_then_create_client_without_copying_anything() {
    let f = fx();
    let (server, spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    // 手で入れたものは何もない（baseUrl だけ）
    assert!(env_var(&f, &env, "session").is_none());
    assert!(env_var(&f, &env, "xsrfToken").is_none());

    // 1. GET /auth/session: XSRF-TOKEN を受け取る（まだ SESSION はない）
    let r = send(&f.state, op_input(&f, &project, "getSession")).await;
    assert_eq!(status(&r), 200);
    assert_eq!(
        updated_names(&r),
        vec![("xsrfToken".to_string(), AutoUpdateAction::Updated)]
    );
    assert_eq!(r.auto_updates[0].environment_id, env.id);
    assert_eq!(r.auto_updates[0].environment_name, "Local");
    assert!(r.auto_update_skipped.is_empty());
    let first = spring.last();
    assert_eq!(first.cookie, None, "最初は何も送らない");
    let xsrf1 = secret(&f, &project, &env, "xsrfToken").expect("トークンを保持した");
    assert_eq!(Some(xsrf1.clone()), spring.current_xsrf());
    // 変数はシークレットとして作られた
    let v = env_var(&f, &env, "xsrfToken").unwrap();
    assert_eq!(v.value, VarValue::Secret);
    assert!(v.enabled);

    // 2. パスワードログイン（yaml では security: []）。Cookie とヘッダーの両方で CSRF を通し、SESSION を受け取る
    let r = send(&f.state, op_input(&f, &project, "createPasswordLogin")).await;
    assert_eq!(status(&r), 200, "CSRF が通る");
    assert_eq!(
        updated_names(&r),
        vec![("session".to_string(), AutoUpdateAction::Updated)]
    );
    let seen = spring.last();
    assert_eq!(seen.xsrf_header.as_deref(), Some(xsrf1.as_str()));
    assert_eq!(
        seen.cookie.as_deref(),
        Some(format!("XSRF-TOKEN={xsrf1}").as_str())
    );
    let pending = secret(&f, &project, &env, "session").expect("セッションを保持した");
    assert!(env_var(&f, &env, "session").is_some_and(|v| v.value == VarValue::Secret));

    // 3. メール認証コードログイン（これも security: []）。1 段階目の SESSION を自動で付ける。
    //    セッション ID とトークンが作り直される
    let r = send(&f.state, op_input(&f, &project, "createEmailCodeLogin")).await;
    assert_eq!(status(&r), 200, "1 段階目の SESSION を付けたので通る");
    assert_eq!(
        updated_names(&r),
        vec![
            ("session".to_string(), AutoUpdateAction::Updated),
            ("xsrfToken".to_string(), AutoUpdateAction::Updated),
        ]
    );
    let seen = spring.last();
    assert_eq!(
        seen.cookie.as_deref(),
        Some(format!("SESSION={pending}; XSRF-TOKEN={xsrf1}").as_str())
    );
    let session = secret(&f, &project, &env, "session").unwrap();
    let xsrf2 = secret(&f, &project, &env, "xsrfToken").unwrap();
    assert_ne!(session, pending, "セッション ID は作り直された");
    assert_ne!(xsrf2, xsrf1, "ログイン後にトークンが変わった");
    assert_eq!(Some(xsrf2.clone()), spring.current_xsrf());

    // 4. 取引先登録（POST）。プレビューには自動で付くヘッダーが出どころつき・伏せ字で出る
    let input = op_input(&f, &project, "createClient");
    let preview = f.state.request_preview(&input).unwrap();
    let cookie = preview
        .auto_headers
        .iter()
        .find(|h| h.name == "Cookie")
        .expect("Cookie が自動で付く");
    assert_eq!(cookie.value, "SESSION=••••; XSRF-TOKEN=••••");
    assert_eq!(cookie.source, "認証タブ（cookieAuth）、CSRF の自動付与");
    let xsrf_header = preview
        .auto_headers
        .iter()
        .find(|h| h.name == "X-XSRF-TOKEN")
        .expect("X-XSRF-TOKEN が自動で付く");
    assert_eq!(xsrf_header.value, "••••");
    assert_eq!(xsrf_header.source, "CSRF の自動付与");
    let dump = format!("{preview:?}");
    assert!(
        !dump.contains(&session) && !dump.contains(&xsrf2),
        "プレビューに値が出ている: {dump}"
    );
    assert!(preview.auto_notes.is_empty(), "{:?}", preview.auto_notes);

    let r = send(&f.state, input).await;
    assert_eq!(status(&r), 200, "手でコピーせずに登録できる");
    assert!(r.auto_updates.is_empty());
    let seen = spring.last();
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("POST", "/api/v1/video/clients")
    );
    assert_eq!(seen.xsrf_header.as_deref(), Some(xsrf2.as_str()));
    assert_eq!(
        seen.cookie.as_deref(),
        Some(format!("SESSION={session}; XSRF-TOKEN={xsrf2}").as_str())
    );
    assert!(
        matches!(&r.result, SendOutcome::Success { response } if response.body.content.contains("ACTIVE"))
    );
    // Cookie が 1 本だけ（既定の認証と自動付与が重ならない）
    assert_eq!(
        seen.cookie.as_deref().unwrap().matches("SESSION=").count(),
        1
    );

    // 5. ログアウトで SESSION が消される（Max-Age=0）→ 値を消す。トークンはそのまま使える
    let r = send(&f.state, op_input(&f, &project, "deleteSession")).await;
    assert_eq!(status(&r), 200);
    assert_eq!(
        updated_names(&r),
        vec![("session".to_string(), AutoUpdateAction::Cleared)]
    );
    assert_eq!(secret(&f, &project, &env, "session"), None);
    assert_eq!(
        secret(&f, &project, &env, "xsrfToken").as_deref(),
        Some(xsrf2.as_str())
    );
    let r = send(&f.state, op_input(&f, &project, "createClient")).await;
    assert_eq!(status(&r), 401, "ログアウト後は未ログイン");
}

#[tokio::test]
async fn nothing_leaks_to_sqlite_debug_or_error_messages() {
    let f = fx();
    let (server, spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    // 手で作った通常の変数 session があっても、自動更新でシークレットに変わり、元の値はファイルに残らない
    let mut e = env.clone();
    e.vars.push(EnvVar {
        name: "session".into(),
        value: VarValue::Plain("OLD-PLAIN-SESSION-9999".into()),
        description: "手で入れた".into(),
        enabled: true,
    });
    f.state.env_save(&e).unwrap();

    let mut all: Vec<RequestFinished> = Vec::new();
    for op in [
        "getSession",
        "createPasswordLogin",
        "createEmailCodeLogin",
        "createClient",
    ] {
        all.push(send(&f.state, op_input(&f, &project, op)).await);
    }
    let session = secret(&f, &project, &env, "session").unwrap();
    let xsrf = secret(&f, &project, &env, "xsrfToken").unwrap();
    let converted = env_var(&f, &env, "session").unwrap();
    assert_eq!(
        converted.value,
        VarValue::Secret,
        "通常の変数はシークレットになる"
    );
    assert_eq!(converted.description, "手で入れた");

    // SQLite（本体・WAL）に値がない
    assert_not_on_disk(
        &f.data,
        &[
            &session,
            &xsrf,
            "OLD-PLAIN-SESSION-9999",
            "SESSV-",
            "XSRFV-",
        ],
    );
    // 画面へ渡す結果（Debug）に値がない。レスポンスの Set-Cookie も伏せてある
    for r in &all {
        let d = format!("{r:?}");
        assert!(
            !d.contains("SESSV-") && !d.contains("XSRFV-") && !d.contains(&session),
            "結果の Debug に値が出ている: {d}"
        );
    }
    let SendOutcome::Success { response } = &all[0].result else {
        panic!()
    };
    let set_cookie: Vec<&str> = response
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
        .map(|h| h.value.as_str())
        .collect();
    assert_eq!(set_cookie, vec!["XSRF-TOKEN=••••; Path=/; SameSite=Lax"]);
    // 結果のシリアライズにも出ない（画面に届く形）
    let json = serde_json::to_string(&response.headers).unwrap();
    assert!(!json.contains("XSRFV-"));
    // プロジェクトの設定・環境の一覧にも値はない
    let dump = format!(
        "{:?} {:?}",
        f.state.project_list().unwrap(),
        f.state.env_list(&project.id).unwrap()
    );
    assert!(
        !dump.contains("SESSV-") && !dump.contains("XSRFV-"),
        "{dump}"
    );
    // サーバーが受け取ったのは本物の値（伏せ字を送っていない）
    let seen = spring.last();
    assert!(seen.cookie.unwrap().contains(&session));
}

#[tokio::test]
async fn a_keychain_write_failure_is_reported_without_the_value_and_does_not_fail_the_send() {
    #[derive(Default)]
    struct Flaky {
        inner: MemorySecretStore,
        fail_set: AtomicBool,
    }
    impl SecretStore for Flaky {
        fn get(&self, key: &SecretKey) -> nekoreq_secrets::Result<Option<String>> {
            self.inner.get(key)
        }
        fn set(&self, key: &SecretKey, value: &str) -> nekoreq_secrets::Result<()> {
            if self.fail_set.load(Ordering::SeqCst) {
                // 実際のキーチェーンの失敗が値を含むことはないが、含んでしまっても伏せられることを確かめる
                return Err(SecretError::Backend(format!("書き込めません ({value})")));
            }
            self.inner.set(key, value)
        }
        fn delete(&self, key: &SecretKey) -> nekoreq_secrets::Result<()> {
            self.inner.delete(key)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(Flaky::default());
    secrets.fail_set.store(true, Ordering::SeqCst);
    let f = Fx {
        data: dir.path().join("data"),
        state: AppState::open_with_secrets(&dir.path().join("data"), secrets.clone()).unwrap(),
        secrets: Arc::new(MemorySecretStore::new()),
        dir,
    };
    let (server, spring) = spring_server().await;
    let (project, _env) = imported(&f, &server.uri()).await;

    let r = send(&f.state, op_input(&f, &project, "getSession")).await;
    assert_eq!(status(&r), 200, "送信は成功している");
    assert!(r.auto_updates.is_empty());
    assert_eq!(r.auto_update_skipped.len(), 1);
    assert_eq!(r.auto_update_skipped[0].name, "xsrfToken");
    let reason = &r.auto_update_skipped[0].reason;
    assert!(reason.contains("保存できませんでした"), "{reason}");
    let issued = spring.current_xsrf().unwrap();
    assert!(
        !reason.contains(&issued) && !format!("{r:?}").contains(&issued),
        "理由に値が出ている: {reason}"
    );
    // 直ったら次の送信で保持できる（Cookie がないのでサーバーはまた発行する）
    secrets.fail_set.store(false, Ordering::SeqCst);
    let r = send(&f.state, op_input(&f, &project, "getSession")).await;
    assert_eq!(updated_names(&r).len(), 1);
}

// ---- 設定で個別に切り替えられる ----

fn set_auto(f: &Fx, project: &Project, auto_session: bool, auto_csrf: bool) {
    let p = f
        .state
        .project_settings_set(
            &project.id,
            &ProjectSettings {
                auto_session,
                auto_csrf,
            },
        )
        .unwrap();
    assert_eq!(p.settings.auto_session, auto_session);
    assert_eq!(p.settings.auto_csrf, auto_csrf);
}

#[tokio::test]
async fn csrf_needs_both_the_cookie_and_the_header_and_each_automation_can_be_switched() {
    let f = fx();
    let (server, spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    assert!(
        project.settings.auto_session && project.settings.auto_csrf,
        "既定はどちらもオン"
    );

    // CSRF をオフ: GET でトークンが出ても保持しない。POST は 403
    set_auto(&f, &project, true, false);
    let r = send(&f.state, op_input(&f, &project, "getSession")).await;
    assert!(r.auto_updates.is_empty(), "オフなら保持しない");
    assert_eq!(secret(&f, &project, &env, "xsrfToken"), None);
    let r = send(&f.state, op_input(&f, &project, "createPasswordLogin")).await;
    assert_eq!(status(&r), 403);
    // オフのとき、プレビューにも付かない
    let preview = f
        .state
        .request_preview(&op_input(&f, &project, "createPasswordLogin"))
        .unwrap();
    assert!(preview
        .auto_headers
        .iter()
        .all(|h| h.name != "X-XSRF-TOKEN" && h.name != "Cookie"));
    assert!(preview.auto_notes.is_empty());

    // オンに戻しても、トークンがまだないうちは付けられない（理由がプレビューに出る）。POST は 403
    set_auto(&f, &project, true, true);
    let preview = f
        .state
        .request_preview(&op_input(&f, &project, "createPasswordLogin"))
        .unwrap();
    assert!(
        preview.auto_notes.iter().any(|n| n.contains("xsrfToken")),
        "{:?}",
        preview.auto_notes
    );
    let r = send(&f.state, op_input(&f, &project, "createPasswordLogin")).await;
    assert_eq!(status(&r), 403);

    // GET でトークンを受け取る（サーバーは Cookie がなければ発行し直す）
    let r = send(&f.state, op_input(&f, &project, "getSession")).await;
    assert_eq!(
        updated_names(&r),
        vec![("xsrfToken".to_string(), AutoUpdateAction::Updated)]
    );
    let token = secret(&f, &project, &env, "xsrfToken").unwrap();

    // 手入力の同名ヘッダーが優先される: 違う値を入れたら、Cookie（自動）と食い違って 403
    let mut input = op_input(&f, &project, "createPasswordLogin");
    input.request.params.push(KeyValue {
        location: ParamIn::Header,
        name: "X-XSRF-TOKEN".into(),
        value: "manual-wrong-token".into(),
        enabled: true,
    });
    let r = send(&f.state, input).await;
    assert_eq!(status(&r), 403);
    assert_eq!(
        spring.last().xsrf_header.as_deref(),
        Some("manual-wrong-token")
    );
    // 手入力のヘッダーが `{{xsrfToken}}`（画面イメージの行）なら、同じ値なので通る。ヘッダーは 1 本だけ
    let mut input = op_input(&f, &project, "createPasswordLogin");
    input.request.params.push(KeyValue {
        location: ParamIn::Header,
        name: "X-XSRF-TOKEN".into(),
        value: "{{xsrfToken}}".into(),
        enabled: true,
    });
    let r = send(&f.state, input).await;
    assert_eq!(status(&r), 200);
    assert_eq!(spring.last().xsrf_header.as_deref(), Some(token.as_str()));
    // 手入力の Cookie（SESSION）も優先される
    let mut input = op_input(&f, &project, "createEmailCodeLogin");
    input.request.params.push(KeyValue {
        location: ParamIn::Cookie,
        name: "SESSION".into(),
        value: "manual-cookie-session".into(),
        enabled: true,
    });
    let _ = send(&f.state, input).await;
    let cookie = spring.last().cookie.unwrap();
    assert!(cookie.contains("SESSION=manual-cookie-session"), "{cookie}");
    assert_eq!(cookie.matches("SESSION=").count(), 1, "{cookie}");
}

#[tokio::test]
async fn session_automation_off_leaves_session_alone_and_does_not_send_it_to_open_apis() {
    let f = fx();
    let (server, spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    set_auto(&f, &project, false, true);

    // トークンだけ自動（GET → POST）。ログインの SESSION は保持しない
    send(&f.state, op_input(&f, &project, "getSession")).await;
    let r = send(&f.state, op_input(&f, &project, "createPasswordLogin")).await;
    assert_eq!(status(&r), 200);
    assert!(
        r.auto_updates.iter().all(|u| u.name != "session"),
        "{:?}",
        r.auto_updates
    );
    assert_eq!(secret(&f, &project, &env, "session"), None);
    assert!(env_var(&f, &env, "session").is_none());

    // 手で session を入れておくと、既定の認証（cookieAuth の API）には付くが、security: [] の API には付かない
    let mut e = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|x| x.id == env.id)
        .unwrap();
    e.vars.push(EnvVar {
        name: "session".into(),
        value: VarValue::Secret,
        description: String::new(),
        enabled: true,
    });
    f.state.env_save(&e).unwrap();
    f.state
        .secret_set(&project.id, &env.id, "session", "manual-session-1")
        .unwrap();
    send(&f.state, op_input(&f, &project, "getSession")).await;
    assert!(
        !spring
            .last()
            .cookie
            .unwrap_or_default()
            .contains("SESSION="),
        "オフなら security: [] の API に SESSION は付かない"
    );
    send(&f.state, op_input(&f, &project, "createClient")).await;
    assert!(spring
        .last()
        .cookie
        .unwrap()
        .contains("SESSION=manual-session-1"));
    // 自動で更新もされない（ログアウトの Max-Age=0 でも消えない）
    let r = send(&f.state, op_input(&f, &project, "deleteSession")).await;
    assert!(r.auto_updates.is_empty());
    assert_eq!(
        secret(&f, &project, &env, "session").as_deref(),
        Some("manual-session-1")
    );
}

#[tokio::test]
async fn explicit_auth_choices_are_respected() {
    let f = fx();
    let (server, spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    send(&f.state, op_input(&f, &project, "getSession")).await;
    send(&f.state, op_input(&f, &project, "createPasswordLogin")).await;
    assert!(secret(&f, &project, &env, "session").is_some());

    // 認証で「なし」を選んだリクエストには SESSION を付けない（CSRF の Cookie・ヘッダーは付く）
    let mut input = op_input(&f, &project, "createClient");
    input.request.auth = Some(nekoreq_core::AuthOverride::None);
    send(&f.state, input).await;
    let seen = spring.last();
    let cookie = seen.cookie.unwrap();
    assert!(!cookie.contains("SESSION="), "{cookie}");
    assert!(cookie.contains("XSRF-TOKEN="), "{cookie}");
    assert!(seen.xsrf_header.is_some());
}

// ---- 環境が未選択のとき ----

#[tokio::test]
async fn without_a_selected_environment_nothing_is_sent_or_saved_to_global() {
    let f = fx();
    let (server, _spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    // 環境を未選択にする（グローバルだけ）。baseUrl はグローバルに置いて送れるようにする
    f.state.env_select(&project.id, None).unwrap();
    let global = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .find(|e| e.is_global)
        .unwrap();
    let mut g = global.clone();
    g.vars.push(EnvVar {
        name: "baseUrl".into(),
        value: VarValue::Plain(server.uri()),
        description: String::new(),
        enabled: true,
    });
    f.state.env_save(&g).unwrap();

    let r = send(&f.state, op_input(&f, &project, "getSession")).await;
    assert_eq!(status(&r), 200);
    assert!(r.auto_updates.is_empty(), "グローバルには保存しない");
    assert_eq!(r.auto_update_skipped.len(), 1);
    assert_eq!(r.auto_update_skipped[0].name, "xsrfToken");
    assert!(
        r.auto_update_skipped[0]
            .reason
            .contains("環境が選ばれていない"),
        "{}",
        r.auto_update_skipped[0].reason
    );
    // グローバルにも、選んでいない環境にも、キーチェーンにも入らない
    let envs = f.state.env_list(&project.id).unwrap();
    assert!(envs
        .iter()
        .all(|e| e.vars.iter().all(|v| v.name != "xsrfToken")));
    assert_eq!(f.secrets.len(), 0);
    assert_eq!(secret(&f, &project, &env, "xsrfToken"), None);

    // 環境を明示して送れば（環境 ID を渡す）、その環境に保存される
    let mut input = op_input(&f, &project, "getSession");
    input.environment_id = Some(env.id.clone());
    let r = send(&f.state, input).await;
    assert_eq!(
        updated_names(&r),
        vec![("xsrfToken".to_string(), AutoUpdateAction::Updated)]
    );
    assert!(secret(&f, &project, &env, "xsrfToken").is_some());
    // 「グローバル」を環境として指定しても、グローバルには保存しない
    let mut input = op_input(&f, &project, "getSession");
    input.environment_id = Some(global.id.clone());
    f.secrets
        .delete(&SecretKey::new(&project.id, &env.id, "xsrfToken"))
        .unwrap();
    let r = send(&f.state, input).await;
    assert!(r.auto_updates.is_empty());
    assert_eq!(r.auto_update_skipped.len(), 1);
}

// ---- 手で作ったリクエスト・送り先の確認 ----

fn adhoc(method: &str, url: &str, body: Body) -> RequestInput {
    RequestInput {
        project_id: String::new(),
        environment_id: None,
        request: RequestSpec {
            method: method.into(),
            url: url.into(),
            params: vec![],
            body,
            auth: None,
        },
        overrides: vec![],
        op_key: None,
    }
}

#[tokio::test]
async fn adhoc_requests_work_the_same_way() {
    let f = fx();
    let (server, spring) = spring_server().await;
    let project = f.state.project_create("手で作る").unwrap();
    let env = f.state.env_create(&project.id, "dev").unwrap();
    let mut e = env.clone();
    e.vars = vec![EnvVar {
        name: "baseUrl".into(),
        value: VarValue::Plain(server.uri()),
        description: String::new(),
        enabled: true,
    }];
    let env = f.state.env_save(&e).unwrap();
    assert_eq!(
        f.state.project_list().unwrap()[0]
            .default_environment_id
            .as_deref(),
        Some(env.id.as_str())
    );
    let mk = |method: &str, path: &str, body: Body| {
        let mut i = adhoc(method, &format!("{{{{baseUrl}}}}{path}"), body);
        i.project_id = project.id.clone();
        i
    };

    let r = send(&f.state, mk("GET", "/api/v1/auth/session", Body::None)).await;
    assert_eq!(
        updated_names(&r),
        vec![("xsrfToken".to_string(), AutoUpdateAction::Updated)]
    );
    let r = send(
        &f.state,
        mk(
            "POST",
            "/api/v1/auth/password-login",
            Body::Json(format!(r#"{{"email":"{EMAIL}","password":"{PASSWORD}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status(&r), 200);
    assert_eq!(
        updated_names(&r),
        vec![("session".to_string(), AutoUpdateAction::Updated)]
    );
    let r = send(
        &f.state,
        mk(
            "POST",
            "/api/v1/auth/email-code-login",
            Body::Json(format!(r#"{{"code":"{CODE}"}}"#)),
        ),
    )
    .await;
    assert_eq!(status(&r), 200);
    let r = send(
        &f.state,
        mk(
            "POST",
            "/api/v1/video/clients",
            Body::Json(r#"{"clientName":"x","contractType":"DIRECT"}"#.into()),
        ),
    )
    .await;
    assert_eq!(status(&r), 200);
    assert!(spring.last().cookie.unwrap().contains("SESSION="));
}

#[tokio::test]
async fn cookies_are_neither_sent_to_nor_saved_from_another_origin() {
    let f = fx();
    let (server, _spring) = spring_server().await;
    let (other, other_spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    // 本来の送り先でトークンとセッションを持つ
    send(&f.state, op_input(&f, &project, "getSession")).await;
    send(&f.state, op_input(&f, &project, "createPasswordLogin")).await;
    let session = secret(&f, &project, &env, "session").unwrap();
    let xsrf = secret(&f, &project, &env, "xsrfToken").unwrap();

    // 別のサイトへ手で作ったリクエストを送る: セッションもトークンも送らない
    let mut input = adhoc(
        "POST",
        &format!("{}/api/v1/auth/password-login", other.uri()),
        Body::None,
    );
    input.project_id = project.id.clone();
    let preview = f.state.request_preview(&input).unwrap();
    assert!(preview
        .auto_headers
        .iter()
        .all(|h| h.name != "Cookie" && h.name != "X-XSRF-TOKEN"));
    assert!(
        preview.auto_notes.iter().any(|n| n.contains("baseUrl")),
        "{:?}",
        preview.auto_notes
    );
    let r = send(&f.state, input).await;
    let seen = other_spring.last();
    assert_eq!(seen.cookie, None);
    assert_eq!(seen.xsrf_header, None);
    assert!(!format!("{seen:?}").contains(&session) && !format!("{seen:?}").contains(&xsrf));
    assert_eq!(status(&r), 403, "CSRF を通せない（それでよい）");

    // 別のサイトが SESSION・XSRF-TOKEN を Set-Cookie しても、保存しない（セッションを上書きされない）
    let mut input = adhoc(
        "GET",
        &format!("{}/api/v1/auth/session", other.uri()),
        Body::None,
    );
    input.project_id = project.id.clone();
    let r = send(&f.state, input).await;
    assert!(r.auto_updates.is_empty());
    assert_eq!(r.auto_update_skipped.len(), 1);
    assert!(
        r.auto_update_skipped[0].reason.contains("baseUrl"),
        "{:?}",
        r.auto_update_skipped
    );
    assert_eq!(
        secret(&f, &project, &env, "xsrfToken").as_deref(),
        Some(xsrf.as_str())
    );
    assert_eq!(
        secret(&f, &project, &env, "session").as_deref(),
        Some(session.as_str())
    );
}

#[tokio::test]
async fn an_environment_without_base_url_does_not_automate_and_says_why() {
    let f = fx();
    let (server, _spring) = spring_server().await;
    let project = f.state.project_create("baseUrl なし").unwrap();
    f.state.env_create(&project.id, "dev").unwrap();
    let mut input = adhoc(
        "GET",
        &format!("{}/api/v1/auth/session", server.uri()),
        Body::None,
    );
    input.project_id = project.id.clone();
    let preview = f.state.request_preview(&input).unwrap();
    assert!(
        preview.auto_notes.iter().any(|n| n.contains("baseUrl")),
        "{:?}",
        preview.auto_notes
    );
    let r = send(&f.state, input).await;
    assert_eq!(status(&r), 200);
    assert!(r.auto_updates.is_empty());
    assert_eq!(r.auto_update_skipped.len(), 1);
}

#[tokio::test]
async fn an_unchanged_value_is_not_reported_again_and_a_changed_one_is() {
    use wiremock::matchers::path;
    let f = fx();
    // 毎回、同じ値の Set-Cookie を返すサーバー（途中で値を変える）
    let server = MockServer::start().await;
    Mock::given(path("/same"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "SESSION=SAME-SESSION-VALUE; Path=/")
                .append_header("set-cookie", "XSRF-TOKEN=SAME-XSRF-VALUE; Path=/"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/changed"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "SESSION=OTHER-SESSION-VALUE; Path=/"),
        )
        .mount(&server)
        .await;
    let project = f.state.project_create("P").unwrap();
    let env = f.state.env_create(&project.id, "dev").unwrap();
    let mut e = env.clone();
    e.vars = vec![EnvVar {
        name: "baseUrl".into(),
        value: VarValue::Plain(server.uri()),
        description: String::new(),
        enabled: true,
    }];
    let env = f.state.env_save(&e).unwrap();
    let get = |p: &str| {
        let mut i = adhoc("GET", &format!("{{{{baseUrl}}}}{p}"), Body::None);
        i.project_id = project.id.clone();
        i
    };

    let r = send(&f.state, get("/same")).await;
    assert_eq!(
        updated_names(&r),
        vec![
            ("session".to_string(), AutoUpdateAction::Updated),
            ("xsrfToken".to_string(), AutoUpdateAction::Updated)
        ]
    );
    // 同じ値が来ても「更新」にはしない（値は変わっていない）
    let r = send(&f.state, get("/same")).await;
    assert!(r.auto_updates.is_empty(), "{:?}", r.auto_updates);
    assert!(r.auto_update_skipped.is_empty());
    assert_eq!(
        secret(&f, &project, &env, "session").as_deref(),
        Some("SAME-SESSION-VALUE")
    );
    // 値が変われば更新として報告する
    let r = send(&f.state, get("/changed")).await;
    assert_eq!(
        updated_names(&r),
        vec![("session".to_string(), AutoUpdateAction::Updated)]
    );
    assert_eq!(
        secret(&f, &project, &env, "session").as_deref(),
        Some("OTHER-SESSION-VALUE")
    );
    assert_eq!(
        secret(&f, &project, &env, "xsrfToken").as_deref(),
        Some("SAME-XSRF-VALUE")
    );
}

#[tokio::test]
async fn project_settings_survive_reopen_and_are_per_project() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let secrets = Arc::new(MemorySecretStore::new());
    let (a, b) = {
        let state = AppState::open_with_secrets(&data, secrets.clone()).unwrap();
        let a = state.project_create("A").unwrap();
        let b = state.project_create("B").unwrap();
        state
            .project_settings_set(
                &a.id,
                &ProjectSettings {
                    auto_session: false,
                    auto_csrf: true,
                },
            )
            .unwrap();
        (a, b)
    };
    let state = AppState::open_with_secrets(&data, secrets).unwrap();
    let list = state.project_list().unwrap();
    let a = list.iter().find(|p| p.id == a.id).unwrap();
    let b = list.iter().find(|p| p.id == b.id).unwrap();
    assert!(!a.settings.auto_session && a.settings.auto_csrf);
    assert_eq!(b.settings, ProjectSettings::default());
    assert!(matches!(
        state.project_settings_set("nope", &ProjectSettings::default()),
        Err(nekoreq_lib::error::AppError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_locked_keychain_does_not_stop_requests_that_do_not_use_secrets() {
    #[derive(Default)]
    struct Locked {
        inner: MemorySecretStore,
        fail_get: AtomicBool,
    }
    impl SecretStore for Locked {
        fn get(&self, key: &SecretKey) -> nekoreq_secrets::Result<Option<String>> {
            if self.fail_get.load(Ordering::SeqCst) {
                return Err(SecretError::Backend("ロックされています".into()));
            }
            self.inner.get(key)
        }
        fn set(&self, key: &SecretKey, value: &str) -> nekoreq_secrets::Result<()> {
            self.inner.set(key, value)
        }
        fn delete(&self, key: &SecretKey) -> nekoreq_secrets::Result<()> {
            self.inner.delete(key)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(Locked::default());
    let data = dir.path().join("data");
    let f = Fx {
        state: AppState::open_with_secrets(&data, secrets.clone()).unwrap(),
        secrets: Arc::new(MemorySecretStore::new()),
        data,
        dir,
    };
    let (server, _spring) = spring_server().await;
    let (project, env) = imported(&f, &server.uri()).await;
    // 値を持った状態にしてから、キーチェーンを読めなくする
    send(&f.state, op_input(&f, &project, "getSession")).await;
    send(&f.state, op_input(&f, &project, "createPasswordLogin")).await;
    for name in ["xsrfToken", "session"] {
        assert!(secrets
            .get(&SecretKey::new(&project.id, &env.id, name))
            .unwrap()
            .is_some());
    }
    secrets.fail_get.store(true, Ordering::SeqCst);

    // 認証なしの GET は送れる。プレビューには理由が出て、自動更新できなかったことも知らせる
    let input = op_input(&f, &project, "getSession");
    let preview = f.state.request_preview(&input).unwrap();
    assert!(
        preview
            .auto_notes
            .iter()
            .any(|n| n.contains("キーチェーン")),
        "{:?}",
        preview.auto_notes
    );
    let r = send(&f.state, input).await;
    assert_eq!(status(&r), 200);
    // 既定の認証が session を使う API は、これまでどおりキーチェーンのエラー（送信しない）
    assert!(matches!(
        f.state
            .request_send(op_input(&f, &project, "createClient"), None, |_| {}),
        Err(nekoreq_lib::error::AppError::Secret(_))
    ));
}
