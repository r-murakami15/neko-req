//! 同期の反映（operations_apply）の端のケース（M4）。テスト担当が独立に書いた観点。

use std::collections::HashSet;

use nekoreq_core::{
    ApiSource, Body, ImportOptions, KeyValue, OpKey, Operation, ParamIn, ParamSpec, RequestDraft,
    SourceKind, SpecMeta,
};
use nekoreq_storage::{Database, Selection};

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

fn op(id: &str, method: &str, path: &str, hash: &str, params: &[&str]) -> Operation {
    Operation {
        id: String::new(),
        project_id: String::new(),
        op_key: OpKey::Id(id.into()),
        operation_id: Some(id.into()),
        method: method.into(),
        path: path.into(),
        tags: vec!["t".into()],
        summary: None,
        description: None,
        parameters: params.iter().map(|p| param(p)).collect(),
        request_body: None,
        responses: vec![],
        security: None,
        source_file: Some("a.yaml".into()),
        source_line: Some(1),
        content_hash: hash.into(),
    }
}

fn source() -> ApiSource {
    ApiSource {
        id: String::new(),
        kind: SourceKind::Path,
        entry: "/x/openapi.yaml".into(),
        files: vec![],
        openapi_version: "3.0.3".into(),
        synced_at: None,
        has_token: false,
        allowed_hosts: vec![],
        options: ImportOptions::default(),
        meta: SpecMeta::default(),
    }
}

fn draft(project: &str, key: &str, names: &[&str]) -> RequestDraft {
    RequestDraft {
        id: String::new(),
        project_id: project.into(),
        op_key: OpKey::Id(key.into()),
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

fn apply(db: &Database, p: &str, ops: &[Operation]) -> nekoreq_storage::ApplyReport {
    db.operations_apply(p, &source(), ops, &Selection::AllNew)
        .unwrap()
}

#[test]
fn a_removed_api_that_comes_back_is_restored_with_its_draft_and_counts_as_added() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    apply(
        &db,
        &p,
        &[
            op("a", "GET", "/a", "h1", &["q"]),
            op("b", "GET", "/b", "h2", &[]),
        ],
    );
    db.draft_save(&draft(&p, "a", &["q"])).unwrap();
    // yaml から消える
    let r = apply(&db, &p, &[op("b", "GET", "/b", "h2", &[])]);
    assert_eq!((r.removed, r.kept_as_deleted), (1, 1));
    // もう一度同じ反映をしても数え直さない
    let r = apply(&db, &p, &[op("b", "GET", "/b", "h2", &[])]);
    assert_eq!(
        (r.removed, r.kept_as_deleted, r.added, r.changed),
        (0, 0, 0, 0)
    );
    // 戻ってくる
    let r = apply(
        &db,
        &p,
        &[
            op("a", "GET", "/a", "h1", &["q"]),
            op("b", "GET", "/b", "h2", &[]),
        ],
    );
    // 戻ってきたものは、同期の差分（確認ダイアログ）と同じく「追加」と数える
    assert_eq!((r.added, r.changed), (1, 0));
    let list = db.operations_list(&p).unwrap();
    assert!(list.iter().all(|s| s.deleted_at.is_none()));
    assert_eq!(list.len(), 2);
    let d = db.draft_get(&p, &OpKey::Id("a".into())).unwrap().unwrap();
    assert_eq!(d.params[0].value, "v-q");
}

#[test]
fn stale_param_marks_follow_the_yaml_back_and_forth_and_values_stay() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    apply(&db, &p, &[op("a", "GET", "/a", "h1", &["q", "limit"])]);
    db.draft_save(&draft(&p, "a", &["q", "limit"])).unwrap();
    let key = OpKey::Id("a".into());
    // limit が消える
    apply(&db, &p, &[op("a", "GET", "/a", "h2", &["q"])]);
    let stale = db.draft_stale_params(&p, &key).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].name, "limit");
    assert_eq!(
        db.draft_get(&p, &key).unwrap().unwrap().params.len(),
        2,
        "値は残る"
    );
    // もう一度反映しても印は増えない
    apply(&db, &p, &[op("a", "GET", "/a", "h2", &["q"])]);
    assert_eq!(db.draft_stale_params(&p, &key).unwrap().len(), 1);
    // limit が戻る → 印が外れ、値はそのまま
    apply(&db, &p, &[op("a", "GET", "/a", "h3", &["q", "limit"])]);
    assert!(db.draft_stale_params(&p, &key).unwrap().is_empty());
    assert_eq!(
        db.draft_get(&p, &key).unwrap().unwrap().params[1].value,
        "v-limit"
    );
    // 入力値のない API のパラメータが消えても何も起きない（印だけの行は作らない）
    apply(
        &db,
        &p,
        &[
            op("a", "GET", "/a", "h3", &["q", "limit"]),
            op("n", "GET", "/n", "h", &["x"]),
        ],
    );
    apply(
        &db,
        &p,
        &[
            op("a", "GET", "/a", "h3", &["q", "limit"]),
            op("n", "GET", "/n", "h4", &[]),
        ],
    );
    assert!(db.draft_get(&p, &OpKey::Id("n".into())).unwrap().is_none());
}

#[test]
fn swapping_operation_ids_between_two_apis_does_not_violate_uniqueness_or_lose_drafts() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    apply(
        &db,
        &p,
        &[
            op("x", "GET", "/a", "h1", &[]),
            op("y", "GET", "/b", "h2", &[]),
        ],
    );
    db.draft_save(&draft(&p, "x", &["q"])).unwrap();
    db.draft_save(&draft(&p, "y", &["q"])).unwrap();
    apply(
        &db,
        &p,
        &[
            op("y", "GET", "/a", "h1", &[]),
            op("x", "GET", "/b", "h2", &[]),
        ],
    );
    let keys: HashSet<_> = db.draft_keys(&p).unwrap();
    assert_eq!(keys.len(), 2, "入力値が 2 件とも残る: {keys:?}");
    assert_eq!(db.operations_list(&p).unwrap().len(), 2);
}

#[test]
fn method_path_collision_with_a_deleted_row_keeps_one_row_per_key() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    apply(&db, &p, &[op("old", "GET", "/a", "h1", &[])]);
    db.draft_save(&draft(&p, "old", &["q"])).unwrap();
    apply(&db, &p, &[]); // 消える（入力値があるので残る）
                         // 同じ GET /a が別の operationId で戻る → 入力値は引き継ぐ（METHOD path が同じ）
    let r = apply(&db, &p, &[op("new", "GET", "/a", "h1", &[])]);
    assert_eq!((r.added, r.changed), (1, 0)); // 戻ってきた API は「追加」
    let keys = db.draft_keys(&p).unwrap();
    assert_eq!(keys, HashSet::from([OpKey::Id("new".into())]));
    assert_eq!(db.operations_list(&p).unwrap().len(), 1);
}

#[test]
fn unselected_apis_stay_hidden_across_syncs_and_vanish_without_a_trace() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    let sel = Selection::Only(HashSet::from([OpKey::Id("a".into())]));
    db.operations_apply(
        &p,
        &source(),
        &[
            op("a", "GET", "/a", "h", &[]),
            op("b", "GET", "/b", "h", &[]),
        ],
        &sel,
    )
    .unwrap();
    let flags = |db: &Database| {
        db.operations_list(&p)
            .unwrap()
            .into_iter()
            .map(|s| (s.operation.op_key.to_string(), s.imported))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        flags(&db),
        vec![("a".to_string(), true), ("b".to_string(), false)]
    );
    // 同期（AllNew）。b は「追加」ではなく、取り込み済みにもならない。新しい c は取り込み済みになる
    let r = apply(
        &db,
        &p,
        &[
            op("a", "GET", "/a", "h", &[]),
            op("b", "GET", "/b", "h2", &[]),
            op("c", "GET", "/c", "h", &[]),
        ],
    );
    assert_eq!(r.added, 1);
    assert_eq!(
        flags(&db),
        vec![
            ("a".to_string(), true),
            ("b".to_string(), false),
            ("c".to_string(), true)
        ]
    );
    // 取り込んでいない b が yaml から消えたら、行ごと消える（削除された API には出ない）
    let r = apply(
        &db,
        &p,
        &[
            op("a", "GET", "/a", "h", &[]),
            op("c", "GET", "/c", "h", &[]),
        ],
    );
    assert_eq!((r.removed, r.kept_as_deleted), (1, 0));
    assert_eq!(flags(&db).len(), 2);
}

#[test]
fn deleting_a_project_removes_everything_that_hangs_off_it() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    let q = db.project_create("Q").unwrap().id;
    for id in [&p, &q] {
        apply(&db, id, &[op("a", "GET", "/a", "h", &["q"])]);
        db.draft_save(&draft(id, "a", &["q"])).unwrap();
    }
    db.project_delete(&p).unwrap();
    assert!(db.operations_list(&p).unwrap().is_empty());
    assert!(db.draft_keys(&p).unwrap().is_empty());
    assert!(db.project_get(&p).unwrap().is_none());
    // 別のプロジェクトは無傷
    assert_eq!(db.operations_list(&q).unwrap().len(), 1);
    assert_eq!(db.draft_keys(&q).unwrap().len(), 1);
    // 消したあとの反映は NotFound（行を作らない）
    assert!(db
        .operations_apply(
            &p,
            &source(),
            &[op("a", "GET", "/a", "h", &[])],
            &Selection::AllNew
        )
        .is_err());
}

#[test]
fn operation_keys_with_odd_characters_round_trip() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    let mut a = op("with space/and:colon", "GET", "/a b/{id}", "h", &[]);
    let mut b = op("日本語のID", "POST", "/日本語", "h", &[]);
    b.op_key = OpKey::method_path("POST", "/日本語 with space");
    b.operation_id = None;
    a.summary = Some("😀".into());
    apply(&db, &p, &[a, b.clone()]);
    let list = db.operations_list(&p).unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|s| s.operation.op_key == b.op_key));
    db.draft_save(&draft(&p, "with space/and:colon", &["q"]))
        .unwrap();
    assert!(db
        .draft_get(&p, &OpKey::Id("with space/and:colon".into()))
        .unwrap()
        .is_some());
}

fn nested(depth: usize) -> serde_json::Value {
    let mut v = serde_json::json!({"type": "string"});
    for _ in 0..depth {
        v = serde_json::json!({"type": "array", "items": v});
    }
    v
}

#[test]
fn a_deeply_nested_schema_can_be_read_back_and_does_not_brick_the_project() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    let mut deep = op("a", "GET", "/a", "h", &[]);
    let mut pr = param("q");
    pr.schema = Some(nested(150));
    deep.parameters.push(pr);
    apply(&db, &p, &[deep.clone(), op("b", "GET", "/b", "h", &[])]);
    // 読み戻せること。少なくとも、ほかの API の取得まで巻き添えにしないこと
    let list = db.operations_list(&p);
    assert!(list.is_ok(), "{:?}", list.err());
    // yaml を直して再同期できること
    deep.parameters.clear();
    deep.content_hash = "h2".into();
    assert!(db
        .operations_apply(
            &p,
            &source(),
            &[deep, op("b", "GET", "/b", "h", &[])],
            &Selection::AllNew
        )
        .is_ok());
}

#[test]
fn a_very_deep_schema_is_read_back_without_overflowing_the_stack() {
    // 保存できたものは必ず読み戻せる（再帰の上限を外し、スタックは伸ばして読む）
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap().id;
    let mut deep = op("a", "GET", "/a", "h", &[]);
    let mut pr = param("q");
    pr.schema = Some(nested(600));
    deep.parameters.push(pr);
    apply(&db, &p, &[deep.clone()]);
    let list = db.operations_list(&p).unwrap();
    assert!(list[0].operation.parameters == deep.parameters);
}
