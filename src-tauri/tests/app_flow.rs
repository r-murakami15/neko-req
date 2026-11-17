//! app（`AppState`）の結合テスト。公開 API だけを使い、Tauri は起動しない。
//! 既存の単体テスト（src-tauri/src/state/tests.rs）と重ならない観点を中心にする:
//! 送信結果のイベント相当・未解決の場所・同時送信と保持するボディ・シークレットが SQLite のファイル全体や
//! エラーメッセージに出ないこと・環境の複製と削除に伴うキーチェーンの後始末。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nekoreq_core::{
    AdHocRequest, ApiKeyIn, AuthOverride, Body, BodyEncoding, EnvVar, Environment, KeyValue,
    ParamIn, RequestDraft, RequestSpec, ResponseData, RowPart, SendError, SendOutcome,
    SendSettings, VarLocation, VarValue,
};
use nekoreq_lib::error::AppError;
use nekoreq_lib::send::{RequestFinished, RequestInput, VarOverride};
use nekoreq_lib::state::{AppState, KEPT_BODIES};
use nekoreq_secrets::{MemorySecretStore, SecretError, SecretKey, SecretStore};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- 道具 ----

struct Fixture {
    dir: tempfile::TempDir,
    state: AppState,
    secrets: Arc<MemorySecretStore>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::new());
    let state = AppState::open_with_secrets(dir.path(), secrets.clone()).unwrap();
    Fixture {
        dir,
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

fn disabled(mut v: EnvVar) -> EnvVar {
    v.enabled = false;
    v
}

fn with_vars(mut env: Environment, vars: Vec<EnvVar>) -> Environment {
    env.vars = vars;
    env
}

fn kv(location: ParamIn, name: &str, value: &str, enabled: bool) -> KeyValue {
    KeyValue {
        location,
        name: name.into(),
        value: value.into(),
        enabled,
    }
}

fn spec(url: &str) -> RequestSpec {
    RequestSpec {
        method: "GET".into(),
        url: url.into(),
        params: vec![],
        body: Body::None,
        auth: None,
    }
}

fn input(project_id: &str, spec: RequestSpec) -> RequestInput {
    RequestInput {
        project_id: project_id.into(),
        environment_id: None,
        request: spec,
        overrides: vec![],
        op_key: None,
    }
}

async fn send_and_wait(state: &AppState, input: RequestInput) -> RequestFinished {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = state
        .request_send(input, None, move |f| {
            let _ = tx.send(f);
        })
        .unwrap();
    let f = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("finished が届く")
        .unwrap();
    assert_eq!(f.send_id, id);
    f
}

fn ok(f: RequestFinished) -> ResponseData {
    match f.result {
        SendOutcome::Success { response } => response,
        SendOutcome::Failure { error } => panic!("成功のはず: {error:?}"),
    }
}

fn err(f: RequestFinished) -> SendError {
    match f.result {
        SendOutcome::Failure { error } => error,
        SendOutcome::Success { response } => panic!("失敗のはず: {}", response.status),
    }
}

/// データフォルダの全ファイル（`data.sqlite` と `-wal` `-shm` を含む）を再帰的に読む。
fn all_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            out.extend(all_files(&p));
        } else {
            out.push((p.display().to_string(), std::fs::read(&p).unwrap()));
        }
    }
    out
}

fn assert_not_in_files(dir: &Path, needles: &[&str]) {
    for (name, bytes) in all_files(dir) {
        for n in needles {
            assert!(
                !bytes.windows(n.len()).any(|w| w == n.as_bytes()),
                "{n} が {name} に残っている"
            );
        }
    }
}

/// 失敗を差し込めるストア。
#[derive(Default)]
struct Flaky {
    inner: MemorySecretStore,
    fail_get: AtomicBool,
    fail_set: AtomicBool,
    sets: Mutex<Vec<String>>,
}
impl SecretStore for Flaky {
    fn get(&self, key: &SecretKey) -> nekoreq_secrets::Result<Option<String>> {
        if self.fail_get.load(Ordering::SeqCst) {
            return Err(SecretError::Backend("ロックされています".into()));
        }
        self.inner.get(key)
    }
    fn set(&self, key: &SecretKey, value: &str) -> nekoreq_secrets::Result<()> {
        if self.fail_set.load(Ordering::SeqCst) {
            return Err(SecretError::Backend("書き込めません".into()));
        }
        self.sets.lock().unwrap().push(key.account());
        self.inner.set(key, value)
    }
    fn delete(&self, key: &SecretKey) -> nekoreq_secrets::Result<()> {
        self.inner.delete(key)
    }
}

async fn ok_server(body: &'static str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    server
}

// ---- 結果のイベント ----

#[tokio::test]
async fn finished_is_delivered_once_with_the_send_id_and_the_send_is_no_longer_cancellable_by_then()
{
    let server = ok_server("hello").await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();

    let state = f.state.clone();
    let seen_cancel = Arc::new(Mutex::new(None));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (sc, c) = (seen_cancel.clone(), calls.clone());
    let id = f
        .state
        .request_send(
            input(&p.id, spec(&format!("{}/x", server.uri()))),
            None,
            move |fin| {
                c.fetch_add(1, Ordering::SeqCst);
                // イベントが届いた時点で、中止の対象からは外れている
                *sc.lock().unwrap() = Some(state.request_cancel(&fin.send_id));
                let _ = tx.send(fin);
            },
        )
        .unwrap();
    let fin = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fin.send_id, id);
    assert_eq!(*seen_cancel.lock().unwrap(), Some(false));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "1 回だけ届く");
    assert_eq!(ok(fin).body.content, "hello");
}

#[tokio::test]
async fn the_finished_event_has_the_wire_shape_the_screen_expects() {
    let server = ok_server("{}").await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let fin = send_and_wait(&f.state, input(&p.id, spec(&server.uri()))).await;
    let v = serde_json::to_value(&fin).unwrap();
    assert_eq!(v["sendId"], fin.send_id);
    assert_eq!(v["result"]["status"], "success");
    assert_eq!(v["result"]["response"]["status"], 200);
    assert!(
        v["result"]["response"]["timing"]["totalMs"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert_eq!(v["result"]["response"]["body"]["encoding"], "text");

    let fin = send_and_wait(&f.state, input(&p.id, spec("{{nope}}/x"))).await;
    let v = serde_json::to_value(&fin).unwrap();
    assert_eq!(v["result"]["status"], "failure");
    assert_eq!(v["result"]["error"]["kind"], "unresolved");
    assert_eq!(v["result"]["error"]["vars"][0]["name"], "nope");
    assert_eq!(v["result"]["error"]["vars"][0]["location"]["kind"], "url");
}

// ---- 未解決 ----

#[tokio::test]
async fn unresolved_failure_arrives_without_any_network_traffic_and_keeps_no_body() {
    let server = ok_server("x").await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let mut s = spec(&format!("{}/x", server.uri()));
    s.params = vec![
        kv(ParamIn::Header, "X-Off", "{{off}}", false),
        kv(ParamIn::Header, "X-On", "{{on}}", true),
        kv(ParamIn::Query, "a", "1", false),
        kv(ParamIn::Query, "b", "{{q}}", true),
    ];
    let t0 = std::time::Instant::now();
    let fin = send_and_wait(&f.state, input(&p.id, s)).await;
    assert!(t0.elapsed() < Duration::from_secs(1));
    let id = fin.send_id.clone();
    let SendError::Unresolved { vars, message } = err(fin) else {
        panic!()
    };
    let loc = |location, row| VarLocation::Param {
        location,
        row,
        part: RowPart::Value,
    };
    assert_eq!(vars.len(), 2, "{vars:?}");
    let find = |n: &str| {
        vars.iter()
            .find(|v| v.name == n)
            .unwrap_or_else(|| panic!("{n} がない: {vars:?}"))
            .location
            .clone()
    };
    assert_eq!(find("on"), loc(ParamIn::Header, 2), "無効の行も数える");
    assert_eq!(find("q"), loc(ParamIn::Query, 2));
    assert!(
        message.contains("{{on}}（ヘッダー 2 行目の値）"),
        "{message}"
    );
    assert!(!message.contains("{{off}}"));
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(matches!(
        f.state
            .response_save_body(&id, f.dir.path().join("x").to_str().unwrap()),
        Err(AppError::NotFound(_))
    ));
}

#[tokio::test]
async fn unresolved_in_a_form_row_after_a_disabled_row_reports_the_visual_row_number() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let mut s = spec("http://127.0.0.1:9/");
    s.method = "POST".into();
    s.body = Body::Form(vec![
        kv(ParamIn::Query, "a", "1", false),
        kv(ParamIn::Query, "b", "2", false),
        kv(ParamIn::Query, "c", "{{x}}", true),
    ]);
    let SendError::Unresolved { vars, .. } = err(send_and_wait(&f.state, input(&p.id, s)).await)
    else {
        panic!()
    };
    assert_eq!(
        vars[0].location,
        VarLocation::BodyForm {
            row: 3,
            part: RowPart::Value
        }
    );
}

#[tokio::test]
async fn disabled_variables_and_secrets_without_values_fall_through_to_the_next_layer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/g-url/g-tok"))
        .respond_with(ResponseTemplate::new(200).set_body_string("layers"))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let global = f.state.env_list(&p.id).unwrap().remove(0);
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(
            global.clone(),
            vec![plain("seg", "g-url"), secret_var("tok")],
        ))
        .unwrap();
    f.state
        .secret_set(&p.id, &global.id, "tok", "g-tok")
        .unwrap();
    // 選んでいる環境: seg は無効、tok は有効なシークレットだが値なし
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![disabled(plain("seg", "e-url")), secret_var("tok")],
        ))
        .unwrap();

    let s = spec(&format!("{}/{{{{seg}}}}/{{{{tok}}}}", server.uri()));
    let fin = send_and_wait(&f.state, input(&p.id, s.clone())).await;
    assert_eq!(ok(fin).body.content, "layers");

    // 環境側に値を入れると環境が勝つ（もう路が合わない）。無効のシークレットは値があっても使わない
    f.state.secret_set(&p.id, &env.id, "tok", "e-tok").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![disabled(plain("seg", "e-url")), disabled(secret_var("tok"))],
        ))
        .unwrap();
    let fin = send_and_wait(&f.state, input(&p.id, s)).await;
    assert_eq!(
        ok(fin).body.content,
        "layers",
        "無効のシークレットは読まない"
    );
}

#[tokio::test]
async fn nothing_resolves_when_no_environment_is_selected_except_globals_and_overrides() {
    let server = ok_server("o").await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let g = f.state.env_list(&p.id).unwrap().remove(0);
    f.state
        .env_save(&with_vars(g, vec![plain("base", &server.uri())]))
        .unwrap();
    let mut i = input(&p.id, spec("{{base}}/{{o}}"));
    // 「o」だけ未解決。空の名前の上書きは無視、名前は前後の空白を除く
    i.overrides = vec![
        VarOverride {
            name: "  ".into(),
            value: "x".into(),
        },
        VarOverride {
            name: " other ".into(),
            value: "y".into(),
        },
    ];
    let SendError::Unresolved { vars, .. } = err(send_and_wait(&f.state, i.clone()).await) else {
        panic!()
    };
    assert_eq!(
        vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        ["o"]
    );
    i.overrides.push(VarOverride {
        name: " o ".into(),
        value: String::new(),
    });
    assert_eq!(
        ok(send_and_wait(&f.state, i).await).status,
        200,
        "空文字の上書きも「値がある」"
    );
}

#[tokio::test]
async fn project_and_environment_errors_are_returned_synchronously_and_nothing_is_started() {
    let f = fixture();
    let p1 = f.state.project_create("P1").unwrap();
    let p2 = f.state.project_create("P2").unwrap();
    let e2 = f.state.env_create(&p2.id, "e").unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let c = called.clone();
    let r = f
        .state
        .request_send(input("missing", spec("http://h/")), None, move |_| {
            c.store(true, Ordering::SeqCst)
        });
    assert!(matches!(r, Err(AppError::NotFound(_))), "{r:?}");
    let mut i = input(&p1.id, spec("http://h/"));
    i.environment_id = Some(e2.id.clone());
    let c = called.clone();
    assert!(
        matches!(
            f.state
                .request_send(i.clone(), None, move |_| c.store(true, Ordering::SeqCst)),
            Err(AppError::NotFound(_))
        ),
        "別のプロジェクトの環境"
    );
    assert!(matches!(
        f.state.request_preview(&i),
        Err(AppError::NotFound(_))
    ));
    i.environment_id = Some("nope".into());
    assert!(matches!(
        f.state.request_preview(&i),
        Err(AppError::NotFound(_))
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn naming_the_global_environment_explicitly_works_like_not_choosing_one() {
    let server = ok_server("g").await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let g = f.state.env_list(&p.id).unwrap().remove(0);
    f.state
        .env_save(&with_vars(g.clone(), vec![plain("base", &server.uri())]))
        .unwrap();
    let mut i = input(&p.id, spec("{{base}}/"));
    i.environment_id = Some(g.id);
    assert_eq!(ok(send_and_wait(&f.state, i).await).body.content, "g");
}

#[tokio::test]
async fn selection_changes_which_environment_is_used_and_deleting_the_selected_one_reselects() {
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    for (srv, body) in [(&a, "from-a"), (&b, "from-b")] {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(srv)
            .await;
    }
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let ea = f.state.env_create(&p.id, "a").unwrap();
    let eb = f.state.env_create(&p.id, "b").unwrap();
    f.state
        .env_save(&with_vars(ea.clone(), vec![plain("base", &a.uri())]))
        .unwrap();
    f.state
        .env_save(&with_vars(eb.clone(), vec![plain("base", &b.uri())]))
        .unwrap();
    let get = || async {
        ok(send_and_wait(&f.state, input(&p.id, spec("{{base}}/"))).await)
            .body
            .content
    };
    assert_eq!(get().await, "from-a", "最初に作った環境");
    f.state.env_select(&p.id, Some(&eb.id)).unwrap();
    assert_eq!(get().await, "from-b");
    f.state.env_delete(&eb.id).unwrap();
    assert_eq!(get().await, "from-a", "選び直される");
    f.state.env_select(&p.id, None).unwrap();
    let SendError::Unresolved { .. } =
        err(send_and_wait(&f.state, input(&p.id, spec("{{base}}/"))).await)
    else {
        panic!()
    };
}

// ---- キーチェーンの失敗 ----

#[tokio::test]
async fn a_keychain_failure_is_a_synchronous_secret_error_and_nothing_is_sent() {
    let server = ok_server("x").await;
    let flaky = Arc::new(Flaky::default());
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open_with_secrets(dir.path(), flaky.clone()).unwrap();
    let p = state.project_create("API").unwrap();
    let env = state.env_create(&p.id, "e").unwrap();
    state
        .env_save(&with_vars(
            env.clone(),
            vec![secret_var("tok"), plain("base", &server.uri())],
        ))
        .unwrap();
    state
        .secret_set(&p.id, &env.id, "tok", "SECRET-TOKEN")
        .unwrap();
    let mut s = spec("{{base}}/");
    s.auth = Some(AuthOverride::Bearer {
        token: "{{tok}}".into(),
    });
    flaky.fail_get.store(true, Ordering::SeqCst);
    let r = state.request_send(input(&p.id, s.clone()), None, |_| {});
    let Err(AppError::Secret(msg)) = r else {
        panic!("{r:?}")
    };
    assert!(msg.contains("ロック"), "{msg}");
    assert!(!msg.contains("SECRET-TOKEN"));
    assert!(matches!(
        state.request_preview(&input(&p.id, s.clone())),
        Err(AppError::Secret(_))
    ));
    // シークレットを使わないリクエストは影響を受けない
    assert_eq!(
        ok(send_and_wait(&state, input(&p.id, spec("{{base}}/"))).await).status,
        200
    );
    flaky.fail_get.store(false, Ordering::SeqCst);
    assert_eq!(ok(send_and_wait(&state, input(&p.id, s)).await).status, 200);
    assert!(server.received_requests().await.unwrap().len() == 2);
}

// ---- 同時送信と保持するボディ ----

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sends_are_delivered_by_send_id_and_only_the_latest_eight_bodies_are_kept() {
    let server = MockServer::start().await;
    for i in 0..12u32 {
        Mock::given(method("GET"))
            .and(path(format!("/n/{i}")))
            .respond_with(
                ResponseTemplate::new(200)
                    // 完了の順を番号の順にする（保持の対象は「完了の新しい 8 件」）
                    .set_delay(Duration::from_millis(u64::from(100 + i * 60)))
                    .set_body_raw(format!("body-{i}").into_bytes(), "application/octet-stream"),
            )
            .mount(&server)
            .await;
    }
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let results: Arc<Mutex<Vec<RequestFinished>>> = Arc::default();
    let mut ids = Vec::new();
    for i in 0..12u32 {
        let r = results.clone();
        let id = f
            .state
            .request_send(
                input(&p.id, spec(&format!("{}/n/{i}", server.uri()))),
                None,
                move |fin| r.lock().unwrap().push(fin),
            )
            .unwrap();
        ids.push(id);
    }
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 12);
    tokio::time::timeout(Duration::from_secs(10), async {
        while results.lock().unwrap().len() < 12 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let done = results.lock().unwrap().clone();
    for (i, id) in ids.iter().enumerate() {
        let fin = done
            .iter()
            .find(|d| &d.send_id == id)
            .expect("各 ID に結果が 1 つ");
        let SendOutcome::Success { response } = &fin.result else {
            panic!()
        };
        assert_eq!(
            response.final_url,
            format!("{}/n/{i}", server.uri()),
            "結果が別の送信に届いていない"
        );
        assert_eq!(response.body.encoding, BodyEncoding::Base64);
    }
    // 保持は直近 8 件（完了順 = 番号順なので 4..12）
    for (i, id) in ids.iter().enumerate() {
        let out = f.dir.path().join(format!("out-{i}.bin"));
        let r = f.state.response_save_body(id, out.to_str().unwrap());
        if i < 12 - KEPT_BODIES {
            assert!(matches!(r, Err(AppError::NotFound(_))), "{i}: {r:?}");
        } else {
            r.unwrap_or_else(|e| panic!("{i}: {e:?}"));
            assert_eq!(
                std::fs::read(&out).unwrap(),
                format!("body-{i}").into_bytes()
            );
        }
    }
}

#[tokio::test]
async fn failed_and_unresolved_sends_do_not_push_out_kept_bodies() {
    let server = ok_server("kept").await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let first = send_and_wait(&f.state, input(&p.id, spec(&server.uri())))
        .await
        .send_id;
    for _ in 0..KEPT_BODIES + 2 {
        err(send_and_wait(&f.state, input(&p.id, spec("{{missing}}/"))).await);
        let e = err(send_and_wait(&f.state, input(&p.id, spec("http://127.0.0.1:1/"))).await);
        assert!(matches!(e, SendError::Connect { .. }), "{e:?}");
    }
    let out = f.dir.path().join("kept.txt");
    f.state
        .response_save_body(&first, out.to_str().unwrap())
        .unwrap();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "kept");
}

#[tokio::test]
async fn a_still_running_send_cannot_be_saved_yet() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(400)))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = f
        .state
        .request_send(input(&p.id, spec(&server.uri())), None, move |fin| {
            drop(tx.send(fin))
        })
        .unwrap();
    let out = f.dir.path().join("x");
    assert!(matches!(
        f.state.response_save_body(&id, out.to_str().unwrap()),
        Err(AppError::NotFound(_))
    ));
    rx.await.unwrap();
    f.state
        .response_save_body(&id, out.to_str().unwrap())
        .unwrap();
}

#[tokio::test]
async fn a_body_over_ten_megabytes_is_cut_for_the_screen_but_saved_whole_and_overwrites_existing_files(
) {
    let n = 10 * 1024 * 1024 + 4321;
    let server = MockServer::start().await;
    let data: Vec<u8> = (0..=255u8).cycle().take(n).collect();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(data.clone(), "application/octet-stream"),
        )
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let fin = send_and_wait(&f.state, input(&p.id, spec(&server.uri()))).await;
    let id = fin.send_id.clone();
    let r = ok(fin);
    assert!(r.body.truncated);
    assert_eq!(r.body.size as usize, n);
    let out = f.dir.path().join("big.bin");
    std::fs::write(&out, b"old").unwrap();
    f.state
        .response_save_body(&id, out.to_str().unwrap())
        .unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), data);
    // 2 回目も保存できる（読み出しでボディは消えない）。ディレクトリを指すと Io
    f.state
        .response_save_body(&id, out.to_str().unwrap())
        .unwrap();
    assert!(matches!(
        f.state
            .response_save_body(&id, f.dir.path().to_str().unwrap()),
        Err(AppError::Io(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_one_of_several_sends_leaves_the_others_untouched() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(500))
                .set_body_string("done"),
        )
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let mut rxs = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let (tx, rx) = tokio::sync::oneshot::channel();
        ids.push(
            f.state
                .request_send(input(&p.id, spec(&server.uri())), None, move |fin| {
                    drop(tx.send(fin))
                })
                .unwrap(),
        );
        rxs.push(rx);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(f.state.request_cancel(&ids[1]));
    for (i, rx) in rxs.into_iter().enumerate() {
        let fin = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fin.send_id, ids[i]);
        if i == 1 {
            assert!(matches!(err(fin), SendError::Cancelled { .. }));
        } else {
            assert_eq!(ok(fin).body.content, "done");
        }
    }
    assert!(!f.state.request_cancel(&ids[1]), "終わったので false");
}

// ---- シークレットが出ない ----

#[tokio::test]
async fn secrets_never_reach_any_file_in_the_data_directory_through_a_full_workflow() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    let global = f.state.env_list(&p.id).unwrap().remove(0);
    let secrets = [
        ("tok", "SECRET-BEARER-aaa111"),
        ("pw", "SECRET-PASS word/bbb222"),
        ("ck", "SECRET-COOKIE-ccc333"),
        ("kq", "SECRET-QUERY&=ddd444"),
        ("jb", "SECRET-JSON-eee555"),
    ];
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![
                plain("base", &server.uri()),
                secret_var("tok"),
                secret_var("pw"),
                secret_var("ck"),
                secret_var("kq"),
            ],
        ))
        .unwrap();
    f.state
        .env_save(&with_vars(global.clone(), vec![secret_var("jb")]))
        .unwrap();
    for (n, v) in &secrets[..4] {
        f.state.secret_set(&p.id, &env.id, n, v).unwrap();
    }
    f.state
        .secret_set(&p.id, &global.id, "jb", secrets[4].1)
        .unwrap();
    let copy = f.state.env_duplicate(&env.id).unwrap();

    let mut s = spec("{{base}}/x?token={{tok}}");
    s.method = "POST".into();
    s.params = vec![
        kv(ParamIn::Cookie, "SESSION", "{{ck}}", true),
        kv(ParamIn::Header, "X-Key", "{{kq}}", true),
    ];
    s.body = Body::Json(r#"{"secret":"{{jb}}"}"#.into());
    s.auth = Some(AuthOverride::Basic {
        username: "alice".into(),
        password: "{{pw}}".into(),
    });
    ok(send_and_wait(&f.state, input(&p.id, s.clone())).await);
    f.state.request_preview(&input(&p.id, s.clone())).unwrap();
    // 保存系: 手で作ったリクエストと入力値（変数の参照だけが入る）
    let adhoc = AdHocRequest {
        id: String::new(),
        project_id: p.id.clone(),
        name: "n".into(),
        description: String::new(),
        method: "POST".into(),
        url: s.url.clone(),
        params: s.params.clone(),
        body: s.body.clone(),
        auth: s.auth.clone(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    f.state.adhoc_save(&adhoc).unwrap();
    f.state
        .draft_save(&RequestDraft {
            id: String::new(),
            project_id: p.id.clone(),
            op_key: nekoreq_core::OpKey::Id("op1".into()),
            params: s.params.clone(),
            body: s.body.clone(),
            auth: s.auth.clone(),
            updated_at: String::new(),
        })
        .unwrap();
    // 環境の保存し直し・複製の保存・変数の削除
    f.state
        .env_save(&with_vars(copy.clone(), vec![secret_var("tok")]))
        .unwrap();
    f.state.env_delete(&copy.id).unwrap();

    let needles: Vec<&str> = secrets.iter().map(|(_, v)| *v).collect();
    assert_not_in_files(f.dir.path(), &needles);
    // キーチェーン側にはある
    assert_eq!(
        f.state
            .secret_reveal(&p.id, &env.id, "tok")
            .unwrap()
            .as_deref(),
        Some(secrets[0].1)
    );
}

#[tokio::test]
async fn preview_masks_every_secret_form_in_the_url_and_headers() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![
                plain("base", "http://h"),
                secret_var("a"),
                secret_var("b"),
                secret_var("c"),
            ],
        ))
        .unwrap();
    let (a, b, c) = ("A secret/1&=", "日本語のひみつ", "Cc+Dd==");
    for (n, v) in [("a", a), ("b", b), ("c", c)] {
        f.state.secret_set(&p.id, &env.id, n, v).unwrap();
    }
    let mut s = spec("{{base}}/{id}");
    s.params = vec![
        kv(ParamIn::Path, "id", "{{a}}", true),
        kv(ParamIn::Query, "q", "{{b}}", true),
        kv(ParamIn::Cookie, "c", "{{c}}", true),
        kv(ParamIn::Header, "X-A", "pre-{{a}}-post", true),
    ];
    s.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Query,
        name: "k".into(),
        value: "{{c}}".into(),
    });
    let pv = f.state.request_preview(&input(&p.id, s)).unwrap();
    let json = serde_json::to_string(&pv).unwrap();
    for secret in [a, b, c] {
        let q = percent_encoding_all(secret);
        assert!(!json.contains(secret), "{secret}: {json}");
        assert!(!json.contains(&q), "{q}: {json}");
    }
    assert!(pv.url.contains("••••"));
    assert!(pv.unresolved.is_empty());
}

fn percent_encoding_all(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn send_failure_messages_do_not_contain_secrets_from_the_url() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(env.clone(), vec![secret_var("tok")]))
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "tok", "LEAK-ME-12345")
        .unwrap();
    let e = err(send_and_wait(
        &f.state,
        input(&p.id, spec("http://127.0.0.1:1/x?token={{tok}}")),
    )
    .await);
    assert!(matches!(e, SendError::Connect { .. }), "{e:?}");
    assert!(!e.message().contains("LEAK-ME-12345"), "{}", e.message());
}

// ---- 環境・シークレットの後始末 ----

#[test]
fn duplicating_an_environment_whose_secret_has_no_value_copies_nothing_and_still_succeeds() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(env.clone(), vec![secret_var("empty")]))
        .unwrap();
    let copy = f.state.env_duplicate(&env.id).unwrap();
    assert_eq!(copy.vars, env_vars_of(&f.state, &env));
    assert!(f.secrets.is_empty());
}

fn env_vars_of(state: &AppState, env: &Environment) -> Vec<EnvVar> {
    state
        .env_list(&env.project_id)
        .unwrap()
        .into_iter()
        .find(|e| e.id == env.id)
        .unwrap()
        .vars
}

#[test]
fn a_failed_secret_copy_rolls_the_duplicate_back_completely() {
    let flaky = Arc::new(Flaky::default());
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open_with_secrets(dir.path(), flaky.clone()).unwrap();
    let p = state.project_create("API").unwrap();
    let env = state.env_create(&p.id, "E").unwrap();
    state
        .env_save(&with_vars(
            env.clone(),
            vec![secret_var("a"), secret_var("b")],
        ))
        .unwrap();
    state.secret_set(&p.id, &env.id, "a", "1").unwrap();
    state.secret_set(&p.id, &env.id, "b", "2").unwrap();
    let before = state.env_list(&p.id).unwrap();
    flaky.fail_set.store(true, Ordering::SeqCst);
    let r = state.env_duplicate(&env.id);
    assert!(matches!(r, Err(AppError::Secret(_))), "{r:?}");
    assert_eq!(
        state.env_list(&p.id).unwrap(),
        before,
        "複製した環境が残っていない"
    );
    assert_eq!(
        flaky.inner.len(),
        2,
        "元のシークレットはそのまま、複製分の取りこぼしもない"
    );
}

#[test]
fn renaming_a_secret_variable_drops_its_value() {
    // 名前を変えるとキーチェーンのキーが変わるので、古い値は消え、新しい名前には値がない。
    // 画面が「値を入れ直してください」と案内する必要がある（現状の挙動の記録）
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(env.clone(), vec![secret_var("token")]))
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "token", "VALUE")
        .unwrap();
    f.state
        .env_save(&with_vars(env.clone(), vec![secret_var("apiToken")]))
        .unwrap();
    assert_eq!(
        f.state.secret_reveal(&p.id, &env.id, "token").unwrap(),
        None
    );
    assert_eq!(
        f.state.secret_reveal(&p.id, &env.id, "apiToken").unwrap(),
        None
    );
    assert!(f.secrets.is_empty());
}

#[test]
fn deleting_a_project_clears_secrets_of_every_environment_including_global_and_leaves_other_projects(
) {
    let f = fixture();
    let p1 = f.state.project_create("P1").unwrap();
    let p2 = f.state.project_create("P2").unwrap();
    for p in [&p1, &p2] {
        let g = f.state.env_list(&p.id).unwrap().remove(0);
        let e = f.state.env_create(&p.id, "e").unwrap();
        f.state
            .env_save(&with_vars(g.clone(), vec![secret_var("g")]))
            .unwrap();
        f.state
            .env_save(&with_vars(e.clone(), vec![secret_var("s")]))
            .unwrap();
        f.state.secret_set(&p.id, &g.id, "g", "G").unwrap();
        f.state.secret_set(&p.id, &e.id, "s", "S").unwrap();
    }
    assert_eq!(f.secrets.len(), 4);
    f.state.project_delete(&p1.id).unwrap();
    assert_eq!(f.secrets.len(), 2);
    let e2 = f.state.env_list(&p2.id).unwrap().remove(1);
    assert_eq!(
        f.state
            .secret_reveal(&p2.id, &e2.id, "s")
            .unwrap()
            .as_deref(),
        Some("S")
    );
}

#[test]
fn secret_set_only_accepts_names_declared_as_secret_variables_of_that_environment() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(
            env.clone(),
            vec![plain("declared_plain", "x"), secret_var("real")],
        ))
        .unwrap();
    // 宣言のない名前・通常の変数の名前には書けない（キーチェーンに取り残されるため）
    for name in ["ghost", "declared_plain"] {
        let e = f.state.secret_set(&p.id, &env.id, name, "G").unwrap_err();
        assert!(matches!(e, AppError::Validation(_)), "{name}: {e:?}");
    }
    assert!(f.secrets.is_empty());
    // 宣言したシークレットには書ける。環境を消せばキーチェーンも空になる
    f.state.secret_set(&p.id, &env.id, "real", "R").unwrap();
    assert_eq!(f.secrets.len(), 1);
    // 別の環境の宣言では書けない
    let other = f.state.env_create(&p.id, "Other").unwrap();
    assert!(matches!(
        f.state.secret_set(&p.id, &other.id, "real", "R"),
        Err(AppError::Validation(_))
    ));
    f.state.env_delete(&env.id).unwrap();
    assert!(
        f.secrets.is_empty(),
        "取り残されたシークレットが {} 件ある",
        f.secrets.len()
    );
}

#[test]
fn secret_commands_reject_bad_input_without_touching_the_store() {
    let f = fixture();
    let p1 = f.state.project_create("P1").unwrap();
    let p2 = f.state.project_create("P2").unwrap();
    let e1 = f.state.env_create(&p1.id, "e").unwrap();
    assert!(
        matches!(
            f.state.secret_set(&p2.id, &e1.id, "a", "v"),
            Err(AppError::NotFound(_))
        ),
        "別のプロジェクトの環境"
    );
    assert!(matches!(
        f.state.secret_set(&p1.id, "nope", "a", "v"),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        f.state.secret_set(&p1.id, &e1.id, "   ", "v"),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        f.state.secret_set(&p1.id, &e1.id, "a", ""),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        f.state.secret_reveal(&p2.id, &e1.id, "a"),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        f.state.secret_delete(&p2.id, &e1.id, "a"),
        Err(AppError::NotFound(_))
    ));
    // 宣言のない名前（シークレットの変数でない名前）は書けない
    assert!(matches!(
        f.state.secret_set(&p1.id, &e1.id, "a", "v"),
        Err(AppError::Validation(_))
    ));
    assert!(f.secrets.is_empty());
    f.state
        .env_save(&with_vars(e1.clone(), vec![secret_var("a")]))
        .unwrap();
    // 前後の空白は取る・値の前後の空白はそのまま
    f.state
        .secret_set(&p1.id, &e1.id, "  a  ", "  padded  ")
        .unwrap();
    assert_eq!(
        f.state
            .secret_reveal(&p1.id, &e1.id, "a")
            .unwrap()
            .as_deref(),
        Some("  padded  ")
    );
}

#[test]
fn the_secret_value_is_never_part_of_what_the_screen_receives_from_environment_commands() {
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let env = f.state.env_create(&p.id, "E").unwrap();
    f.state
        .env_save(&with_vars(env.clone(), vec![secret_var("tok")]))
        .unwrap();
    f.state
        .secret_set(&p.id, &env.id, "tok", "UI-VISIBLE-SECRET")
        .unwrap();
    let copy = f.state.env_duplicate(&env.id).unwrap();
    let saved = f.state.env_save(&env).unwrap();
    let selected = f.state.env_select(&p.id, Some(&copy.id)).unwrap();
    let json = serde_json::to_string(&(
        f.state.env_list(&p.id).unwrap(),
        copy,
        saved,
        selected,
        f.state.project_list().unwrap(),
    ))
    .unwrap();
    assert!(!json.contains("UI-VISIBLE-SECRET"), "{json}");
}

#[tokio::test]
async fn send_settings_default_is_used_when_none_is_given_and_a_custom_timeout_applies() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(600)))
        .mount(&server)
        .await;
    let f = fixture();
    let p = f.state.project_create("API").unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    f.state
        .request_send(
            input(&p.id, spec(&server.uri())),
            Some(SendSettings {
                timeout_ms: 150,
                ..SendSettings::default()
            }),
            move |fin| drop(tx.send(fin)),
        )
        .unwrap();
    assert!(matches!(err(rx.await.unwrap()), SendError::Timeout { .. }));
    assert_eq!(
        ok(send_and_wait(&f.state, input(&p.id, spec(&server.uri()))).await).status,
        200
    );
}
