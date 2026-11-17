use std::sync::Arc;
use std::time::Duration;

use nekoreq_core::{
    AdHocRequest, AuthOverride, Body, EnvVar, Environment, HeaderPair, KeyValue, ParamIn,
    SendError, SendOutcome, SendSettings, VarLocation, VarValue,
};
use nekoreq_secrets::{MemorySecretStore, SecretKey, SecretStore};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::send::{RequestFinished, RequestInput, VarOverride};

struct Fixture {
    _dir: tempfile::TempDir,
    state: AppState,
    secrets: Arc<MemorySecretStore>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::new());
    let state = AppState::open_with_secrets(dir.path(), secrets.clone()).unwrap();
    Fixture {
        _dir: dir,
        state,
        secrets,
    }
}

fn plain(name: &str, value: &str) -> EnvVar {
    EnvVar {
        name: name.into(),
        value: VarValue::Plain(value.into()),
        description: String::new(),
        enabled: true,
    }
}

fn secret_var(name: &str) -> EnvVar {
    EnvVar {
        name: name.into(),
        value: VarValue::Secret,
        description: String::new(),
        enabled: true,
    }
}

fn with_vars(mut env: Environment, vars: Vec<EnvVar>) -> Environment {
    env.vars = vars;
    env
}

fn kv(location: ParamIn, name: &str, value: &str) -> KeyValue {
    KeyValue {
        location,
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}

fn get_request(project_id: &str, url: &str) -> AdHocRequest {
    AdHocRequest {
        id: String::new(),
        project_id: project_id.into(),
        name: "セッション確認".into(),
        description: String::new(),
        method: "GET".into(),
        url: url.into(),
        params: vec![],
        body: Body::None,
        auth: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn input(project_id: &str, req: &AdHocRequest) -> RequestInput {
    RequestInput {
        project_id: project_id.into(),
        environment_id: None,
        request: nekoreq_core::RequestSpec::from(req),
        overrides: vec![],
        op_key: None,
    }
}

/// 送信して、`finished` が届くのを待つ。
async fn send_and_wait(
    state: &AppState,
    input: RequestInput,
    settings: Option<SendSettings>,
) -> RequestFinished {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = state
        .request_send(input, settings, move |f| {
            let _ = tx.send(f);
        })
        .unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("finished が届く")
        .unwrap();
    assert_eq!(finished.send_id, id);
    finished
}

fn success(f: RequestFinished) -> nekoreq_core::ResponseData {
    match f.result {
        SendOutcome::Success { response } => response,
        SendOutcome::Failure { error } => panic!("成功のはず: {error:?}"),
    }
}

fn failure(f: RequestFinished) -> SendError {
    match f.result {
        SendOutcome::Failure { error } => error,
        SendOutcome::Success { response } => panic!("失敗のはず: {}", response.status),
    }
}

// ---- M1 から ----

#[test]
fn project_lifecycle_through_state() {
    let f = fixture();
    let state = &f.state;
    assert!(state.project_list().unwrap().is_empty());

    let p = state.project_create("テスト").unwrap();
    assert_eq!(state.project_list().unwrap(), vec![p.clone()]);
    let r = state.project_rename(&p.id, "改名").unwrap();
    assert_eq!(r.name, "改名");
    state.project_delete(&p.id).unwrap();
    assert!(state.project_list().unwrap().is_empty());
    assert!(matches!(
        state.project_delete(&p.id),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        state.project_create(" "),
        Err(AppError::Validation(_))
    ));
}

#[test]
fn projects_persist_across_reopen_and_info_reports_paths() {
    let dir = tempfile::tempdir().unwrap();
    let created = {
        let state =
            AppState::open_with_secrets(dir.path(), Arc::new(MemorySecretStore::new())).unwrap();
        state.project_create("残る").unwrap()
    };
    let state =
        AppState::open_with_secrets(dir.path(), Arc::new(MemorySecretStore::new())).unwrap();
    assert_eq!(state.project_list().unwrap(), vec![created]);

    let info = state.app_info("0.1.0", "com.nekoreq.desktop").unwrap();
    assert_eq!(info.name, "NekoReq");
    assert_eq!(info.schema_version, latest_schema_version());
    assert!(info.db_path.ends_with(DB_FILE_NAME));
}

// ---- 送信（M2） ----

#[tokio::test]
async fn base_url_in_the_environment_then_get_through_wiremock() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(r#"{"data":{"authenticated":false}}"#, "application/json"),
        )
        .mount(&server)
        .await;

    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f
        .state
        .env_create(&p.id, "ローカル (Dev Container)")
        .unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![plain("baseUrl", &server.uri())],
        ))
        .unwrap();
    // 最初に作った環境が「選んでいる環境」になっているので、環境の指定はいらない
    assert_eq!(
        f.state.project_list().unwrap()[0].default_environment_id,
        Some(env.id.clone())
    );

    let req = f
        .state
        .adhoc_save(&get_request(&p.id, "{{baseUrl}}/api/v1/auth/session"))
        .unwrap();
    assert_eq!(req.method, "GET");

    let finished = send_and_wait(&f.state, input(&p.id, &req), None).await;
    let r = success(finished);
    assert_eq!(r.status, 200);
    assert_eq!(r.body.content, r#"{"data":{"authenticated":false}}"#);
    assert_eq!(r.final_url, format!("{}/api/v1/auth/session", server.uri()));
    assert!(r.timing.connect_ms.is_some());
    assert!(
        f.state.inner.in_flight().is_empty(),
        "終わったら送信中から消える"
    );
}

#[tokio::test]
async fn preview_shows_the_expanded_url_masks_secrets_and_lists_unresolved() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![
                plain("baseUrl", "http://localhost:8080"),
                secret_var("apiKey"),
            ],
        ))
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "apiKey", "KEY-123")
        .unwrap();

    let mut req = get_request(&p.id, "{{baseUrl}}/x/{{missing}}");
    req.params = vec![
        kv(ParamIn::Query, "key", "{{apiKey}}"),
        kv(ParamIn::Query, "q", "Netflix"),
    ];
    let prev = f.state.request_preview(&input(&p.id, &req)).unwrap();
    assert_eq!(
        prev.url,
        "http://localhost:8080/x/{{missing}}?key=••••&q=Netflix"
    );
    assert_eq!(prev.unresolved.len(), 1);
    assert_eq!(prev.unresolved[0].name, "missing");
    assert_eq!(prev.unresolved[0].location, VarLocation::Url);

    req.url = "{{baseUrl}}/x".into();
    let prev = f.state.request_preview(&input(&p.id, &req)).unwrap();
    assert_eq!(prev.url, "http://localhost:8080/x?key=••••&q=Netflix");
    assert!(prev.unresolved.is_empty());
    assert!(!prev.url.contains("KEY-123"));
}

#[tokio::test]
async fn precedence_override_then_environment_then_global() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    let global = f.state.db().env_global(&p.id).unwrap();
    f.state
        .env_save(&with_vars(
            global,
            vec![
                plain("who", "global"),
                plain("g", "G"),
                plain("both", "global"),
            ],
        ))
        .unwrap();
    f.state
        .env_save(&with_vars(
            env,
            vec![
                plain("who", "env"),
                plain("both", "env"),
                EnvVar {
                    enabled: false,
                    ..plain("g", "disabled-env-value")
                },
            ],
        ))
        .unwrap();

    let mut req = get_request(&p.id, "http://h/{{who}}/{{both}}/{{g}}");
    let mut inp = input(&p.id, &req);
    inp.overrides = vec![VarOverride {
        name: "who".into(),
        value: "override".into(),
    }];
    let prev = f.state.request_preview(&inp).unwrap();
    // who は上書き、both は環境、g は環境のが無効なのでグローバル
    assert_eq!(prev.url, "http://h/override/env/G");

    // 動的変数
    req.url = "http://h/{{$uuid}}".into();
    let prev = f.state.request_preview(&input(&p.id, &req)).unwrap();
    let id = prev.url.rsplit('/').next().unwrap();
    assert!(uuid::Uuid::parse_str(id).is_ok(), "{}", prev.url);
}

#[tokio::test]
async fn the_chosen_environment_wins_and_another_can_be_given_explicitly() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let a = f.state.env_create(&p.id, "A").unwrap();
    let b = f.state.env_create(&p.id, "B").unwrap();
    f.state
        .env_save(&with_vars(a.clone(), vec![plain("host", "a")]))
        .unwrap();
    f.state
        .env_save(&with_vars(b.clone(), vec![plain("host", "b")]))
        .unwrap();
    let req = get_request(&p.id, "http://{{host}}/");

    // 選んでいる環境は A（最初に作ったもの）
    assert_eq!(
        f.state.request_preview(&input(&p.id, &req)).unwrap().url,
        "http://a/"
    );
    // B を選ぶと B
    f.state.env_select(&p.id, Some(&b.id)).unwrap();
    assert_eq!(
        f.state.request_preview(&input(&p.id, &req)).unwrap().url,
        "http://b/"
    );
    // 明示すればそちらが優先
    let mut inp = input(&p.id, &req);
    inp.environment_id = Some(a.id.clone());
    assert_eq!(f.state.request_preview(&inp).unwrap().url, "http://a/");
    // 未選択なら環境の変数は使われない
    f.state.env_select(&p.id, None).unwrap();
    let prev = f.state.request_preview(&input(&p.id, &req)).unwrap();
    assert_eq!(prev.unresolved[0].name, "host");
    // 他のプロジェクトの環境は指定できない
    let other = f.state.project_create("他").unwrap();
    let oe = f.state.env_create(&other.id, "O").unwrap();
    let mut inp = input(&p.id, &req);
    inp.environment_id = Some(oe.id);
    assert!(matches!(
        f.state.request_preview(&inp),
        Err(AppError::NotFound(_))
    ));
    // プロジェクトがなければ NotFound
    assert!(matches!(
        f.state.request_preview(&input("nope", &req)),
        Err(AppError::NotFound(_))
    ));
}

#[tokio::test]
async fn unresolved_variables_are_delivered_as_a_typed_failure_without_sending() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let mut req = get_request(&p.id, &format!("{}/x", server.uri()));
    req.params = vec![kv(ParamIn::Query, "a", "{{nope}}")];
    req.auth = Some(AuthOverride::Bearer {
        token: "{{token}}".into(),
    });

    let err = failure(send_and_wait(&f.state, input(&p.id, &req), None).await);
    let SendError::Unresolved { vars, message } = err else {
        panic!("Unresolved のはず");
    };
    assert_eq!(vars.len(), 2);
    assert_eq!(vars[0].name, "nope");
    assert_eq!(
        vars[0].location,
        VarLocation::Param {
            location: ParamIn::Query,
            row: 1,
            part: nekoreq_core::RowPart::Value
        }
    );
    assert_eq!(vars[1].name, "token");
    assert!(message.contains("{{nope}}"));
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "送信していない"
    );
}

#[tokio::test]
async fn secrets_are_read_from_the_store_and_sent_but_never_stored_in_sqlite() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/me"))
        .and(header("authorization", "Bearer s3cr3t-token"))
        .and(header("cookie", "SESSION=sess-1"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![
                plain("baseUrl", &server.uri()),
                secret_var("token"),
                secret_var("session"),
                secret_var("unused"),
            ],
        ))
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "token", "s3cr3t-token")
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "session", "sess-1")
        .unwrap();

    let mut req = get_request(&p.id, "{{baseUrl}}/me");
    req.auth = Some(AuthOverride::Bearer {
        token: "{{token}}".into(),
    });
    req.params = vec![kv(ParamIn::Cookie, "SESSION", "{{session}}")];
    let r = success(send_and_wait(&f.state, input(&p.id, &req), None).await);
    assert_eq!(r.status, 200, "シークレットが送られている");

    // SQLite の中にシークレットの値がない（ファイルを丸ごと探す）
    let db_bytes = std::fs::read(f._dir.path().join(DB_FILE_NAME)).unwrap();
    for needle in ["s3cr3t-token", "sess-1"] {
        assert!(
            !db_bytes
                .windows(needle.len())
                .any(|w| w == needle.as_bytes()),
            "{needle} が SQLite に残っている"
        );
    }
    // 画面へ返す環境にも値は載らない
    let envs = f.state.env_list(&p.id).unwrap();
    let json = serde_json::to_string(&envs).unwrap();
    assert!(!json.contains("s3cr3t-token") && !json.contains("sess-1"));
}

#[tokio::test]
async fn only_the_secrets_a_request_uses_are_read() {
    /// 読まれたキーを覚えておくストア。
    #[derive(Default)]
    struct Spy {
        inner: MemorySecretStore,
        reads: std::sync::Mutex<Vec<String>>,
    }
    impl SecretStore for Spy {
        fn get(&self, key: &SecretKey) -> nekoreq_secrets::Result<Option<String>> {
            self.reads.lock().unwrap().push(key.name.clone());
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
    let spy = Arc::new(Spy::default());
    let state = AppState::open_with_secrets(dir.path(), spy.clone()).unwrap();
    let p = state.project_create("API").unwrap();
    let env = state.env_create(&p.id, "E").unwrap();
    state
        .env_save(&with_vars(
            env.clone(),
            vec![secret_var("a"), secret_var("b")],
        ))
        .unwrap();
    state.secret_set(&p.id, &env.id, "a", "A").unwrap();
    state.secret_set(&p.id, &env.id, "b", "B").unwrap();
    let req = get_request(&p.id, "http://h/{{a}}");
    state.request_preview(&input(&p.id, &req)).unwrap();
    assert_eq!(*spy.reads.lock().unwrap(), ["a"], "使わない b は読まない");
}

#[tokio::test]
async fn a_secret_without_a_value_is_unresolved() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(env, vec![secret_var("token")]))
        .unwrap();
    let req = get_request(&p.id, "http://h/{{token}}");
    let prev = f.state.request_preview(&input(&p.id, &req)).unwrap();
    assert_eq!(prev.unresolved[0].name, "token");
}

#[tokio::test]
async fn cancel_stops_a_slow_request_and_delivers_a_cancelled_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let req = get_request(&p.id, &format!("{}/slow", server.uri()));

    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = f
        .state
        .request_send(input(&p.id, &req), None, move |fin| {
            let _ = tx.send(fin);
        })
        .unwrap();
    assert_eq!(f.state.inner.in_flight().len(), 1);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(f.state.request_cancel(&id));
    let fin = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fin.send_id, id);
    assert!(matches!(failure(fin), SendError::Cancelled { .. }));
    assert!(!f.state.request_cancel(&id), "終わったものは止められない");
    assert!(!f.state.request_cancel("unknown"));
}

#[tokio::test]
async fn timeout_setting_is_applied() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let req = get_request(&p.id, &format!("{}/slow", server.uri()));
    let settings = SendSettings {
        timeout_ms: 200,
        ..SendSettings::default()
    };
    let err = failure(send_and_wait(&f.state, input(&p.id, &req), Some(settings)).await);
    assert!(matches!(err, SendError::Timeout { .. }), "{err:?}");
}

#[tokio::test]
async fn invalid_url_is_a_typed_failure() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let req = get_request(&p.id, "ftp://example.com/x");
    let err = failure(send_and_wait(&f.state, input(&p.id, &req), None).await);
    assert!(matches!(err, SendError::InvalidRequest { .. }), "{err:?}");
}

#[tokio::test]
async fn post_json_body_and_header_rows_are_sent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/password-login"))
        .and(header("x-trace", "t-1"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(401).set_body_raw(
            r#"{"error":{"code":"LOGIN_FAILED"}}"#,
            "application/problem+json",
        ))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![
                plain("baseUrl", &server.uri()),
                plain("email", "a@example.com"),
                secret_var("password"),
            ],
        ))
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "password", "pw-1")
        .unwrap();

    let mut req = get_request(&p.id, "{{baseUrl}}/api/v1/auth/password-login");
    req.method = "post".into();
    req.params = vec![kv(ParamIn::Header, "X-Trace", "t-1")];
    req.body = Body::Json(r#"{"email":"{{email}}","password":"{{password}}"}"#.into());
    let r = success(send_and_wait(&f.state, input(&p.id, &req), None).await);
    assert_eq!(r.status, 401);
    let received = server.received_requests().await.unwrap();
    assert_eq!(
        String::from_utf8(received[0].body.clone()).unwrap(),
        r#"{"email":"a@example.com","password":"pw-1"}"#
    );
}

#[tokio::test]
async fn response_body_can_be_saved_and_old_ones_expire() {
    let server = MockServer::start().await;
    let big: Vec<u8> = (0..255u8).cycle().take(300_000).collect();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(big.clone(), "application/octet-stream"),
        )
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let req = get_request(&p.id, &format!("{}/bin", server.uri()));

    let fin = send_and_wait(&f.state, input(&p.id, &req), None).await;
    let first_id = fin.send_id.clone();
    let r = success(fin);
    assert_eq!(r.body.encoding, nekoreq_core::BodyEncoding::Base64);
    assert_eq!(r.body.size as usize, big.len());

    let out = f._dir.path().join("saved.bin");
    f.state
        .response_save_body(&first_id, out.to_str().unwrap())
        .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), big);

    // 相対パス・知らない ID はエラー
    assert!(matches!(
        f.state.response_save_body(&first_id, "relative.bin"),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        f.state.response_save_body("nope", out.to_str().unwrap()),
        Err(AppError::NotFound(_))
    ));
    // 書き込めない場所は Io
    assert!(matches!(
        f.state
            .response_save_body(&first_id, "/nonexistent-dir/x.bin"),
        Err(AppError::Io(_))
    ));

    // 直近 KEPT_BODIES 件だけ持つ
    for _ in 0..KEPT_BODIES {
        success(send_and_wait(&f.state, input(&p.id, &req), None).await);
    }
    assert!(matches!(
        f.state.response_save_body(&first_id, out.to_str().unwrap()),
        Err(AppError::NotFound(_))
    ));
    assert_eq!(f.state.inner.bodies().len(), KEPT_BODIES);
}

// ---- 環境・シークレットの管理 ----

#[test]
fn secret_commands_validate_and_round_trip() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();

    // 宣言のない名前には書けない（先に環境でシークレットの変数として保存する）
    assert!(matches!(
        f.state.secret_set(&p.id, &env.id, "password", "hunter2"),
        Err(AppError::Validation(_))
    ));
    f.state
        .env_save(&with_vars(env.clone(), vec![secret_var("password")]))
        .unwrap();
    assert_eq!(
        f.state.secret_reveal(&p.id, &env.id, "password").unwrap(),
        None
    );
    f.state
        .secret_set(&p.id, &env.id, " password ", "hunter2")
        .unwrap();
    assert_eq!(
        f.state
            .secret_reveal(&p.id, &env.id, "password")
            .unwrap()
            .as_deref(),
        Some("hunter2")
    );
    // キーはサービス名 NekoReq/<プロジェクト ID>、アカウント名 <環境 ID>/<変数名>
    let key = SecretKey::new(&p.id, &env.id, "password");
    assert_eq!(key.service(), format!("NekoReq/{}", p.id));
    assert_eq!(f.secrets.get(&key).unwrap().as_deref(), Some("hunter2"));

    assert!(matches!(
        f.state.secret_set(&p.id, &env.id, "password", ""),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        f.state.secret_set(&p.id, &env.id, " ", "x"),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        f.state.secret_set(&p.id, "nope", "a", "x"),
        Err(AppError::NotFound(_))
    ));
    let other = f.state.project_create("他").unwrap();
    assert!(
        matches!(
            f.state.secret_set(&other.id, &env.id, "a", "x"),
            Err(AppError::NotFound(_))
        ),
        "他のプロジェクトの環境には書けない"
    );

    f.state.secret_delete(&p.id, &env.id, "password").unwrap();
    assert_eq!(
        f.state.secret_reveal(&p.id, &env.id, "password").unwrap(),
        None
    );
    f.state.secret_delete(&p.id, &env.id, "password").unwrap();
}

#[test]
fn global_environment_secrets_work_too() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let global = f.state.db().env_global(&p.id).unwrap();
    f.state
        .env_save(&with_vars(global.clone(), vec![secret_var("session")]))
        .unwrap();
    f.state
        .secret_set(&p.id, &global.id, "session", "G-SESSION")
        .unwrap();
    let req = get_request(&p.id, "http://h/?s={{session}}");
    let prev = f.state.request_preview(&input(&p.id, &req)).unwrap();
    assert_eq!(prev.url, "http://h/?s=••••");
}

#[test]
fn saving_an_environment_clears_secrets_of_removed_variables() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![secret_var("a"), secret_var("b"), secret_var("c")],
        ))
        .unwrap();
    for n in ["a", "b", "c"] {
        f.state.secret_set(&p.id, &env.id, n, "v").unwrap();
    }
    // a は消す、b は通常の変数にする、c はそのまま
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![plain("b", "now plain"), secret_var("c")],
        ))
        .unwrap();
    let get = |n: &str| f.secrets.get(&SecretKey::new(&p.id, &env.id, n)).unwrap();
    assert_eq!(get("a"), None);
    assert_eq!(get("b"), None);
    assert_eq!(get("c").as_deref(), Some("v"));
}

#[test]
fn duplicate_copies_secret_values_and_delete_removes_them() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "ローカル").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![plain("baseUrl", "http://x"), secret_var("password")],
        ))
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "password", "hunter2")
        .unwrap();

    let copy = f.state.env_duplicate(&env.id).unwrap();
    assert_eq!(copy.name, "ローカル のコピー");
    assert_eq!(
        f.state
            .secret_reveal(&p.id, &copy.id, "password")
            .unwrap()
            .as_deref(),
        Some("hunter2")
    );
    assert_eq!(f.secrets.len(), 2);

    f.state.env_delete(&copy.id).unwrap();
    assert_eq!(f.secrets.len(), 1, "複製の分だけ消える");
    assert_eq!(
        f.state
            .secret_reveal(&p.id, &env.id, "password")
            .unwrap()
            .as_deref(),
        Some("hunter2")
    );

    // プロジェクトごと消すと、残りも消える
    f.state.project_delete(&p.id).unwrap();
    assert!(f.secrets.is_empty());
}

#[test]
fn global_cannot_be_deleted_or_duplicated_through_state() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let global = f.state.db().env_global(&p.id).unwrap();
    assert!(matches!(
        f.state.env_delete(&global.id),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        f.state.env_duplicate(&global.id),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        f.state.env_select(&p.id, Some(&global.id)),
        Err(AppError::Validation(_))
    ));
    assert_eq!(f.state.env_list(&p.id).unwrap().len(), 1);
}

#[test]
fn adhoc_and_draft_through_state() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let a = f
        .state
        .adhoc_save(&get_request(&p.id, "{{baseUrl}}/x"))
        .unwrap();
    assert_eq!(f.state.adhoc_list(&p.id).unwrap(), vec![a.clone()]);
    f.state.adhoc_delete(&a.id).unwrap();
    assert!(f.state.adhoc_list(&p.id).unwrap().is_empty());
    assert!(matches!(
        f.state.adhoc_delete(&a.id),
        Err(AppError::NotFound(_))
    ));

    let key = nekoreq_core::OpKey::Id("getClientList".into());
    assert_eq!(f.state.draft_get(&p.id, &key).unwrap(), None);
    let d = f
        .state
        .draft_save(&nekoreq_core::RequestDraft {
            id: String::new(),
            project_id: p.id.clone(),
            op_key: key.clone(),
            params: vec![kv(ParamIn::Query, "q", "Netflix")],
            body: Body::None,
            auth: None,
            updated_at: String::new(),
        })
        .unwrap();
    assert_eq!(f.state.draft_get(&p.id, &key).unwrap(), Some(d));
}

// ---- 実際の API（ホストで動かしているときだけ手で流す） ----

/// ホストの API（`GET /api/v1/auth/session`）に送る。API が止まっていると失敗するので `#[ignore]`。
/// 実行: `cargo test -p nekoreq real_api -- --ignored`（コンテナから `host.docker.internal:8080` に届く）
#[tokio::test]
#[ignore = "ホストで API（localhost:8080）が動いているときだけ"]
async fn real_api_get_session() {
    let f = fixture();
    let p = f
        .state
        .project_create("日本アニメーション ライセンス契約管理システム API")
        .unwrap();
    let env = f
        .state
        .env_create(&p.id, "ローカル (Dev Container)")
        .unwrap();
    f.state
        .env_save(&with_vars(
            env,
            vec![plain("baseUrl", "http://host.docker.internal:8080")],
        ))
        .unwrap();
    let req = f
        .state
        .adhoc_save(&get_request(&p.id, "{{baseUrl}}/api/v1/auth/session"))
        .unwrap();
    let r = success(send_and_wait(&f.state, input(&p.id, &req), None).await);
    println!("status = {} {}", r.status, r.status_text);
    println!("timing = {:?}", r.timing);
    println!("connection = {:?}", r.connection);
    println!(
        "headers = {:?}",
        r.headers
            .iter()
            .map(|HeaderPair { name, .. }| name.as_str())
            .collect::<Vec<_>>()
    );
    println!("body = {}", r.body.content);
    assert!(r.status < 500);
    assert!(r.timing.connect_ms.is_some());
}
