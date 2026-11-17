//! OpenAPI の取り込み・同期・API ツリー・送信（M3・M4）を `AppState` 経由で通しで確かめる。Tauri は起動しない。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nekoreq_core::{
    AuthOverride, Body, EnvVar, ImportOptions, KeyValue, OpKey, ParamIn, RequestDraft, RequestSpec,
    SendOutcome, SourceKind, VarValue,
};
use nekoreq_lib::openapi::{
    ImportRequest, ImportResult, PreviewResult, SourceInput, SyncApplyResult, SyncCheckResult,
    UploadInput,
};
use nekoreq_lib::send::{RequestFinished, RequestInput, VarOverride};
use nekoreq_lib::state::AppState;
use nekoreq_openapi::{ChangedField, LoadProblem};
use nekoreq_secrets::{MemorySecretStore, SecretKey, SecretStore};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---- 道具 ----

struct Fx {
    _dir: tempfile::TempDir,
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
        _dir: dir,
        data,
        state,
        secrets,
    }
}

fn openapi_fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/openapi/tests/fixtures")
        .join(rel)
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

/// 編集できるコピー（分割された見本）。
fn editable_split() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&openapi_fixture("split"), tmp.path());
    let entry = tmp.path().join("openapi.yaml");
    (tmp, entry)
}

fn path_input(p: &Path) -> SourceInput {
    SourceInput::Path {
        path: p.display().to_string(),
        allowed_hosts: vec![],
    }
}

fn import_req(source: SourceInput) -> ImportRequest {
    ImportRequest {
        source,
        project_id: None,
        project_name: None,
        selected: None,
        options: ImportOptions::default(),
    }
}

async fn import_new(f: &Fx, source: SourceInput) -> nekoreq_core::Project {
    match f.state.openapi_import(import_req(source)).await.unwrap() {
        ImportResult::Imported { project, .. } => *project,
        other => panic!("取り込めるはず: {other:?}"),
    }
}

async fn send_and_wait(state: &AppState, input: RequestInput) -> RequestFinished {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .request_send(input, None, move |f| {
            let _ = tx.send(f);
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("finished")
        .unwrap()
}

fn ok_response(f: RequestFinished) -> nekoreq_core::ResponseData {
    match f.result {
        SendOutcome::Success { response } => response,
        SendOutcome::Failure { error } => panic!("成功のはず: {error:?}"),
    }
}

fn input_for(project_id: &str, key: &OpKey, request: RequestSpec, base_url: &str) -> RequestInput {
    RequestInput {
        project_id: project_id.into(),
        environment_id: None,
        request,
        overrides: vec![VarOverride {
            name: "baseUrl".into(),
            value: base_url.into(),
        }],
        op_key: Some(key.clone()),
    }
}

fn key(id: &str) -> OpKey {
    OpKey::Id(id.into())
}

// ---- 取り込みから送信まで ----

#[tokio::test]
async fn import_list_open_and_send_through_app_state() {
    let f = fx();
    let (_tmp, entry) = editable_split();

    // 確認（何も保存しない）
    let PreviewResult::Ready { preview } = f
        .state
        .openapi_preview(path_input(&entry), None)
        .await
        .unwrap()
    else {
        panic!("確認画面のデータが返るはず")
    };
    assert_eq!(preview.operation_count, 6);
    assert_eq!(preview.file_count, 5);
    assert_eq!(preview.title, "Sample API");
    assert!(
        f.state.project_list().unwrap().is_empty(),
        "確認では保存しない"
    );

    // 取り込み
    let ImportResult::Imported {
        project,
        operation_count,
        created_environments,
        report,
        diff,
    } = f
        .state
        .openapi_import(import_req(path_input(&entry)))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(operation_count, 6);
    assert_eq!(report.added, 6);
    assert!(diff.is_none());
    assert_eq!(created_environments, vec!["ローカル", "本番"]);
    assert_eq!(project.name, "Sample API");
    let source = project.source.as_ref().unwrap();
    assert_eq!(source.kind, SourceKind::Path);
    assert_eq!(source.files.len(), 5);
    assert_eq!(source.openapi_version, "3.0.3");
    assert_eq!(source.meta.servers.len(), 2);
    // servers から環境ができ、最初の環境が選ばれる（baseUrl 変数）
    let envs = f.state.env_list(&project.id).unwrap();
    assert_eq!(envs.len(), 3, "グローバル + 2");
    let local = envs.iter().find(|e| e.name == "ローカル").unwrap();
    assert_eq!(local.vars[0].name, "baseUrl");
    assert_eq!(
        local.vars[0].value,
        VarValue::Plain("http://localhost:8080".into())
    );
    assert_eq!(
        project.default_environment_id.as_deref(),
        Some(local.id.as_str())
    );

    // API ツリー
    let list = f.state.operation_list(&project.id).unwrap();
    assert_eq!(list.len(), 6);
    let users: Vec<_> = list
        .iter()
        .filter(|i| i.folder.as_deref() == Some("users"))
        .map(|i| i.op_key.to_string())
        .collect();
    assert_eq!(
        users,
        vec![
            "listUsers",
            "createUser",
            "getUser",
            "DELETE /users/{userId}"
        ]
    );
    let health = list.iter().find(|i| i.path == "/health").unwrap();
    assert!(!health.requires_auth && health.folder.is_none());
    assert!(
        list.iter()
            .find(|i| i.op_key == key("listUsers"))
            .unwrap()
            .requires_auth
    );

    // 送信タブ（初期値は無効の状態）
    let open = f
        .state
        .operation_open(&project.id, &key("listUsers"))
        .unwrap();
    assert_eq!(open.request.url, "{{baseUrl}}/users");
    assert!(open.request.params.iter().all(|p| !p.enabled));
    assert!(!open.has_draft);

    // 送信: cookieAuth の既定の認証は、環境変数 session があるときだけ付く
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .and(query_param("q", "foo"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"items\":[]}"))
        .mount(&server)
        .await;
    let mut req = open.request.clone();
    req.params[0].enabled = true; // q=foo
    let input = input_for(&project.id, &key("listUsers"), req.clone(), &server.uri());

    // session が未定義 → 認証なしで送る（展開できない変数のエラーにはしない）
    let resp = ok_response(send_and_wait(&f.state, input.clone()).await);
    assert_eq!(resp.status, 200);
    assert!(server.received_requests().await.unwrap()[0]
        .headers
        .get("cookie")
        .is_none());
    let preview = f.state.request_preview(&input).unwrap();
    assert!(preview.unresolved.is_empty());
    assert!(preview.auto_headers.iter().all(|h| h.name != "Cookie"));

    // session をシークレットの環境変数として持つと、Cookie: SESSION=… が付く
    let mut env = local.clone();
    env.vars.push(EnvVar {
        name: "session".into(),
        value: VarValue::Secret,
        description: String::new(),
        enabled: true,
    });
    f.state.env_save(&env).unwrap();
    f.state
        .secret_set(&project.id, &env.id, "session", "s3cret-session")
        .unwrap();
    let resp = ok_response(send_and_wait(&f.state, input.clone()).await);
    assert_eq!(resp.status, 200);
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs[1].headers.get("cookie").unwrap().to_str().unwrap(),
        "SESSION=s3cret-session"
    );
    // プレビューには伏せ字で出る
    let preview = f.state.request_preview(&input).unwrap();
    let cookie = preview
        .auto_headers
        .iter()
        .find(|h| h.name == "Cookie")
        .unwrap();
    assert!(!cookie.value.contains("s3cret-session"), "{cookie:?}");

    // security: [] の API には、既定の認証としては付かない（M5 の自動化はオフにして、M3 の動きを確かめる。
    // 自動化がオンのときは、ログインの 2 段階目のために SESSION を付ける: tests/m5_cookie_csrf.rs）
    f.state
        .project_settings_set(
            &project.id,
            &nekoreq_core::ProjectSettings {
                auto_session: false,
                auto_csrf: false,
            },
        )
        .unwrap();
    let health_open = f
        .state
        .operation_open(&project.id, &OpKey::method_path("GET", "/health"))
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let input = input_for(
        &project.id,
        &health_open.op_key,
        health_open.request.clone(),
        &server.uri(),
    );
    ok_response(send_and_wait(&f.state, input).await);
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs.last().unwrap().headers.get("cookie").is_none());

    // 明示した「認証なし」は尊重する
    let mut none_req = req.clone();
    none_req.auth = Some(AuthOverride::None);
    let input = input_for(&project.id, &key("listUsers"), none_req, &server.uri());
    ok_response(send_and_wait(&f.state, input).await);
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .last()
        .unwrap()
        .headers
        .get("cookie")
        .is_none());

    // bearer の API（DELETE）は変数 token が要る。あれば Authorization が付く
    Mock::given(method("DELETE"))
        .and(path("/users/7"))
        .and(header("authorization", "Bearer tok-1"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let del_key = OpKey::method_path("DELETE", "/users/{userId}");
    let mut del = f
        .state
        .operation_open(&project.id, &del_key)
        .unwrap()
        .request;
    del.params[0].enabled = true;
    del.params[0].value = "7".into();
    let mut input = input_for(&project.id, &del_key, del, &server.uri());
    input.overrides.push(VarOverride {
        name: "token".into(),
        value: "tok-1".into(),
    });
    assert_eq!(
        ok_response(send_and_wait(&f.state, input).await).status,
        204
    );
}

#[tokio::test]
async fn selecting_operations_and_options_are_applied() {
    let f = fx();
    let (_tmp, entry) = editable_split();
    let mut req = import_req(path_input(&entry));
    req.selected = Some(vec![key("listUsers"), key("getTree"), key("notInYaml")]);
    req.options = ImportOptions {
        tag_folders: false,
        fill_from_examples: false,
        apply_cookie_auth: false,
        description_as_doc: false,
    };
    let ImportResult::Imported {
        project,
        operation_count,
        ..
    } = f.state.openapi_import(req).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(operation_count, 2);
    let list = f.state.operation_list(&project.id).unwrap();
    assert_eq!(list.len(), 2);
    assert!(
        list.iter().all(|i| i.folder.is_none()),
        "タグごとのフォルダはオフ"
    );
    // description はドキュメントにしない
    let detail = f
        .state
        .operation_get(&project.id, &key("listUsers"))
        .unwrap();
    assert!(detail.description.is_none());
    // example から埋めない: 値は空
    let open = f
        .state
        .operation_open(&project.id, &key("listUsers"))
        .unwrap();
    assert!(open.request.params.iter().all(|p| p.value.is_empty()));
    // cookieAuth を認証に反映しない
    assert!(open.default_auth.is_none());
    // 何も選ばなければ新規プロジェクトは作らない
    let mut none = import_req(path_input(&entry));
    none.selected = Some(vec![key("nope")]);
    assert!(f.state.openapi_import(none).await.is_err());
    assert_eq!(f.state.project_list().unwrap().len(), 1);
}

#[tokio::test]
async fn design_tab_has_everything_the_screen_needs() {
    let f = fx();
    let (_tmp, entry) = editable_split();
    let project = import_new(&f, path_input(&entry)).await;
    let d = f
        .state
        .operation_get(&project.id, &key("listUsers"))
        .unwrap();
    assert_eq!(d.summary.as_deref(), Some("ユーザー一覧"));
    assert_eq!(d.tags, vec!["users"]);
    assert!(d.security.required);
    assert_eq!(d.security.labels, vec!["cookieAuth（Cookie: SESSION）"]);
    assert_eq!(d.source_file.as_deref(), Some("paths/users.yaml"));
    assert!(d.source_line.is_some());
    assert!(d.imported_at.is_some());
    assert!(d
        .description
        .as_deref()
        .unwrap()
        .contains("| status | code |"));
    assert_eq!(d.parameters.len(), 2);
    assert_eq!(d.parameters[1].default.as_deref(), Some("1"));
    assert_eq!(
        d.responses
            .iter()
            .map(|r| r.status.as_str())
            .collect::<Vec<_>>(),
        vec!["200", "404"]
    );
    assert_eq!(d.error_codes[0].code, "NOT_FOUND");
    let r404 = &d.responses[1];
    assert_eq!(r404.error_codes[0].code, "NOT_FOUND");
    assert_eq!(r404.contents[0].content_type, "application/problem+json");
    assert_eq!(r404.contents[0].schema_label.as_deref(), Some("Problem"));
    // 書かれたファイルを開く場所（入口からの相対パス）
    let p = f
        .state
        .editor_target(&project.id, "paths/users.yaml")
        .unwrap();
    assert_eq!(p, entry.parent().unwrap().join("paths/users.yaml"));
    assert!(
        f.state
            .editor_target(&project.id, "../../etc/passwd")
            .is_err(),
        "取り込んだファイル以外は開けない"
    );
}

// ---- 同期 ----

#[tokio::test]
async fn sync_shows_changes_keeps_drafts_and_moves_removed_apis() {
    let f = fx();
    let (tmp, entry) = editable_split();
    let project = import_new(&f, path_input(&entry)).await;
    let pid = project.id.clone();

    // 入力値を保存しておく
    let open = f.state.operation_open(&pid, &key("listUsers")).unwrap();
    let mut req = open.request.clone();
    req.params[0].enabled = true;
    req.params[0].value = "手で入れた値".into();
    f.state
        .draft_save(&RequestDraft {
            id: String::new(),
            project_id: pid.clone(),
            op_key: key("listUsers"),
            params: req.params.clone(),
            body: Body::None,
            auth: None,
            updated_at: String::new(),
        })
        .unwrap();
    f.state
        .draft_save(&RequestDraft {
            id: String::new(),
            project_id: pid.clone(),
            op_key: key("getTree"),
            params: vec![],
            body: Body::None,
            auth: None,
            updated_at: String::new(),
        })
        .unwrap();

    // 変更なし
    let SyncCheckResult::Ready {
        diff,
        changed_files,
    } = f.state.openapi_sync_check(&pid, None, None).await.unwrap()
    else {
        panic!()
    };
    assert!(diff.is_empty(), "{diff:?}");
    assert!(changed_files.is_empty());
    assert!(!f.state.openapi_source_changed(&pid).unwrap().changed);

    // yaml の description を書き換える / page パラメータを消す / getTree を消す
    let users = tmp.path().join("paths/users.yaml");
    std::thread::sleep(Duration::from_millis(20));
    let text = std::fs::read_to_string(&users).unwrap();
    let text = text
        .replace("ユーザーを返す。", "ユーザーの一覧を返す。")
        .replace("        - $ref: \"#/components/parameters/Page\"\n", "");
    std::fs::write(&users, text).unwrap();
    std::fs::write(tmp.path().join("paths/tree.yaml"), "paths: {}\n").unwrap();

    // 起動時・ウィンドウに戻ったときの確認（読み直さずに更新日時だけ）
    let changed = f.state.openapi_source_changed(&pid).unwrap();
    assert!(changed.changed);
    let mut files = changed.changed_files.clone();
    files.sort();
    assert_eq!(files, vec!["paths/tree.yaml", "paths/users.yaml"]);

    // 差分を確認（何も反映しない）
    let SyncCheckResult::Ready {
        diff,
        changed_files,
    } = f.state.openapi_sync_check(&pid, None, None).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(changed_files.len(), 2);
    assert_eq!(diff.changed.len(), 1);
    let c = &diff.changed[0];
    assert_eq!(c.entry.op_key, key("listUsers"));
    let fields: Vec<_> = c.changes.iter().map(|ch| ch.field).collect();
    assert_eq!(
        fields,
        vec![ChangedField::Description, ChangedField::Parameters]
    );
    assert_eq!(c.removed_params, vec!["page"]);
    assert!(c.entry.has_draft);
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.removed[0].op_key, key("getTree"));
    assert!(diff.removed[0].has_draft);
    assert!(diff.added.is_empty());
    // まだ反映していない
    let before = f.state.operation_get(&pid, &key("listUsers")).unwrap();
    assert!(before
        .description
        .as_deref()
        .unwrap()
        .contains("ユーザーを返す。"));
    assert!(f.state.openapi_source_changed(&pid).unwrap().changed);

    // 反映
    let SyncApplyResult::Applied {
        diff: applied,
        report,
        project,
    } = f.state.openapi_sync_apply(&pid, None, None).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(applied, diff);
    assert_eq!(
        (report.changed, report.removed, report.kept_as_deleted),
        (1, 1, 1)
    );
    assert!(project.source.as_ref().unwrap().synced_at.is_some());
    assert!(
        !f.state.openapi_source_changed(&pid).unwrap().changed,
        "反映すると印は消える"
    );
    // 設計タブに反映される
    let after = f.state.operation_get(&pid, &key("listUsers")).unwrap();
    assert!(after
        .description
        .as_deref()
        .unwrap()
        .contains("ユーザーの一覧を返す。"));
    assert_eq!(after.parameters.len(), 1);
    // 入力値は残り、消えたパラメータは「yaml にない」として無効のまま残る
    let open = f.state.operation_open(&pid, &key("listUsers")).unwrap();
    assert!(open.has_draft);
    assert_eq!(open.request.params[0].value, "手で入れた値");
    assert!(open.request.params[0].enabled);
    let page = open
        .request
        .params
        .iter()
        .find(|p| p.name == "page")
        .unwrap();
    assert!(!page.enabled, "送信から外す");
    assert_eq!(page.location, ParamIn::Query);
    assert_eq!(open.missing_in_yaml.len(), 1);
    assert_eq!(open.missing_in_yaml[0].name, "page");
    // 消えた API は「削除された API」として残る
    let list = f.state.operation_list(&pid).unwrap();
    assert_eq!(list.len(), 6);
    let tree = list.iter().find(|i| i.op_key == key("getTree")).unwrap();
    assert!(tree.deleted && tree.has_draft);
    assert!(
        f.state
            .operation_open(&pid, &key("getTree"))
            .unwrap()
            .deleted
    );
    // 自分で消すまで残る。消すと入力値も消える
    f.state.operation_forget(&pid, &key("getTree")).unwrap();
    assert_eq!(f.state.operation_list(&pid).unwrap().len(), 5);
    assert!(f.state.draft_get(&pid, &key("getTree")).unwrap().is_none());
    // もう一度確認すると変更なし
    let SyncCheckResult::Ready { diff, .. } =
        f.state.openapi_sync_check(&pid, None, None).await.unwrap()
    else {
        panic!()
    };
    assert!(diff.is_empty());
}

#[tokio::test]
async fn broken_yaml_aborts_the_sync_and_keeps_the_previous_operations() {
    let f = fx();
    let (tmp, entry) = editable_split();
    let project = import_new(&f, path_input(&entry)).await;
    let pid = project.id.clone();
    let before = f.state.operation_list(&pid).unwrap();

    std::fs::write(
        tmp.path().join("paths/tree.yaml"),
        "paths:\n  /tree:\n    get: [unclosed\n",
    )
    .unwrap();
    let SyncCheckResult::Blocked { problems } =
        f.state.openapi_sync_check(&pid, None, None).await.unwrap()
    else {
        panic!("止まるはず")
    };
    let LoadProblem::YamlError {
        file, line, reason, ..
    } = &problems[0]
    else {
        panic!("{problems:?}")
    };
    assert_eq!(file, "paths/tree.yaml");
    assert!(line.is_some() && !reason.is_empty());
    let SyncApplyResult::Blocked { .. } =
        f.state.openapi_sync_apply(&pid, None, None).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(
        f.state.operation_list(&pid).unwrap(),
        before,
        "前回の Operation を使い続ける"
    );
    // 足りないファイルも同じ（足りないファイルと参照元の一覧）
    std::fs::remove_file(tmp.path().join("common/responses.yaml")).unwrap();
    std::fs::write(tmp.path().join("paths/tree.yaml"), "paths: {}\n").unwrap();
    let SyncCheckResult::Blocked { problems } =
        f.state.openapi_sync_check(&pid, None, None).await.unwrap()
    else {
        panic!()
    };
    let LoadProblem::MissingFile {
        file,
        referenced_from,
        ..
    } = &problems[0]
    else {
        panic!("{problems:?}")
    };
    assert_eq!(file, "common/responses.yaml");
    assert_eq!(referenced_from[0].file, "paths/users.yaml");
    assert!(referenced_from[0].line.is_some());
}

#[tokio::test]
async fn integrating_into_an_existing_project_shows_a_diff_and_adds_unselected_later() {
    let f = fx();
    let (tmp, entry) = editable_split();
    let mut req = import_req(path_input(&entry));
    req.selected = Some(vec![key("listUsers")]);
    let ImportResult::Imported { project, .. } = f.state.openapi_import(req).await.unwrap() else {
        panic!()
    };
    assert_eq!(f.state.operation_list(&project.id).unwrap().len(), 1);
    // 選ばなかった API は同期で「追加」と出ない
    let SyncCheckResult::Ready { diff, .. } = f
        .state
        .openapi_sync_check(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(diff.is_empty(), "{diff:?}");
    // 既存プロジェクトへの統合の確認（差分つき）。追加で getTree を選んで取り込む
    let PreviewResult::Ready { preview } = f
        .state
        .openapi_preview(path_input(&entry), Some(project.id.clone()))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(preview.diff.is_some());
    let mut req = import_req(path_input(&entry));
    req.project_id = Some(project.id.clone());
    req.selected = Some(vec![key("getTree")]);
    let ImportResult::Imported {
        operation_count,
        created_environments,
        ..
    } = f.state.openapi_import(req).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(operation_count, 2);
    assert!(created_environments.is_empty(), "同名の環境は作らない");
    drop(tmp);
}

// ---- アップロード（コピー方式） ----

#[tokio::test]
async fn upload_copies_the_folder_and_sync_needs_a_new_upload() {
    let f = fx();
    let src = tempfile::tempdir().unwrap();
    copy_dir(&openapi_fixture("split"), src.path());
    let project = import_new(
        &f,
        SourceInput::Upload {
            paths: vec![src.path().display().to_string()],
            entry: None,
            allowed_hosts: vec![],
        },
    )
    .await;
    let source = project.source.as_ref().unwrap();
    assert_eq!(source.kind, SourceKind::Copy);
    assert_eq!(source.entry, "openapi.yaml");
    // 元の構成のまま sources/<プロジェクト ID>/ に置かれる
    let dest = f.data.join("sources").join(&project.id);
    assert!(dest.join("openapi.yaml").exists());
    assert!(dest.join("paths/users.yaml").exists());
    assert!(dest.join("common/schemas.yaml").exists());
    assert!(
        !f.data.join("tmp").exists()
            || std::fs::read_dir(f.data.join("tmp"))
                .unwrap()
                .next()
                .is_none(),
        "一時フォルダは残らない"
    );
    assert_eq!(f.state.operation_list(&project.id).unwrap().len(), 6);
    // エディタで開くのは、置いたコピー
    assert_eq!(
        f.state
            .editor_target(&project.id, "paths/users.yaml")
            .unwrap(),
        dest.join("paths/users.yaml")
    );
    // 元のフォルダを変えても、コピーは変わらない。更新日時の確認は常に「変わっていない」
    std::fs::write(src.path().join("paths/tree.yaml"), "paths: {}\n").unwrap();
    assert!(!f.state.openapi_source_changed(&project.id).unwrap().changed);

    // 同期はもう一度アップロード
    let SyncCheckResult::NeedsUpload { entry } = f
        .state
        .openapi_sync_check(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(entry, "openapi.yaml");
    let up = UploadInput {
        paths: vec![src.path().display().to_string()],
        entry: None,
    };
    let SyncCheckResult::Ready { diff, .. } = f
        .state
        .openapi_sync_check(&project.id, Some(up.clone()), None)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(diff.removed.len(), 1);
    let SyncApplyResult::Applied { .. } = f
        .state
        .openapi_sync_apply(&project.id, Some(up), None)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(f.state.operation_list(&project.id).unwrap().len(), 5);
    assert_eq!(
        std::fs::read_to_string(dest.join("paths/tree.yaml")).unwrap(),
        "paths: {}\n",
        "コピーも置き直す"
    );

    // プロジェクトを消すとコピーも消える
    f.state.project_delete(&project.id).unwrap();
    assert!(!dest.exists());
}

#[tokio::test]
async fn upload_with_several_entry_candidates_asks_and_reads_zip() {
    let f = fx();
    let multi = openapi_fixture("multi_entry");
    let PreviewResult::NeedsEntry { candidates } = f
        .state
        .openapi_preview(
            SourceInput::Upload {
                paths: vec![multi.display().to_string()],
                entry: None,
                allowed_hosts: vec![],
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!("候補を返すはず")
    };
    assert_eq!(candidates, vec!["openapi.bundled.yaml", "openapi.yaml"]);
    let PreviewResult::Ready { preview } = f
        .state
        .openapi_preview(
            SourceInput::Upload {
                paths: vec![multi.display().to_string()],
                entry: Some("openapi.bundled.yaml".into()),
                allowed_hosts: vec![],
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(preview.entry, "openapi.bundled.yaml");
    assert_eq!(preview.file_count, 1);
    // 存在しない入口はエラー
    assert!(f
        .state
        .openapi_preview(
            SourceInput::Upload {
                paths: vec![multi.display().to_string()],
                entry: Some("nope.yaml".into()),
                allowed_hosts: vec![]
            },
            None
        )
        .await
        .is_err());

    // zip
    let zip_dir = tempfile::tempdir().unwrap();
    let zip_path = zip_dir.path().join("api.zip");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, src) in [
            ("api/openapi.yaml", openapi_fixture("split/openapi.yaml")),
            (
                "api/paths/users.yaml",
                openapi_fixture("split/paths/users.yaml"),
            ),
            (
                "api/paths/tree.yaml",
                openapi_fixture("split/paths/tree.yaml"),
            ),
            (
                "api/common/schemas.yaml",
                openapi_fixture("split/common/schemas.yaml"),
            ),
            (
                "api/common/responses.yaml",
                openapi_fixture("split/common/responses.yaml"),
            ),
        ] {
            z.start_file(name, opts).unwrap();
            z.write_all(&std::fs::read(src).unwrap()).unwrap();
        }
        z.finish().unwrap();
    }
    let project = import_new(
        &f,
        SourceInput::Upload {
            paths: vec![zip_path.display().to_string()],
            entry: None,
            allowed_hosts: vec![],
        },
    )
    .await;
    assert_eq!(project.source.as_ref().unwrap().entry, "api/openapi.yaml");
    assert_eq!(f.state.operation_list(&project.id).unwrap().len(), 6);
    assert!(f
        .data
        .join("sources")
        .join(&project.id)
        .join("api/paths/users.yaml")
        .exists());
}

// ---- URL ----

#[tokio::test]
async fn url_source_stores_the_token_in_the_keychain_and_uses_it_for_sync() {
    let f = fx();
    let server = MockServer::start().await;
    for (p, file) in [
        ("/api/openapi.yaml", "split/openapi.yaml"),
        ("/api/paths/users.yaml", "split/paths/users.yaml"),
        ("/api/paths/tree.yaml", "split/paths/tree.yaml"),
        ("/api/common/schemas.yaml", "split/common/schemas.yaml"),
        ("/api/common/responses.yaml", "split/common/responses.yaml"),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .and(header("authorization", "Bearer tok-url"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(std::fs::read_to_string(openapi_fixture(file)).unwrap()),
            )
            .mount(&server)
            .await;
    }
    let url = format!("{}/api/openapi.yaml", server.uri());
    // トークンなしだと読めない（非公開の想定）
    let PreviewResult::Blocked { problems } = f
        .state
        .openapi_preview(
            SourceInput::Url {
                url: url.clone(),
                token: None,
                allowed_hosts: vec![],
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(problems[0].describe().contains("アクセストークン"));
    // トークンつきで取り込む
    let project = import_new(
        &f,
        SourceInput::Url {
            url: url.clone(),
            token: Some("tok-url".into()),
            allowed_hosts: vec![],
        },
    )
    .await;
    let source = project.source.as_ref().unwrap();
    assert_eq!(source.kind, SourceKind::Url);
    assert_eq!(source.entry, url);
    assert!(source.has_token);
    assert_eq!(
        f.secrets
            .get(&SecretKey::new(&project.id, "source", "token"))
            .unwrap()
            .as_deref(),
        Some("tok-url")
    );
    // SQLite のファイルのどこにもトークンは平文で残らない
    for (name, bytes) in all_files(&f.data) {
        assert!(
            !String::from_utf8_lossy(&bytes).contains("tok-url"),
            "{name} にトークンがある"
        );
    }
    // 同期はキーチェーンのトークンで読める
    let SyncCheckResult::Ready { diff, .. } = f
        .state
        .openapi_sync_check(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(diff.is_empty());
    // トークンの更新・削除
    let p = f.state.source_token_set(&project.id, "tok-url").unwrap();
    assert!(p.source.unwrap().has_token);
    let p = f.state.source_token_set(&project.id, "").unwrap();
    assert!(!p.source.unwrap().has_token);
    assert!(f
        .secrets
        .get(&SecretKey::new(&project.id, "source", "token"))
        .unwrap()
        .is_none());
    let SyncCheckResult::Blocked { .. } = f
        .state
        .openapi_sync_check(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!("トークンを消したので読めない")
    };
    // URL 方式以外にはトークンを置けない
    let (_tmp, entry) = editable_split();
    let local = import_new(&f, path_input(&entry)).await;
    assert!(f.state.source_token_set(&local.id, "x").is_err());
    // エディタでは開けない
    assert!(f
        .state
        .editor_target(&project.id, "paths/users.yaml")
        .is_err());
    f.state.project_delete(&project.id).unwrap();
}

fn all_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(all_files(&p));
        } else {
            out.push((p.display().to_string(), std::fs::read(&p).unwrap()));
        }
    }
    out
}

#[tokio::test]
async fn foreign_host_reference_waits_for_confirmation_in_the_app_and_is_remembered() {
    let f = fx();
    let main = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("GET")).and(path("/openapi.yaml")).respond_with(ResponseTemplate::new(200).set_body_string(format!(
        "openapi: 3.0.3\ninfo: {{title: F, version: '1'}}\npaths:\n  /a:\n    $ref: '{}/s.yaml#/paths/~1a'\n", other.uri()
    ))).mount(&main).await;
    Mock::given(method("GET")).and(path("/s.yaml")).respond_with(ResponseTemplate::new(200).set_body_string(
        "paths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200':\n          description: ok\n"
    )).mount(&other).await;
    let url = format!("{}/openapi.yaml", main.uri());
    let host = other.uri();
    let PreviewResult::NeedsConfirmation { foreign } = f
        .state
        .openapi_preview(
            SourceInput::Url {
                url: url.clone(),
                token: None,
                allowed_hosts: vec![],
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(foreign[0].host, host);
    assert!(other.received_requests().await.unwrap().is_empty());
    let project = import_new(
        &f,
        SourceInput::Url {
            url,
            token: None,
            allowed_hosts: vec![host.clone()],
        },
    )
    .await;
    assert_eq!(project.source.as_ref().unwrap().allowed_hosts, vec![host]);
    // 同期では確認済みのホストをそのまま読む
    let SyncCheckResult::Ready { diff, .. } = f
        .state
        .openapi_sync_check(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!("確認済みなので聞き直さない")
    };
    assert!(diff.is_empty());
}

// ---- 実際の API 定義（あれば） ----

#[tokio::test]
async fn real_fixture_import_get_client_list_and_send() {
    let (Some(dir), Some(bundled)) = (
        std::env::var_os("NEKOREQ_FIXTURE_DIR"),
        std::env::var_os("NEKOREQ_FIXTURE_BUNDLED"),
    ) else {
        eprintln!("NEKOREQ_FIXTURE_DIR / NEKOREQ_FIXTURE_BUNDLED が無いのでスキップ");
        return;
    };
    let entry = PathBuf::from(dir).join("openapi.yaml");
    if !entry.exists() || !Path::new(&bundled).exists() {
        eprintln!("fixture が無いのでスキップ");
        return;
    }
    let f = fx();
    let t = Instant::now();
    let PreviewResult::Ready { preview } = f
        .state
        .openapi_preview(path_input(&entry), None)
        .await
        .unwrap()
    else {
        panic!()
    };
    let preview_time = t.elapsed();
    let t = Instant::now();
    let ImportResult::Imported {
        project,
        operation_count,
        created_environments,
        ..
    } = f
        .state
        .openapi_import(import_req(path_input(&entry)))
        .await
        .unwrap()
    else {
        panic!()
    };
    let import_time = t.elapsed();
    eprintln!(
        "確認 {preview_time:?} / 取り込み（保存まで）{import_time:?} / {} 件・{} ファイル",
        operation_count, preview.file_count
    );
    assert!(import_time < Duration::from_secs(2), "{import_time:?}");
    let split_count = operation_count;
    assert!(split_count > 0);
    assert_eq!(created_environments, vec!["ローカル (Dev Container)"]);
    let envs = f.state.env_list(&project.id).unwrap();
    let local = envs
        .iter()
        .find(|e| e.name == "ローカル (Dev Container)")
        .unwrap();
    assert_eq!(
        local.vars[0].value,
        VarValue::Plain("http://localhost:8080".into())
    );
    assert_eq!(
        f.state.operation_list(&project.id).unwrap().len(),
        split_count as usize
    );

    // 「取引先一覧取得」がそのまま送れる
    let get = key("getClientList");
    let open = f.state.operation_open(&project.id, &get).unwrap();
    assert_eq!(open.method, "GET");
    assert_eq!(open.request.url, "{{baseUrl}}/api/v1/video/clients");
    let detail = f.state.operation_get(&project.id, &get).unwrap();
    assert_eq!(detail.summary.as_deref(), Some("取引先一覧取得"));
    assert_eq!(detail.source_file.as_deref(), Some("video/client.yaml"));
    assert_eq!(detail.source_line, Some(5));
    let ok = detail.responses.iter().find(|r| r.status == "200").unwrap();
    assert_eq!(
        ok.contents[0].schema_label.as_deref(),
        Some("ClientListApiResponse")
    );
    assert!(!ok.contents[0].rows.is_empty());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/video/clients"))
        .and(query_param("perPage", "20"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("{\"data\":{\"items\":[],\"total\":0}}"),
        )
        .mount(&server)
        .await;
    let mut req = open.request.clone();
    let per_page = req.params.iter_mut().find(|p| p.name == "perPage").unwrap();
    per_page.value = "20".into();
    per_page.enabled = true;
    let resp = ok_response(
        send_and_wait(&f.state, input_for(&project.id, &get, req, &server.uri())).await,
    );
    assert_eq!(resp.status, 200);

    // security: [] の 6 件は認証なし
    let list = f.state.operation_list(&project.id).unwrap();
    assert_eq!(list.iter().filter(|i| !i.requires_auth).count(), 6);

    // bundle 版から取り込んでも同じ件数になる
    let ImportResult::Imported {
        operation_count, ..
    } = f
        .state
        .openapi_import(import_req(path_input(Path::new(&bundled))))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(operation_count, split_count);
}

// 実際のファイルの 1 つを書き換えた一時コピーで、description の変更が「変更」になる
#[tokio::test]
async fn real_fixture_description_change_is_a_diff_and_drafts_survive() {
    let Some(dir) = std::env::var_os("NEKOREQ_FIXTURE_DIR") else {
        eprintln!("NEKOREQ_FIXTURE_DIR が無いのでスキップ");
        return;
    };
    let src = PathBuf::from(dir);
    if !src.join("openapi.yaml").exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&src, tmp.path());
    let entry = tmp.path().join("openapi.yaml");
    let f = fx();
    let project = import_new(&f, path_input(&entry)).await;
    let get = key("getClientList");
    let mut req = f.state.operation_open(&project.id, &get).unwrap().request;
    req.params[0].value = "Netflix".into();
    req.params[0].enabled = true;
    f.state
        .draft_save(&RequestDraft {
            id: String::new(),
            project_id: project.id.clone(),
            op_key: get.clone(),
            params: req.params.clone(),
            body: Body::None,
            auth: None,
            updated_at: String::new(),
        })
        .unwrap();

    let client = tmp.path().join("video/client.yaml");
    std::thread::sleep(Duration::from_millis(20));
    let text = std::fs::read_to_string(&client).unwrap();
    std::fs::write(
        &client,
        text.replacen("取引先 (親) ごとに", "【書き換え】取引先 (親) ごとに", 1),
    )
    .unwrap();
    assert!(f.state.openapi_source_changed(&project.id).unwrap().changed);
    let t = Instant::now();
    let SyncCheckResult::Ready { diff, .. } = f
        .state
        .openapi_sync_check(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!()
    };
    let check_time = t.elapsed();
    eprintln!("同期の確認（差分が出るまで）: {check_time:?}");
    assert!(check_time < Duration::from_secs(1), "{check_time:?}");
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(diff.changed[0].entry.op_key, get);
    assert_eq!(diff.changed[0].changes[0].field, ChangedField::Description);
    f.state
        .openapi_sync_apply(&project.id, None, None)
        .await
        .unwrap();
    let detail = f.state.operation_get(&project.id, &get).unwrap();
    assert!(detail.description.unwrap().starts_with("【書き換え】"));
    let open = f.state.operation_open(&project.id, &get).unwrap();
    assert!(open.has_draft);
    assert_eq!(open.request.params[0].value, "Netflix");
    assert!(open.request.params[0].enabled);
    let _ = KeyValue {
        location: ParamIn::Query,
        name: String::new(),
        value: String::new(),
        enabled: false,
    };
}
