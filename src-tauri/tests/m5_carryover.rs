//! M3・M4 から持ち越した 3 件（M5）を `AppState` 経由で確かめる。
//!
//! - 同期の `needsConfirmation` を、確認したホスト（`allowedHosts`）を渡して続けられる。確認したホストは保存される。
//! - `editor_open` が行番号つきでエディタを起動する（設定のコマンドか、自動で見つけたエディタ）。取り込み元の外は開けない。
//! - （`examples` の複数保持は crates/openapi/tests/semantics.rs。`ForeignRef.host` の説明は型のドキュメント。）

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nekoreq_core::{AppSettings, ImportOptions};
use nekoreq_lib::editor::EditorLaunch;
use nekoreq_lib::error::AppError;
use nekoreq_lib::openapi::{
    ImportRequest, ImportResult, SourceInput, SyncApplyResult, SyncCheckResult,
};
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

fn import_req(source: SourceInput) -> ImportRequest {
    ImportRequest {
        source,
        project_id: None,
        project_name: None,
        selected: None,
        options: ImportOptions::default(),
    }
}

fn path_input(p: &Path) -> SourceInput {
    SourceInput::Path {
        path: p.display().to_string(),
        allowed_hosts: vec![],
    }
}

async fn import_path(f: &Fx, entry: &Path) -> nekoreq_core::Project {
    match f
        .state
        .openapi_import(import_req(path_input(entry)))
        .await
        .unwrap()
    {
        ImportResult::Imported { project, .. } => *project,
        other => panic!("取り込めるはず: {other:?}"),
    }
}

// ---- 同期の確認済みホスト ----

const LOCAL_YAML: &str = "openapi: 3.0.3\ninfo: {title: S, version: '1'}\npaths:\n  /local:\n    get:\n      operationId: getLocal\n      responses:\n        '200': {description: ok}\n";

async fn foreign_server() -> MockServer {
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/s.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "paths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200':\n          description: ok\n",
        ))
        .mount(&other)
        .await;
    other
}

fn yaml_with_foreign_ref(origin: &str) -> String {
    format!("{LOCAL_YAML}  /a:\n    $ref: '{origin}/s.yaml#/paths/~1a'\n")
}

#[tokio::test]
async fn sync_check_continues_with_allowed_hosts_and_remembers_them() {
    let f = fx();
    let other = foreign_server().await;
    let origin = other.uri();
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("openapi.yaml");
    std::fs::write(&entry, LOCAL_YAML).unwrap();
    let project = import_path(&f, &entry).await;
    assert!(project.source.as_ref().unwrap().allowed_hosts.is_empty());

    // yaml に別ホストの参照を足して同期 → 確認待ち（読みに行かない）
    std::fs::write(&entry, yaml_with_foreign_ref(&origin)).unwrap();
    let SyncCheckResult::NeedsConfirmation { foreign } = f
        .state
        .openapi_sync_check(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!("確認待ちのはず")
    };
    assert_eq!(foreign.len(), 1);
    assert_eq!(foreign[0].host, origin, "host は origin の形");
    assert!(other.received_requests().await.unwrap().is_empty());
    // 確認待ちのままでは何も保存されない
    let stored = f.state.project_list().unwrap().remove(0);
    assert!(stored.source.unwrap().allowed_hosts.is_empty());

    // 空の一覧や別のホストを渡しても、確認にはならない
    let SyncCheckResult::NeedsConfirmation { .. } = f
        .state
        .openapi_sync_check(
            &project.id,
            None,
            Some(vec!["https://other.example".into()]),
        )
        .await
        .unwrap()
    else {
        panic!("別のホストの確認では足りない")
    };

    // 確認したホストを渡すと続けられる。差分は「追加」1 件
    let SyncCheckResult::Ready { diff, .. } = f
        .state
        .openapi_sync_check(&project.id, None, Some(vec![foreign[0].host.clone()]))
        .await
        .unwrap()
    else {
        panic!("続けられるはず")
    };
    assert_eq!(diff.added.len(), 1, "{diff:?}");
    assert!(!other.received_requests().await.unwrap().is_empty());
    // 確認したホストは取り込み元に保存される
    let stored = f.state.project_list().unwrap().remove(0);
    assert_eq!(stored.source.unwrap().allowed_hosts, vec![origin.clone()]);

    // 以後は渡さなくても確認を求めず、反映もできる
    let SyncApplyResult::Applied {
        report, project, ..
    } = f
        .state
        .openapi_sync_apply(&project.id, None, None)
        .await
        .unwrap()
    else {
        panic!("確認済みなので聞き直さない")
    };
    assert_eq!(report.added, 1);
    assert_eq!(project.source.unwrap().allowed_hosts, vec![origin]);
}

#[tokio::test]
async fn sync_apply_with_allowed_hosts_saves_them_with_the_changes() {
    let f = fx();
    let other = foreign_server().await;
    let origin = other.uri();
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("openapi.yaml");
    std::fs::write(&entry, LOCAL_YAML).unwrap();
    let project = import_path(&f, &entry).await;
    std::fs::write(&entry, yaml_with_foreign_ref(&origin)).unwrap();

    assert!(matches!(
        f.state
            .openapi_sync_apply(&project.id, None, None)
            .await
            .unwrap(),
        SyncApplyResult::NeedsConfirmation { .. }
    ));
    // 反映が止まったときは何も書かない
    assert_eq!(f.state.operation_list(&project.id).unwrap().len(), 1);

    let SyncApplyResult::Applied {
        report, project, ..
    } = f
        .state
        .openapi_sync_apply(
            &project.id,
            None,
            Some(vec![format!("  {origin}  "), String::new()]),
        )
        .await
        .unwrap()
    else {
        panic!("確認したので反映できる")
    };
    assert_eq!(report.added, 1);
    // 前後の空白と空は除いて保存する
    assert_eq!(
        project.source.as_ref().unwrap().allowed_hosts,
        vec![origin.clone()]
    );
    assert_eq!(f.state.operation_list(&project.id).unwrap().len(), 2);

    // 同じホストをもう一度渡しても重複しない
    let SyncCheckResult::Ready { .. } = f
        .state
        .openapi_sync_check(&project.id, None, Some(vec![origin.to_uppercase()]))
        .await
        .unwrap()
    else {
        panic!()
    };
    let stored = f.state.project_list().unwrap().remove(0);
    assert_eq!(stored.source.unwrap().allowed_hosts, vec![origin]);
}

// ---- エディタ ----

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

fn split_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/openapi/tests/fixtures/split"),
        tmp.path(),
    );
    let entry = tmp.path().join("openapi.yaml");
    (tmp, entry)
}

#[test]
fn app_settings_store_the_editor_command_and_reject_a_broken_one() {
    let f = fx();
    assert_eq!(f.state.app_settings_get().unwrap(), AppSettings::default());
    let saved = f
        .state
        .app_settings_set(&AppSettings {
            editor_command: Some("  code -g {file}:{line}  ".into()),
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(
        saved.editor_command.as_deref(),
        Some("code -g {file}:{line}")
    );
    assert_eq!(f.state.app_settings_get().unwrap(), saved);
    // 空は未設定
    let saved = f
        .state
        .app_settings_set(&AppSettings {
            editor_command: Some("   ".into()),
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(saved.editor_command, None);
    // 引用符が閉じていない
    let e = f
        .state
        .app_settings_set(&AppSettings {
            editor_command: Some("code \"{file}".into()),
            ..AppSettings::default()
        })
        .unwrap_err();
    assert!(matches!(e, AppError::Validation(_)), "{e:?}");
    assert_eq!(f.state.app_settings_get().unwrap().editor_command, None);
}

#[cfg(unix)]
mod editor_unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// 受け取った引数を 1 行ずつ書き出すだけのエディタ。
    fn fake_editor(dir: &Path) -> (PathBuf, PathBuf) {
        let out = dir.join("args.txt");
        let script = dir.join("fake-editor.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script, out)
    }

    fn wait_lines(out: &Path, n: usize) -> Vec<String> {
        for _ in 0..100 {
            let text = std::fs::read_to_string(out).unwrap_or_default();
            if text.lines().count() >= n {
                return text.lines().map(String::from).collect();
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("エディタが呼ばれなかった");
    }

    #[tokio::test]
    async fn editor_open_passes_the_file_and_line_to_the_configured_editor() {
        let f = fx();
        let (_tmp, entry) = split_fixture();
        let project = import_path(&f, &entry).await;
        let tools = tempfile::tempdir().unwrap();
        let (script, out) = fake_editor(tools.path());
        f.state
            .app_settings_set(&AppSettings {
                editor_command: Some(format!("{} -g {{file}}:{{line}}", script.display())),
                ..AppSettings::default()
            })
            .unwrap();

        // 設計タブの「paths/users.yaml:12」をクリックしたとき
        let launched = f
            .state
            .editor_open(&project.id, "paths/users.yaml", Some(12))
            .unwrap();
        assert!(
            matches!(launched, EditorLaunch::Spawned { .. }),
            "{launched:?}"
        );
        let lines = wait_lines(&out, 2);
        let target = f
            .state
            .editor_target(&project.id, "paths/users.yaml")
            .unwrap();
        assert_eq!(
            lines,
            vec!["-g".to_string(), format!("{}:12", target.display())]
        );
        assert!(target.ends_with("paths/users.yaml"));
    }

    #[tokio::test]
    async fn editor_open_refuses_files_outside_the_imported_ones_without_starting_anything() {
        let f = fx();
        let (_tmp, entry) = split_fixture();
        let project = import_path(&f, &entry).await;
        let tools = tempfile::tempdir().unwrap();
        let (script, out) = fake_editor(tools.path());
        f.state
            .app_settings_set(&AppSettings {
                editor_command: Some(format!("{} {{file}}", script.display())),
                ..AppSettings::default()
            })
            .unwrap();
        for file in [
            "../../../etc/passwd",
            "/etc/passwd",
            "paths/../../outside.yaml",
            "not-imported.yaml",
            "",
        ] {
            let e = f.state.editor_open(&project.id, file, Some(1)).unwrap_err();
            assert!(matches!(e, AppError::Validation(_)), "{file}: {e:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!out.exists(), "エディタが起動されてしまった");
        // プロジェクトがなければ NotFound
        assert!(matches!(
            f.state.editor_open("nope", "openapi.yaml", None),
            Err(AppError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_broken_editor_setting_is_an_error_not_a_silent_fallback() {
        let f = fx();
        let (_tmp, entry) = split_fixture();
        let project = import_path(&f, &entry).await;
        f.state
            .app_settings_set(&AppSettings {
                editor_command: Some("/no/such/editor-binary {file}".into()),
                ..AppSettings::default()
            })
            .unwrap();
        let e = f
            .state
            .editor_open(&project.id, "openapi.yaml", Some(3))
            .unwrap_err();
        assert!(
            matches!(&e, AppError::Io(m) if m.contains("エディタを起動できませんでした")),
            "{e:?}"
        );
    }
}
