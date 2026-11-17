//! M3・M4 で足したリポジトリ（取り込み元・Operation のキャッシュ・同期の反映・servers から環境）とマイグレーション v4 のテスト。

use std::collections::HashSet;

use nekoreq_core::{
    ApiSource, Body, ImportOptions, KeyValue, OpKey, Operation, ParamIn, ParamSpec, RequestDraft,
    ServerInfo, SourceFile, SourceKind, SpecMeta,
};
use rusqlite::Connection;

use super::*;

fn param(name: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        location: ParamIn::Query,
        required: false,
        description: None,
        example: None,
        default: None,
        schema: None,
    }
}

fn op(id: &str, method: &str, path: &str, hash: &str) -> Operation {
    Operation {
        id: String::new(),
        project_id: String::new(),
        op_key: OpKey::Id(id.into()),
        operation_id: Some(id.into()),
        method: method.into(),
        path: path.into(),
        tags: vec!["t".into()],
        summary: Some(format!("summary {id}")),
        description: None,
        parameters: vec![param("q"), param("page")],
        request_body: None,
        responses: vec![],
        security: None,
        source_file: Some("a.yaml".into()),
        source_line: Some(3),
        content_hash: hash.into(),
    }
}

fn source() -> ApiSource {
    ApiSource {
        id: String::new(),
        kind: SourceKind::Path,
        entry: "/x/openapi.yaml".into(),
        files: vec![SourceFile {
            path: "openapi.yaml".into(),
            modified_at: Some("2026-01-01T00:00:00.000Z".into()),
            hash: "abc".into(),
        }],
        openapi_version: "3.0.3".into(),
        synced_at: Some("2026-01-01T00:00:00.000Z".into()),
        has_token: false,
        allowed_hosts: vec!["example.com".into()],
        options: ImportOptions::default(),
        meta: SpecMeta {
            title: "T".into(),
            version: "1".into(),
            ..Default::default()
        },
    }
}

fn draft(project: &str, key: OpKey, names: &[&str]) -> RequestDraft {
    RequestDraft {
        id: String::new(),
        project_id: project.into(),
        op_key: key,
        params: names
            .iter()
            .map(|n| KeyValue {
                location: ParamIn::Query,
                name: (*n).into(),
                value: format!("v-{n}"),
                enabled: true,
            })
            .collect(),
        body: Body::None,
        auth: None,
        updated_at: String::new(),
    }
}

fn keys(db: &Database, project: &str) -> Vec<(String, bool, bool)> {
    db.operations_list(project)
        .unwrap()
        .into_iter()
        .map(|s| {
            (
                s.operation.op_key.to_string(),
                s.imported,
                s.deleted_at.is_some(),
            )
        })
        .collect()
}

#[test]
fn migration_v3_to_v4_keeps_existing_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        for sql in &crate::migrations::MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(
            "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p1', 'P', 't', 't');
             INSERT INTO api_sources (id, project_id, kind, entry, files_json, openapi_version)
               VALUES ('s1', 'p1', 'path', '/x/openapi.yaml', '[]', '3.0.3');
             INSERT INTO operations (id, project_id, op_key, method, path, content_hash)
               VALUES ('o1', 'p1', 'id:a', 'GET', '/a', 'h');
             INSERT INTO request_drafts (id, project_id, op_key, body_json, updated_at)
               VALUES ('d1', 'p1', 'id:a', '{\"kind\":\"none\"}', 't');",
        )
        .unwrap();
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
    let p = db.project_get("p1").unwrap().unwrap();
    let s = p.source.unwrap();
    assert!(!s.has_token && s.allowed_hosts.is_empty());
    assert_eq!(s.options, ImportOptions::default());
    let ops = db.operations_list("p1").unwrap();
    assert!(ops[0].imported && ops[0].deleted_at.is_none());
    assert!(db
        .draft_stale_params("p1", &OpKey::Id("a".into()))
        .unwrap()
        .is_empty());
}

#[test]
fn first_import_saves_source_and_only_selected_operations_are_imported() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let ops = vec![
        op("a", "GET", "/a", "1"),
        op("b", "GET", "/b", "1"),
        op("c", "POST", "/c", "1"),
    ];
    let sel: HashSet<OpKey> = [OpKey::Id("a".into()), OpKey::Id("c".into())].into();
    let r = db
        .operations_apply(&p.id, &source(), &ops, &Selection::Only(sel))
        .unwrap();
    assert_eq!((r.added, r.changed, r.removed), (3, 0, 0));
    assert_eq!(
        keys(&db, &p.id),
        vec![
            ("a".into(), true, false),
            ("b".into(), false, false),
            ("c".into(), true, false)
        ]
    );
    let s = db.project_get(&p.id).unwrap().unwrap().source.unwrap();
    assert_eq!(s.entry, "/x/openapi.yaml");
    assert_eq!(s.files.len(), 1);
    assert_eq!(s.allowed_hosts, vec!["example.com"]);
    assert_eq!(s.meta.title, "T");
    assert!(!s.id.is_empty());
    // 保存した Operation は読み書きで同じ
    let got = db
        .operation_get(&p.id, &OpKey::Id("a".into()))
        .unwrap()
        .unwrap();
    assert_eq!(got.operation.parameters.len(), 2);
    assert_eq!(got.operation.summary.as_deref(), Some("summary a"));
    assert!(!got.operation.id.is_empty());
    assert_eq!(got.operation.project_id, p.id);

    // もう一度反映しても、選ばなかった b は「追加」にならず、取り込み済みにもならない
    let r = db
        .operations_apply(&p.id, &source(), &ops, &Selection::AllNew)
        .unwrap();
    assert_eq!((r.added, r.changed, r.removed), (0, 0, 0));
    assert_eq!(keys(&db, &p.id)[1], ("b".into(), false, false));
    // 統合で選ぶと取り込み済みになる
    let sel: HashSet<OpKey> = [OpKey::Id("b".into())].into();
    db.operations_apply(&p.id, &source(), &ops, &Selection::Only(sel))
        .unwrap();
    assert_eq!(keys(&db, &p.id)[1], ("b".into(), true, false));
}

#[test]
fn applying_changes_keeps_drafts_and_marks_removed_apis() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let v1 = vec![
        op("a", "GET", "/a", "1"),
        op("b", "GET", "/b", "1"),
        op("c", "GET", "/c", "1"),
    ];
    db.operations_apply(&p.id, &source(), &v1, &Selection::AllNew)
        .unwrap();
    db.draft_save(&draft(&p.id, OpKey::Id("a".into()), &["q", "page"]))
        .unwrap();
    db.draft_save(&draft(&p.id, OpKey::Id("b".into()), &["q"]))
        .unwrap();

    // a は変更、b は削除（入力値あり）、c は削除（入力値なし）、d は追加
    let mut a2 = op("a", "GET", "/a", "2");
    a2.summary = Some("新しい summary".into());
    a2.parameters = vec![param("q")]; // page が yaml から消えた
    let v2 = vec![a2, op("d", "GET", "/d", "1")];
    let r = db
        .operations_apply(&p.id, &source(), &v2, &Selection::AllNew)
        .unwrap();
    assert_eq!(
        (r.added, r.changed, r.removed, r.kept_as_deleted),
        (1, 1, 2, 1)
    );
    assert_eq!(
        keys(&db, &p.id),
        vec![
            ("a".into(), true, false),
            ("b".into(), true, true),
            ("d".into(), true, false)
        ]
    );
    // 入力値は消えない。消えたパラメータは「yaml にない」の印つきで値が残る
    let da = db
        .draft_get(&p.id, &OpKey::Id("a".into()))
        .unwrap()
        .unwrap();
    assert_eq!(da.params.len(), 2);
    assert_eq!(da.params[1].value, "v-page");
    assert_eq!(
        db.draft_stale_params(&p.id, &OpKey::Id("a".into()))
            .unwrap(),
        vec![StaleParam {
            location: ParamIn::Query,
            name: "page".into()
        }]
    );
    assert!(db
        .draft_get(&p.id, &OpKey::Id("b".into()))
        .unwrap()
        .is_some());
    assert_eq!(
        db.operation_get(&p.id, &OpKey::Id("a".into()))
            .unwrap()
            .unwrap()
            .operation
            .summary
            .as_deref(),
        Some("新しい summary")
    );

    // yaml に page が戻ると印は外れる。b も戻ると「削除された API」ではなくなる
    let mut a3 = op("a", "GET", "/a", "3");
    a3.parameters = vec![param("q"), param("page")];
    let v3 = vec![a3, op("b", "GET", "/b", "1"), op("d", "GET", "/d", "1")];
    db.operations_apply(&p.id, &source(), &v3, &Selection::AllNew)
        .unwrap();
    assert!(db
        .draft_stale_params(&p.id, &OpKey::Id("a".into()))
        .unwrap()
        .is_empty());
    assert_eq!(keys(&db, &p.id)[1], ("b".into(), true, false));
}

#[test]
fn dropping_a_stale_param_from_the_draft_clears_its_mark() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    db.operations_apply(
        &p.id,
        &source(),
        &[op("a", "GET", "/a", "1")],
        &Selection::AllNew,
    )
    .unwrap();
    db.draft_save(&draft(&p.id, OpKey::Id("a".into()), &["q", "page"]))
        .unwrap();
    let mut a2 = op("a", "GET", "/a", "2");
    a2.parameters = vec![param("q")];
    db.operations_apply(&p.id, &source(), &[a2], &Selection::AllNew)
        .unwrap();
    assert_eq!(
        db.draft_stale_params(&p.id, &OpKey::Id("a".into()))
            .unwrap()
            .len(),
        1
    );
    // 画面で消して保存すると、印も外れる
    db.draft_save(&draft(&p.id, OpKey::Id("a".into()), &["q"]))
        .unwrap();
    assert!(db
        .draft_stale_params(&p.id, &OpKey::Id("a".into()))
        .unwrap()
        .is_empty());
}

#[test]
fn changed_operation_id_carries_the_draft_over() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    db.operations_apply(
        &p.id,
        &source(),
        &[op("getUser", "GET", "/u", "1")],
        &Selection::AllNew,
    )
    .unwrap();
    let saved = db
        .draft_save(&draft(&p.id, OpKey::Id("getUser".into()), &["q"]))
        .unwrap();
    let r = db
        .operations_apply(
            &p.id,
            &source(),
            &[op("fetchUser", "GET", "/u", "2")],
            &Selection::AllNew,
        )
        .unwrap();
    assert_eq!((r.added, r.changed, r.removed, r.renamed), (0, 1, 0, 1));
    assert_eq!(keys(&db, &p.id), vec![("fetchUser".into(), true, false)]);
    assert!(db
        .draft_get(&p.id, &OpKey::Id("getUser".into()))
        .unwrap()
        .is_none());
    let moved = db
        .draft_get(&p.id, &OpKey::Id("fetchUser".into()))
        .unwrap()
        .unwrap();
    assert_eq!(moved.id, saved.id);
    assert_eq!(moved.params[0].value, "v-q");
}

#[test]
fn removed_api_can_be_forgotten_only_when_it_is_gone_from_yaml() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    db.operations_apply(
        &p.id,
        &source(),
        &[op("a", "GET", "/a", "1"), op("b", "GET", "/b", "1")],
        &Selection::AllNew,
    )
    .unwrap();
    db.draft_save(&draft(&p.id, OpKey::Id("b".into()), &["q"]))
        .unwrap();
    db.operations_apply(
        &p.id,
        &source(),
        &[op("a", "GET", "/a", "1")],
        &Selection::AllNew,
    )
    .unwrap();
    assert!(db
        .draft_keys(&p.id)
        .unwrap()
        .contains(&OpKey::Id("b".into())));
    // まだある API は消せない
    assert!(matches!(
        db.operation_forget(&p.id, &OpKey::Id("a".into())),
        Err(StorageError::Invalid(_))
    ));
    db.operation_forget(&p.id, &OpKey::Id("b".into())).unwrap();
    assert!(db
        .draft_get(&p.id, &OpKey::Id("b".into()))
        .unwrap()
        .is_none());
    assert_eq!(keys(&db, &p.id), vec![("a".into(), true, false)]);
}

#[test]
fn environments_are_created_from_servers_once() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let servers = vec![
        ServerInfo {
            url: "http://localhost:8080".into(),
            description: Some("ローカル (Dev Container)".into()),
        },
        ServerInfo {
            url: "https://api.example.com".into(),
            description: None,
        },
    ];
    let created = db.env_create_from_servers(&p.id, &servers).unwrap();
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].name, "ローカル (Dev Container)");
    assert_eq!(created[1].name, "https://api.example.com");
    let base = &created[0].vars[0];
    assert_eq!(base.name, "baseUrl");
    assert_eq!(
        base.value,
        nekoreq_core::VarValue::Plain("http://localhost:8080".into())
    );
    // 最初に作ったものが選ばれる
    let project = db.project_get(&p.id).unwrap().unwrap();
    assert_eq!(
        project.default_environment_id.as_deref(),
        Some(created[0].id.as_str())
    );
    // 同名の環境がすでにあれば作らない（手で作った同名も）
    assert!(db
        .env_create_from_servers(&p.id, &servers)
        .unwrap()
        .is_empty());
    let manual = db.env_create(&p.id, "検証").unwrap();
    assert_eq!(manual.name, "検証");
    let again = db
        .env_create_from_servers(
            &p.id,
            &[ServerInfo {
                url: "https://stg".into(),
                description: Some("検証".into()),
            }],
        )
        .unwrap();
    assert!(again.is_empty());
    assert_eq!(db.env_list(&p.id).unwrap().len(), 4, "グローバル + 3 つ");
    // 「グローバル」という説明は URL の名前にする
    let g = db
        .env_create_from_servers(
            &p.id,
            &[ServerInfo {
                url: "https://g.example".into(),
                description: Some("グローバル".into()),
            }],
        )
        .unwrap();
    assert_eq!(g[0].name, "https://g.example");
}

#[test]
fn deleting_the_project_removes_source_and_operations() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    db.operations_apply(
        &p.id,
        &source(),
        &[op("a", "GET", "/a", "1")],
        &Selection::AllNew,
    )
    .unwrap();
    db.project_delete(&p.id).unwrap();
    let conn = db.conn();
    for table in ["api_sources", "operations"] {
        let n: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table}");
    }
}

#[test]
fn source_files_can_be_refreshed_without_touching_operations() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    db.operations_apply(
        &p.id,
        &source(),
        &[op("a", "GET", "/a", "1")],
        &Selection::AllNew,
    )
    .unwrap();
    let files = vec![SourceFile {
        path: "openapi.yaml".into(),
        modified_at: Some("2026-02-02T00:00:00.000Z".into()),
        hash: "def".into(),
    }];
    db.source_touch_files(&p.id, &files).unwrap();
    db.source_set_has_token(&p.id, true).unwrap();
    let s = db.project_get(&p.id).unwrap().unwrap().source.unwrap();
    assert_eq!(s.files, files);
    assert!(s.has_token);
    assert_eq!(db.operations_list(&p.id).unwrap().len(), 1);
    assert!(db.source_touch_files("nope", &files).is_err());
}
