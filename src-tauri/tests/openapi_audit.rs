//! OpenAPI の取り込み・同期（M3・M4）を AppState 経由で確かめる、テスト担当が独立に書いた観点。
//! 既存の openapi_flow.rs と重ならないもの（秘密の扱い・コピー方式の安全性・同期の端のケース・性能）。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use nekoreq_core::{
    Body, ImportOptions, KeyValue, OpKey, ParamIn, Project, RequestDraft, SourceKind,
};
use nekoreq_lib::openapi::{
    ImportRequest, ImportResult, PreviewResult, SourceInput, SyncApplyResult, SyncCheckResult,
    UploadInput,
};
use nekoreq_lib::state::AppState;
use nekoreq_openapi::{LoadProblem, SyncDiff};
use nekoreq_secrets::{MemorySecretStore, SecretKey, SecretStore};
use wiremock::matchers::{header, method, path};
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

const HEAD: &str = "openapi: 3.0.3\ninfo: {title: Audit, version: '1'}\n";

const ENTRY: &str = "servers:\n  - {url: 'http://localhost:8080', description: ローカル}\ntags: [{name: t}]\npaths:\n  /a: {$ref: './a.yaml#/p'}\n  /b: {$ref: './b.yaml'}\n  /c:\n    get:\n      operationId: getC\n      tags: [t]\n      summary: C\n      parameters:\n        - {name: q, in: query, schema: {type: string}}\n        - {name: limit, in: query, schema: {type: integer}}\n      responses:\n        '200': {description: ok}\n";
const A_YAML: &str = "p:\n  get:\n    operationId: getA\n    tags: [t]\n    summary: A\n    responses:\n      '200': {description: ok}\n";
const B_YAML: &str = "post:\n  operationId: postB\n  tags: [t]\n  summary: B\n  responses:\n    '200': {description: ok}\n";

fn spec_tree() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("openapi.yaml"), format!("{HEAD}{ENTRY}")).unwrap();
    fs::write(d.path().join("a.yaml"), A_YAML).unwrap();
    fs::write(d.path().join("b.yaml"), B_YAML).unwrap();
    let e = d.path().join("openapi.yaml");
    (d, e)
}

fn path_input(p: &Path) -> SourceInput {
    SourceInput::Path {
        path: p.display().to_string(),
        allowed_hosts: vec![],
    }
}

fn req(source: SourceInput) -> ImportRequest {
    ImportRequest {
        source,
        project_id: None,
        project_name: None,
        selected: None,
        options: ImportOptions::default(),
    }
}

async fn import_new(f: &Fx, source: SourceInput) -> Project {
    match f.state.openapi_import(req(source)).await.unwrap() {
        ImportResult::Imported { project, .. } => *project,
        other => panic!("取り込めるはず: {other:?}"),
    }
}

fn key(id: &str) -> OpKey {
    OpKey::Id(id.into())
}

fn kv(name: &str, value: &str) -> KeyValue {
    KeyValue {
        location: ParamIn::Query,
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}

fn save_draft(f: &Fx, project: &str, k: OpKey, params: Vec<KeyValue>) {
    f.state
        .draft_save(&RequestDraft {
            id: String::new(),
            project_id: project.into(),
            op_key: k,
            params,
            body: Body::None,
            auth: None,
            updated_at: String::new(),
        })
        .unwrap();
}

fn all_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(all_files(&p));
            } else if let Ok(b) = fs::read(&p) {
                out.push((p.display().to_string(), b));
            }
        }
    }
    out
}

fn touch(p: &Path, secs_from_now: i64) {
    let t = if secs_from_now >= 0 {
        SystemTime::now() + Duration::from_secs(secs_from_now as u64)
    } else {
        SystemTime::now() - Duration::from_secs((-secs_from_now) as u64)
    };
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

async fn sync_check(f: &Fx, id: &str) -> SyncCheckResult {
    f.state.openapi_sync_check(id, None).await.unwrap()
}

async fn sync_diff(f: &Fx, id: &str) -> SyncDiff {
    match sync_check(f, id).await {
        SyncCheckResult::Ready { diff, .. } => diff,
        other => panic!("読めるはず: {other:?}"),
    }
}

async fn sync_apply(f: &Fx, id: &str) -> (SyncDiff, nekoreq_lib::openapi::ImportReport) {
    match f.state.openapi_sync_apply(id, None).await.unwrap() {
        SyncApplyResult::Applied { diff, report, .. } => (diff, report),
        other => panic!("反映できるはず: {other:?}"),
    }
}

// ================= 秘密の扱い =================

#[test]
fn debug_output_of_inputs_that_carry_a_token_does_not_show_it() {
    let input = SourceInput::Url {
        url: "https://h.example/x.yaml".into(),
        token: Some("TOKEN-VALUE-XYZ".into()),
        allowed_hosts: vec![],
    };
    assert!(
        !format!("{input:?}").contains("TOKEN-VALUE-XYZ"),
        "SourceInput"
    );
    assert!(
        !format!("{:?}", req(input)).contains("TOKEN-VALUE-XYZ"),
        "ImportRequest"
    );
    let p = nekoreq_openapi::LoadParams {
        entry: nekoreq_openapi::LoadEntry::Url("https://h.example/x.yaml".into()),
        token: Some("TOKEN-VALUE-XYZ".into()),
        allowed_hosts: vec![],
        root: None,
    };
    assert!(!format!("{p:?}").contains("TOKEN-VALUE-XYZ"), "LoadParams");
}

const MIN: &str = "openapi: 3.0.3\ninfo: {title: U, version: '1'}\npaths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200': {description: ok}\n";

#[tokio::test]
async fn userinfo_in_the_entry_url_is_refused_and_never_stored() {
    // 決定: `user:pass@` を含む URL はエラー（トークンはトークン欄で）。クエリはそのまま使い・保存する
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MIN))
        .mount(&server)
        .await;
    let host = server.uri().trim_start_matches("http://").to_string();
    for url in [
        format!("http://me:USERPASS@{host}/x.yaml"),
        format!("http://USERPASS@{host}/x.yaml?access_token=QUERYSECRET"),
    ] {
        let r = f
            .state
            .openapi_import(req(SourceInput::Url {
                url: url.clone(),
                token: None,
                allowed_hosts: vec![],
            }))
            .await;
        let Err(e) = r else {
            panic!("ユーザー情報つきの URL は取り込めないはず")
        };
        let msg = e.to_string();
        assert!(msg.contains("トークン"), "{msg}");
        assert!(!msg.contains("USERPASS"), "{msg}");
        let pv = f
            .state
            .openapi_preview(
                SourceInput::Url {
                    url,
                    token: None,
                    allowed_hosts: vec![],
                },
                None,
            )
            .await;
        assert!(pv.is_err());
    }
    assert!(f.state.project_list().unwrap().is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
    for (name, bytes) in all_files(&f.data) {
        assert!(
            !String::from_utf8_lossy(&bytes).contains("USERPASS"),
            "{name} に USERPASS"
        );
    }
    // クエリだけなら、そのまま取り込める（保存もそのまま）
    let url = format!("http://{host}/x.yaml?access_token=Q1");
    let project = import_new(
        &f,
        SourceInput::Url {
            url: url.clone(),
            token: None,
            allowed_hosts: vec![],
        },
    )
    .await;
    assert_eq!(project.source.as_ref().unwrap().entry, url);
}

#[tokio::test]
async fn a_token_pasted_with_a_trailing_newline_still_works() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x.yaml"))
        .and(header("authorization", "Bearer tok-1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MIN))
        .mount(&server)
        .await;
    let r = f
        .state
        .openapi_import(req(SourceInput::Url {
            url: format!("{}/x.yaml", server.uri()),
            token: Some("tok-1\n".into()),
            allowed_hosts: vec![],
        }))
        .await
        .unwrap();
    assert!(matches!(r, ImportResult::Imported { .. }), "{r:?}");
}

#[tokio::test]
async fn token_and_urls_in_problem_texts_and_previews_are_clean() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let r = f
        .state
        .openapi_preview(
            SourceInput::Url {
                url: format!("{}/x.yaml?sig=QS", server.uri()),
                token: Some("TOKEN-IN-ERROR".into()),
                allowed_hosts: vec![],
            },
            None,
        )
        .await
        .unwrap();
    let PreviewResult::Blocked { problems } = r else {
        panic!()
    };
    let text = format!("{problems:?}");
    assert!(
        !text.contains("TOKEN-IN-ERROR") && !text.contains("QS"),
        "{text}"
    );
    assert!(
        f.state.project_list().unwrap().is_empty(),
        "確認では何も保存しない"
    );
    assert!(
        !f.data.join("sources").exists()
            || fs::read_dir(f.data.join("sources"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[tokio::test]
async fn deleting_a_project_removes_its_token_and_copied_yaml_but_not_other_projects() {
    let f = fx();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(MIN))
        .mount(&server)
        .await;
    let url_project = import_new(
        &f,
        SourceInput::Url {
            url: format!("{}/x.yaml", server.uri()),
            token: Some("tok-del".into()),
            allowed_hosts: vec![],
        },
    )
    .await;
    let (_d, e) = spec_tree();
    let up = |d: &Path| SourceInput::Upload {
        paths: vec![d.display().to_string()],
        entry: None,
        allowed_hosts: vec![],
    };
    let copy_a = import_new(&f, up(e.parent().unwrap())).await;
    let copy_b = import_new(&f, up(e.parent().unwrap())).await;
    let dir_a = f.data.join("sources").join(&copy_a.id);
    let dir_b = f.data.join("sources").join(&copy_b.id);
    assert!(dir_a.join("openapi.yaml").exists() && dir_b.join("openapi.yaml").exists());
    let tok = SecretKey::new(&url_project.id, "source", "token");
    assert!(f.secrets.get(&tok).unwrap().is_some());

    f.state.project_delete(&url_project.id).unwrap();
    assert!(
        f.secrets.get(&tok).unwrap().is_none(),
        "トークンがキーチェーンに残っている"
    );
    f.state.project_delete(&copy_a.id).unwrap();
    assert!(!dir_a.exists(), "sources/<ID> が残っている");
    assert!(
        dir_b.join("openapi.yaml").exists(),
        "ほかのプロジェクトのコピーが消えた"
    );
    assert!(f.state.operation_list(&copy_b.id).unwrap().len() == 3);
    f.state.project_delete(&copy_b.id).unwrap();
    assert!(fs::read_dir(f.data.join("sources"))
        .map(|mut r| r.next().is_none())
        .unwrap_or(true));
    // 一時フォルダも残らない
    assert!(
        !f.data.join("tmp").exists() || fs::read_dir(f.data.join("tmp")).unwrap().next().is_none()
    );
    // 存在しないプロジェクトを消しても、データフォルダは無傷
    assert!(f.state.project_delete("..").is_err());
    assert!(f.data.exists());
}

// ================= コピー方式の安全性 =================

#[tokio::test]
async fn upload_cannot_read_host_files_through_refs_end_to_end() {
    let f = fx();
    let host = tempfile::tempdir().unwrap();
    fs::write(
        host.path().join("private.yaml"),
        "get:\n  operationId: leaked\n  summary: HOST-PRIVATE-SUMMARY\n  responses:\n    '200': {description: ok}\n",
    )
    .unwrap();
    let up = tempfile::tempdir().unwrap();
    fs::write(
        up.path().join("openapi.yaml"),
        format!(
            "{HEAD}paths:\n  /leak: {{$ref: '{}'}}\n",
            host.path().join("private.yaml").display()
        ),
    )
    .unwrap();
    let r = f
        .state
        .openapi_import(req(SourceInput::Upload {
            paths: vec![up.path().display().to_string()],
            entry: None,
            allowed_hosts: vec![],
        }))
        .await
        .unwrap();
    match r {
        ImportResult::Blocked { .. } => {}
        ImportResult::Imported { project, .. } => {
            let d = f.state.operation_get(&project.id, &key("leaked"));
            panic!("ホストのファイルを取り込んだ: {:?}", d.map(|d| d.summary));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn editor_target_only_opens_files_that_were_imported() {
    let f = fx();
    let (_d, e) = spec_tree();
    let p = import_new(&f, path_input(&e)).await;
    let ok = f.state.editor_target(&p.id, "a.yaml").unwrap();
    assert_eq!(ok, e.parent().unwrap().join("a.yaml"));
    for bad in [
        "../../../../etc/passwd",
        "/etc/passwd",
        "a.yaml/../../x",
        "./a.yaml",
        "A.YAML",
        "",
        "..",
        "sub/../a.yaml",
    ] {
        assert!(
            f.state.editor_target(&p.id, bad).is_err(),
            "{bad:?} が通った"
        );
    }
    // コピー方式でも同じ
    let c = import_new(
        &f,
        SourceInput::Upload {
            paths: vec![e.parent().unwrap().display().to_string()],
            entry: None,
            allowed_hosts: vec![],
        },
    )
    .await;
    assert!(f.state.editor_target(&c.id, "../x").is_err());
    assert!(f
        .state
        .editor_target(&c.id, "b.yaml")
        .unwrap()
        .starts_with(f.data.join("sources")));
}

#[tokio::test]
async fn reuploading_replaces_the_copy_and_does_not_keep_removed_files() {
    let f = fx();
    let (d, _e) = spec_tree();
    let p = import_new(
        &f,
        SourceInput::Upload {
            paths: vec![d.path().display().to_string()],
            entry: None,
            allowed_hosts: vec![],
        },
    )
    .await;
    let dest = f.data.join("sources").join(&p.id);
    assert!(dest.join("b.yaml").exists());
    // b.yaml への参照をやめ、b.yaml を消して再アップロード
    fs::write(
        d.path().join("openapi.yaml"),
        format!("{HEAD}{}", ENTRY.replace("  /b: {$ref: './b.yaml'}\n", "")),
    )
    .unwrap();
    fs::remove_file(d.path().join("b.yaml")).unwrap();
    let up = UploadInput {
        paths: vec![d.path().display().to_string()],
        entry: None,
    };
    let r = f.state.openapi_sync_apply(&p.id, Some(up)).await.unwrap();
    let SyncApplyResult::Applied { diff, .. } = r else {
        panic!("{r:?}")
    };
    assert_eq!(diff.removed.len(), 1);
    assert!(!dest.join("b.yaml").exists(), "古いファイルが残っている");
    assert!(dest.join("a.yaml").exists());
    assert!(
        !f.data.join("tmp").exists() || fs::read_dir(f.data.join("tmp")).unwrap().next().is_none()
    );
    // 壊れた再アップロード（取り込めない）は、前回のコピーと Operation を残す
    fs::write(
        d.path().join("openapi.yaml"),
        "openapi: 3.0.3\ninfo: [oops\n",
    )
    .unwrap();
    let up = UploadInput {
        paths: vec![d.path().display().to_string()],
        entry: None,
    };
    let r = f.state.openapi_sync_apply(&p.id, Some(up)).await.unwrap();
    assert!(matches!(r, SyncApplyResult::Blocked { .. }), "{r:?}");
    assert!(dest.join("a.yaml").exists());
    assert_eq!(f.state.operation_list(&p.id).unwrap().len(), 2);
}

// ================= 同期 =================

#[tokio::test]
async fn syncing_twice_in_a_row_changes_nothing_the_second_time() {
    let f = fx();
    let (d, e) = spec_tree();
    let p = import_new(&f, path_input(&e)).await;
    assert!(sync_diff(&f, &p.id).await.is_empty());
    let (diff1, r1) = sync_apply(&f, &p.id).await;
    assert!(diff1.is_empty());
    assert_eq!((r1.added, r1.changed, r1.removed, r1.renamed), (0, 0, 0, 0));
    // 編集 → 1 回目は差分、2 回目は 0
    fs::write(
        d.path().join("a.yaml"),
        A_YAML.replace("summary: A", "summary: A2"),
    )
    .unwrap();
    let (diff2, r2) = sync_apply(&f, &p.id).await;
    assert_eq!(diff2.changed.len(), 1);
    assert_eq!(r2.changed, 1);
    let (diff3, r3) = sync_apply(&f, &p.id).await;
    assert!(diff3.is_empty());
    assert_eq!((r3.added, r3.changed, r3.removed), (0, 0, 0));
    assert!(sync_diff(&f, &p.id).await.is_empty());
}

#[tokio::test]
async fn source_changed_sees_touched_and_removed_files_and_a_check_clears_it() {
    let f = fx();
    let (d, e) = spec_tree();
    let p = import_new(&f, path_input(&e)).await;
    assert!(!f.state.openapi_source_changed(&p.id).unwrap().changed);
    // 触っただけ（中身は同じ）
    touch(&d.path().join("a.yaml"), 120);
    let c = f.state.openapi_source_changed(&p.id).unwrap();
    assert!(c.changed);
    assert_eq!(c.changed_files, vec!["a.yaml"]);
    // 同期の確認: 中身は同じなので差分なし、changedFiles も空。覚え直されて印は消える
    let SyncCheckResult::Ready {
        diff,
        changed_files,
    } = sync_check(&f, &p.id).await
    else {
        panic!()
    };
    assert!(diff.is_empty());
    assert!(
        changed_files.is_empty(),
        "中身が同じなら変更ファイルにしない: {changed_files:?}"
    );
    assert!(
        !f.state.openapi_source_changed(&p.id).unwrap().changed,
        "覚え直したので印は消える"
    );
    // 古い日時に戻っても（git checkout など）変更として見る
    touch(&d.path().join("b.yaml"), -3600);
    assert!(f.state.openapi_source_changed(&p.id).unwrap().changed);
    let _ = sync_check(&f, &p.id).await;
    assert!(!f.state.openapi_source_changed(&p.id).unwrap().changed);
    // 参照先のファイルが消えた
    fs::remove_file(d.path().join("b.yaml")).unwrap();
    let c = f.state.openapi_source_changed(&p.id).unwrap();
    assert_eq!(c.changed_files, vec!["b.yaml"]);
    // 入口が新しいファイルを参照するように書き換わった（入口の日時が変わる）
    fs::write(d.path().join("b.yaml"), B_YAML).unwrap();
    fs::write(
        d.path().join("new.yaml"),
        "get:\n  operationId: getNew\n  responses:\n    '200': {description: ok}\n",
    )
    .unwrap();
    let mut entry = fs::read_to_string(&e).unwrap();
    entry.push_str("  /new: {$ref: './new.yaml'}\n");
    fs::write(&e, entry).unwrap();
    touch(&e, 300);
    let c = f.state.openapi_source_changed(&p.id).unwrap();
    assert!(
        c.changed_files.contains(&"openapi.yaml".to_string()),
        "{c:?}"
    );
    let SyncCheckResult::Ready {
        diff,
        changed_files,
    } = sync_check(&f, &p.id).await
    else {
        panic!()
    };
    assert_eq!(diff.added.len(), 1);
    assert!(
        changed_files.contains(&"new.yaml".to_string()),
        "新しい参照ファイルが変更ファイルに出る: {changed_files:?}"
    );
}

#[tokio::test]
async fn a_blocked_sync_leaves_operations_files_and_drafts_exactly_as_they_were() {
    let f = fx();
    let (d, e) = spec_tree();
    let p = import_new(&f, path_input(&e)).await;
    save_draft(&f, &p.id, key("getC"), vec![kv("q", "keep-me")]);
    let snapshot = |f: &Fx| {
        let list = serde_json::to_string(&f.state.operation_list(&p.id).unwrap()).unwrap();
        let proj = serde_json::to_string(&f.state.project_list().unwrap()).unwrap();
        let open =
            serde_json::to_string(&f.state.operation_open(&p.id, &key("getC")).unwrap()).unwrap();
        (list, proj, open)
    };
    let before = snapshot(&f);
    type Mutation = Box<dyn Fn()>;
    let cases: Vec<(&str, Mutation)> = vec![
        (
            "壊れた yaml",
            Box::new({
                let a = d.path().join("a.yaml");
                move || fs::write(&a, "p: [1, 2\n  get: x\n").unwrap()
            }),
        ),
        (
            "足りないファイル",
            Box::new({
                let a = d.path().join("a.yaml");
                let b = d.path().join("b.yaml");
                move || {
                    fs::write(&a, A_YAML).unwrap();
                    fs::remove_file(&b).unwrap();
                }
            }),
        ),
        (
            "入口がない",
            Box::new({
                let b = d.path().join("b.yaml");
                let e = e.clone();
                move || {
                    fs::write(&b, B_YAML).unwrap();
                    fs::rename(&e, e.with_extension("moved")).unwrap();
                }
            }),
        ),
        (
            "openapi でない",
            Box::new({
                let e = e.clone();
                move || fs::write(&e, "title: not openapi\n").unwrap()
            }),
        ),
        (
            "Swagger 2.0",
            Box::new({
                let e = e.clone();
                move || {
                    fs::write(
                        &e,
                        "swagger: '2.0'\ninfo: {title: x, version: '1'}\npaths: {}\n",
                    )
                    .unwrap()
                }
            }),
        ),
    ];
    for (name, mutate) in cases {
        mutate();
        match sync_check(&f, &p.id).await {
            SyncCheckResult::Blocked { problems } => assert!(!problems.is_empty(), "{name}"),
            other => panic!("{name}: 止まるはず: {other:?}"),
        }
        match f.state.openapi_sync_apply(&p.id, None).await.unwrap() {
            SyncApplyResult::Blocked { problems } => {
                for pr in &problems {
                    assert!(!pr.describe().is_empty());
                }
            }
            other => panic!("{name}: 止まるはず: {other:?}"),
        }
        assert_eq!(snapshot(&f), before, "{name}: 状態が変わった");
        // 更新日時の確認は読めないファイルがあっても落ちない
        let _ = f.state.openapi_source_changed(&p.id).unwrap();
    }
    // 元に戻せば同期できる
    fs::rename(e.with_extension("moved"), &e)
        .unwrap_or_else(|_| fs::write(&e, format!("{HEAD}{ENTRY}")).unwrap());
    fs::write(&e, format!("{HEAD}{ENTRY}")).unwrap();
    assert!(sync_diff(&f, &p.id).await.is_empty());
    assert_eq!(
        f.state
            .operation_open(&p.id, &key("getC"))
            .unwrap()
            .request
            .params
            .iter()
            .find(|x| x.name == "q")
            .unwrap()
            .value,
        "keep-me"
    );
}

#[tokio::test]
async fn blocked_problems_name_the_missing_file_and_the_referencing_line_through_the_app() {
    let f = fx();
    let (d, e) = spec_tree();
    let p = import_new(&f, path_input(&e)).await;
    fs::remove_file(d.path().join("b.yaml")).unwrap();
    let SyncCheckResult::Blocked { problems } = sync_check(&f, &p.id).await else {
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
    assert_eq!(file, "b.yaml");
    assert_eq!(referenced_from[0].file, "openapi.yaml");
    // `  /b: {$ref: './b.yaml'}` の行（HEAD 2 行 + ENTRY の 6 行目 = 8 行目）
    assert_eq!(referenced_from[0].line, Some(8));
}

#[tokio::test]
async fn restoring_a_removed_api_is_reported_the_same_in_the_diff_and_the_report() {
    let f = fx();
    let (d, e) = spec_tree();
    let p = import_new(&f, path_input(&e)).await;
    save_draft(&f, &p.id, key("postB"), vec![kv("x", "1")]);
    let original = fs::read_to_string(&e).unwrap();
    fs::write(&e, original.replace("  /b: {$ref: './b.yaml'}\n", "")).unwrap();
    let (diff, r) = sync_apply(&f, &p.id).await;
    assert_eq!(
        (diff.removed.len(), r.removed, r.kept_as_deleted),
        (1, 1, 1)
    );
    let list = f.state.operation_list(&p.id).unwrap();
    assert!(
        list.iter()
            .find(|i| i.op_key == key("postB"))
            .unwrap()
            .deleted
    );
    // 戻す
    fs::write(&e, original).unwrap();
    let _ = &d;
    let diff = sync_diff(&f, &p.id).await;
    let (applied, report) = sync_apply(&f, &p.id).await;
    assert_eq!(diff, applied);
    let list = f.state.operation_list(&p.id).unwrap();
    let b = list.iter().find(|i| i.op_key == key("postB")).unwrap();
    assert!(!b.deleted && b.has_draft);
    // 差分の「追加」と反映の件数が食い違わない
    assert_eq!(
        report.added as usize + report.changed as usize,
        applied.added.len() + applied.changed.len(),
        "diff={applied:?} report={report:?}"
    );
    assert_eq!(
        report.added as usize,
        applied.added.len(),
        "diff では追加、report では変更になっている"
    );
}

#[tokio::test]
async fn unselected_apis_are_not_added_by_sync_and_their_edits_do_not_pollute_the_diff() {
    let f = fx();
    let (d, e) = spec_tree();
    let r = f
        .state
        .openapi_import(ImportRequest {
            source: path_input(&e),
            project_id: None,
            project_name: None,
            selected: Some(vec![key("getC")]),
            options: ImportOptions::default(),
        })
        .await
        .unwrap();
    let ImportResult::Imported {
        project,
        operation_count,
        ..
    } = r
    else {
        panic!()
    };
    assert_eq!(operation_count, 1);
    assert!(
        sync_diff(&f, &project.id).await.is_empty(),
        "選ばなかった API は「追加」にならない"
    );
    // 選ばなかった getA を編集・postB を削除
    fs::write(
        d.path().join("a.yaml"),
        A_YAML.replace("summary: A", "summary: A-new"),
    )
    .unwrap();
    let original = fs::read_to_string(&e).unwrap();
    fs::write(&e, original.replace("  /b: {$ref: './b.yaml'}\n", "")).unwrap();
    let diff = sync_diff(&f, &project.id).await;
    // ツリーに出ていない API の変更・削除は、画面の差分（確認ダイアログ）に出さないのが自然
    assert!(
        diff.changed.is_empty() && diff.removed.is_empty() && diff.added.is_empty(),
        "取り込んでいない API の変更が差分に出ている: {diff:#?}"
    );
}

#[tokio::test]
async fn renames_param_removal_and_new_params_combine_without_losing_input() {
    let f = fx();
    let (d, e) = spec_tree();
    let p = import_new(&f, path_input(&e)).await;
    save_draft(
        &f,
        &p.id,
        key("getC"),
        vec![kv("q", "Q1"), kv("limit", "10")],
    );
    save_draft(&f, &p.id, key("getA"), vec![kv("z", "Z")]);
    // getC: operationId を変え、limit を消し、page を足す。getA は path を移す（METHOD path が変わる）
    let mut text = fs::read_to_string(&e).unwrap();
    text = text
        .replace("operationId: getC", "operationId: fetchC")
        .replace(
            "        - {name: limit, in: query, schema: {type: integer}}\n",
            "        - {name: page, in: query, schema: {type: integer}}\n",
        )
        .replace(
            "  /a: {$ref: './a.yaml#/p'}",
            "  /a2: {$ref: './a.yaml#/p'}",
        );
    fs::write(&e, text).unwrap();
    let diff = sync_diff(&f, &p.id).await;
    let c = diff
        .changed
        .iter()
        .find(|c| c.entry.op_key == key("fetchC"))
        .expect("fetchC");
    assert_eq!(c.renamed_from, Some(key("getC")));
    assert_eq!(c.removed_params, vec!["limit"]);
    // getA は同じ operationId なので、path が変わっても同じ API（入力値も残る）
    assert!(
        diff.added.is_empty() && diff.removed.is_empty(),
        "{diff:#?}"
    );
    let (_, report) = sync_apply(&f, &p.id).await;
    assert_eq!(report.renamed, 1);
    let open = f.state.operation_open(&p.id, &key("fetchC")).unwrap();
    assert!(open.has_draft);
    let by = |n: &str| open.request.params.iter().find(|x| x.name == n).cloned();
    assert_eq!(by("q").unwrap().value, "Q1");
    assert!(by("q").unwrap().enabled);
    let limit = by("limit").unwrap();
    assert_eq!(limit.value, "10", "yaml から消えたパラメータの値は残る");
    assert!(!limit.enabled, "送信から外す");
    assert_eq!(open.missing_in_yaml.len(), 1);
    assert_eq!(open.missing_in_yaml[0].name, "limit");
    assert!(
        by("page").is_some_and(|p| !p.enabled),
        "新しいパラメータは無効で足される"
    );
    assert!(
        f.state
            .operation_open(&p.id, &key("getA"))
            .unwrap()
            .has_draft
    );
    let _ = d;
}

// ================= servers → 環境 =================

#[tokio::test]
async fn servers_become_environments_once_and_sync_never_recreates_them() {
    let f = fx();
    let d = tempfile::tempdir().unwrap();
    let e = d.path().join("openapi.yaml");
    fs::write(
        &e,
        format!(
            "{HEAD}servers:\n  - {{url: 'https://{{env}}.example.com:{{port}}/v1', description: 本番, variables: {{env: {{default: api}}, port: {{default: '443'}}}}}}\n  - {{url: 'http://localhost:8080', description: 本番}}\n  - {{url: 'http://localhost:9090'}}\n  - {{url: '/relative/base'}}\n  - {{url: '', description: 空}}\n  - {{url: 'https://x.example/{{nodefault}}', description: グローバル}}\npaths: {{}}\n"
        ),
    )
    .unwrap();
    let r = f.state.openapi_import(req(path_input(&e))).await.unwrap();
    let ImportResult::Imported {
        project,
        created_environments,
        ..
    } = r
    else {
        panic!("{r:?}")
    };
    // 同名（本番）は 2 つ目を作らない。URL だけのものは URL が名前。「グローバル」は予約名なので URL の名前
    assert_eq!(
        created_environments,
        vec![
            "本番".to_string(),
            "http://localhost:9090".to_string(),
            "/relative/base".to_string(),
            "https://x.example/{nodefault}".to_string(),
        ]
    );
    let envs = f.state.env_list(&project.id).unwrap();
    let base = |name: &str| {
        let e = envs
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        match &e.vars[0].value {
            nekoreq_core::VarValue::Plain(v) => v.clone(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(base("本番"), "https://api.example.com:443/v1");
    // ユーザーが環境を消して、同期しても戻らない。再取り込み（統合）では作り直す（取り込みのときだけ作る）
    let del = envs.iter().find(|e| e.name == "本番").unwrap();
    f.state.env_delete(&del.id).unwrap();
    let _ = sync_apply(&f, &project.id).await;
    assert!(f
        .state
        .env_list(&project.id)
        .unwrap()
        .iter()
        .all(|e| e.name != "本番"));
    // 取り込み直しても重複はしない
    let mut r2 = req(path_input(&e));
    r2.project_id = Some(project.id.clone());
    let ImportResult::Imported {
        created_environments,
        ..
    } = f.state.openapi_import(r2).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(created_environments, vec!["本番".to_string()]);
    let names: Vec<_> = f
        .state
        .env_list(&project.id)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(
        names.iter().collect::<HashSet<_>>().len(),
        names.len(),
        "{names:?}"
    );
}

// ================= 性能 =================

fn synthetic(dir: &Path, n_ops: usize, files: usize) -> PathBuf {
    let mut schemas = String::new();
    for i in 0..50 {
        schemas.push_str(&format!(
            "S{i}:\n  type: object\n  required: [id]\n  properties:\n    id: {{type: integer, minimum: 1}}\n    name: {{type: string, maxLength: 50}}\n    status: {{type: string, enum: [A, B, C]}}\n"
        ));
        if i % 5 != 4 {
            schemas.push_str(&format!("    child: {{$ref: '#/S{}'}}\n", i + 1));
        }
    }
    fs::create_dir_all(dir.join("paths")).unwrap();
    fs::write(dir.join("common.yaml"), schemas).unwrap();
    let per = n_ops / files;
    let mut entry =
        format!("{HEAD}servers:\n  - {{url: 'http://localhost:1', description: L}}\ntags:\n");
    for t in 0..20 {
        entry.push_str(&format!("  - name: tag{t}\n"));
    }
    entry.push_str("paths:\n");
    let mut k = 0;
    for f in 0..files {
        let mut body = String::new();
        for _ in 0..per {
            body.push_str(&format!(
                "/r/{k}:\n  get:\n    operationId: get{k}\n    summary: 取得 {k}\n    description: |\n      説明 {k}\n      | status | code | 状況 |\n      | --- | --- | --- |\n      | 404 | NOT_FOUND | なし |\n    tags: [tag{}]\n    parameters:\n      - {{name: q, in: query, schema: {{type: string}}}}\n    responses:\n      '200':\n        description: ok\n        content:\n          application/json:\n            schema: {{$ref: '../common.yaml#/S{}'}}\n      '404': {{description: nf}}\n",
                k % 20,
                k % 50
            ));
            k += 1;
        }
        fs::write(dir.join(format!("paths/f{f}.yaml")), body).unwrap();
    }
    k = 0;
    for f in 0..files {
        for _ in 0..per {
            entry.push_str(&format!(
                "  /r/{k}: {{$ref: './paths/f{f}.yaml#/~1r~1{k}'}}\n"
            ));
            k += 1;
        }
    }
    let e = dir.join("openapi.yaml");
    fs::write(&e, entry).unwrap();
    e
}

#[tokio::test]
async fn performance_with_1000_apis_through_the_app() {
    let f = fx();
    let d = tempfile::tempdir().unwrap();
    let e = synthetic(d.path(), 1000, 25);
    let t = Instant::now();
    let r = f.state.openapi_preview(path_input(&e), None).await.unwrap();
    let preview_t = t.elapsed();
    let PreviewResult::Ready { preview } = r else {
        panic!("{r:?}")
    };
    assert_eq!(preview.operation_count, 1000);
    let t = Instant::now();
    let p = import_new(&f, path_input(&e)).await;
    let import_t = t.elapsed();
    let t = Instant::now();
    let list = f.state.operation_list(&p.id).unwrap();
    let list_t = t.elapsed();
    assert_eq!(list.len(), 1000);
    let t = Instant::now();
    let d1 = sync_diff(&f, &p.id).await;
    let check_t = t.elapsed();
    assert!(d1.is_empty());
    // 1 件変えて同期
    let mut f0 = fs::read_to_string(d.path().join("paths/f0.yaml")).unwrap();
    f0 = f0.replacen("summary: 取得 0", "summary: 変更", 1);
    fs::write(d.path().join("paths/f0.yaml"), f0).unwrap();
    let t = Instant::now();
    let (diff, report) = sync_apply(&f, &p.id).await;
    let apply_t = t.elapsed();
    assert_eq!((diff.changed.len(), report.changed), (1, 1));
    let t = Instant::now();
    let _ = f.state.operation_get(&p.id, &key("get5")).unwrap();
    let _ = f.state.operation_open(&p.id, &key("get5")).unwrap();
    let one_t = t.elapsed();
    let t = Instant::now();
    let _ = f.state.openapi_source_changed(&p.id).unwrap();
    let changed_t = t.elapsed();
    let db_kb = fs::metadata(f.data.join("data.sqlite"))
        .map(|m| m.len() / 1024)
        .unwrap_or(0);
    eprintln!(
        "PERF(app, debug build) 1000 ops: preview={preview_t:?} import={import_t:?} operation_list={list_t:?} sync_check(差分なし)={check_t:?} sync_apply(1 件変更)={apply_t:?} get+open={one_t:?} source_changed={changed_t:?} sqlite={db_kb} KB"
    );
    assert!(import_t < Duration::from_secs(30), "{import_t:?}");
}

#[tokio::test]
async fn performance_with_the_real_fixture_through_the_app() {
    let Some(dir) = std::env::var_os("NEKOREQ_FIXTURE_DIR") else {
        eprintln!("NEKOREQ_FIXTURE_DIR が無いのでスキップ");
        return;
    };
    let entry = PathBuf::from(dir).join("openapi.yaml");
    if !entry.exists() {
        return;
    }
    let f = fx();
    let t = Instant::now();
    let p = import_new(&f, path_input(&entry)).await;
    let import_t = t.elapsed();
    let t = Instant::now();
    let list = f.state.operation_list(&p.id).unwrap();
    let list_t = t.elapsed();
    let t = Instant::now();
    let d = sync_diff(&f, &p.id).await;
    let check_t = t.elapsed();
    assert!(d.is_empty());
    let t = Instant::now();
    let (d2, _) = sync_apply(&f, &p.id).await;
    let apply_t = t.elapsed();
    assert!(d2.is_empty());
    let t = Instant::now();
    let c = f.state.openapi_source_changed(&p.id).unwrap();
    let changed_t = t.elapsed();
    assert!(!c.changed);
    let db_kb = fs::metadata(f.data.join("data.sqlite"))
        .map(|m| m.len() / 1024)
        .unwrap_or(0);
    eprintln!(
        "PERF(app, debug build) real fixture {} ops: import={import_t:?} operation_list={list_t:?} sync_check={check_t:?} sync_apply(差分なし)={apply_t:?} source_changed={changed_t:?} sqlite={db_kb} KB",
        list.len()
    );
    assert!(
        check_t < Duration::from_secs(1),
        "同期ボタンから差分まで 1 秒以内（非機能要件）: {check_t:?}"
    );
    let _ = SourceKind::Path;
}
