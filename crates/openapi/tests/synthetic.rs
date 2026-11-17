//! 合成の fixture（tests/fixtures）での結合テスト。

mod common;

use std::collections::HashSet;

use common::*;
use nekoreq_core::{OpKey, ParamIn};
use nekoreq_openapi::*;
use serde_json::{json, Value};

#[tokio::test]
async fn split_files_resolve_relative_to_the_referencing_file() {
    let dir = fixture("split");
    let l = loaded(&dir.join("openapi.yaml")).await;
    // openapi.yaml / paths/users.yaml / paths/tree.yaml / common/schemas.yaml / common/responses.yaml
    assert_eq!(l.files.len(), 5, "{:?}", l.files);
    assert_eq!(l.reference, ReferenceStyle::Split);
    assert_eq!(l.entry, "openapi.yaml");
    assert_eq!(l.openapi_version, "3.0.3");
    assert_eq!(l.operations.len(), 6);

    let list = op(&l, "listUsers");
    // `#/components/parameters/Page` は paths/users.yaml 自身の中を指す
    let names: Vec<_> = list
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location))
        .collect();
    assert_eq!(names, vec![("q", ParamIn::Query), ("page", ParamIn::Query)]);
    assert_eq!(list.parameters[0].example, Some(json!("foo")));
    assert_eq!(list.parameters[1].default, Some(json!(1)));
    // 404 は ../common/responses.yaml の中の ./schemas.yaml（common/ 基準）まで届く
    let r404 = list.responses.iter().find(|r| r.status == "404").unwrap();
    let schema = r404.content[0].schema.as_ref().unwrap();
    assert_eq!(schema["x-nekoreq-ref"], "Problem");
    assert_eq!(schema["properties"]["code"]["example"], "NOT_FOUND");
    // 200 は同じファイルの #/components/schemas/UserList → ../common/schemas.yaml の User（allOf）
    let ok = list.responses.iter().find(|r| r.status == "200").unwrap();
    let s = ok.content[0].schema.as_ref().unwrap();
    assert_eq!(s["x-nekoreq-ref"], "UserList");
    assert_eq!(s["properties"]["items"]["items"]["x-nekoreq-ref"], "User");
    assert_eq!(
        s["properties"]["items"]["items"]["allOf"][0]["x-nekoreq-ref"],
        "Base"
    );
}

#[tokio::test]
async fn path_level_parameters_are_merged_and_operation_wins() {
    let l = loaded(&fixture("split/openapi.yaml")).await;
    let get = op(&l, "getUser");
    assert_eq!(get.parameters.len(), 1);
    let p = &get.parameters[0];
    assert_eq!(p.name, "userId");
    assert!(p.required);
    assert_eq!(p.description.as_deref(), Some("オペレーション側の説明"));
    assert_eq!(p.schema.as_ref().unwrap()["minimum"], 1);
}

#[tokio::test]
async fn operations_without_operation_id_use_method_and_path() {
    let l = loaded(&fixture("split/openapi.yaml")).await;
    let del = op(&l, "DELETE /users/{userId}");
    assert_eq!(del.op_key, OpKey::method_path("DELETE", "/users/{userId}"));
    assert_eq!(del.operation_id, None);
    // path レベルの userId を引き継ぐ
    assert_eq!(del.parameters.len(), 1);
    assert_eq!(
        del.security.as_ref().unwrap()[0].schemes[0].name,
        "bearerAuth"
    );
    let health = op(&l, "GET /health");
    assert_eq!(health.security, Some(vec![]));
    assert!(health.tags.is_empty());
    assert_eq!(l.inspection.no_security_count, 1);
}

#[tokio::test]
async fn source_file_and_line_point_to_where_the_operation_is_written() {
    let dir = fixture("split");
    let l = loaded(&dir.join("openapi.yaml")).await;
    for o in &l.operations {
        let file = dir.join(o.source_file.as_ref().unwrap());
        let line = o.source_line.unwrap();
        let text = line_text(&file, line);
        assert_eq!(
            text.trim(),
            format!("{}:", o.method.to_lowercase()),
            "{} {}:{line}",
            o.op_key,
            file.display()
        );
    }
    assert_eq!(
        op(&l, "listUsers").source_file.as_deref(),
        Some("paths/users.yaml")
    );
    assert_eq!(
        op(&l, "GET /health").source_file.as_deref(),
        Some("openapi.yaml")
    );
}

#[tokio::test]
async fn split_and_bundled_give_the_same_operations() {
    let a = loaded(&fixture("split/openapi.yaml")).await;
    let b = loaded(&fixture("bundled/openapi.yaml")).await;
    assert_eq!(b.files.len(), 1);
    assert_eq!(b.reference, ReferenceStyle::Bundled);
    assert_eq!(a.operations.len(), b.operations.len());
    for oa in &a.operations {
        let ob = op(&b, &oa.op_key.to_string());
        assert_eq!(oa.method, ob.method);
        assert_eq!(oa.path, ob.path);
        assert_eq!(oa.tags, ob.tags);
        assert_eq!(oa.summary, ob.summary);
        assert_eq!(oa.description, ob.description);
        assert_eq!(oa.parameters, ob.parameters, "{}", oa.op_key);
        assert_eq!(oa.request_body, ob.request_body, "{}", oa.op_key);
        assert_eq!(oa.responses, ob.responses, "{}", oa.op_key);
        assert_eq!(oa.security, ob.security);
        assert_eq!(
            oa.content_hash, ob.content_hash,
            "ハッシュは書かれた場所によらない"
        );
    }
    assert_eq!(a.meta, b.meta);
}

#[tokio::test]
async fn meta_has_servers_security_and_tags() {
    let l = loaded(&fixture("split/openapi.yaml")).await;
    assert_eq!(l.meta.title, "Sample API");
    assert_eq!(l.meta.version, "1.2.3");
    // servers の変数は既定値で置き換える
    let urls: Vec<_> = l.meta.servers.iter().map(|s| s.url.as_str()).collect();
    assert_eq!(
        urls,
        vec!["http://localhost:8080", "https://api.example.com"]
    );
    assert_eq!(l.meta.servers[0].description.as_deref(), Some("ローカル"));
    assert_eq!(l.meta.root_security[0].schemes[0].name, "cookieAuth");
    let cookie = l
        .meta
        .security_schemes
        .iter()
        .find(|s| s.name == "cookieAuth")
        .unwrap();
    assert_eq!(cookie.key_name.as_deref(), Some("SESSION"));
    assert_eq!(cookie.label(), "cookieAuth（Cookie: SESSION）");
    let preview = build_preview(&l, None);
    assert_eq!(preview.default_auth, vec!["cookieAuth（Cookie: SESSION）"]);
    assert_eq!(
        preview
            .tags
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>(),
        vec![Some("users".into()), Some("tree".into()), None]
    );
    assert_eq!(preview.operation_count, 6);
    assert_eq!(preview.inspection.markdown_table_count, 1);
}

#[tokio::test]
async fn circular_references_stop_on_the_second_visit() {
    let l = loaded(&fixture("split/openapi.yaml")).await;
    let tree = op(&l, "getTree");
    let schema = tree.responses[0].content[0].schema.as_ref().unwrap();
    assert_eq!(schema["x-nekoreq-ref"], "Node");
    assert_eq!(schema["properties"]["child"]["x-nekoreq-circular"], true);
    assert_eq!(
        schema["properties"]["children"]["items"]["x-nekoreq-circular"],
        true
    );

    let detail = describe_operation(tree, &l.meta, &Default::default(), None, false);
    let media = &detail.responses[0].contents[0];
    let rows: Vec<_> = media
        .rows
        .iter()
        .map(|r| (r.depth, r.name.as_str(), r.type_label.as_str(), r.circular))
        .collect();
    assert_eq!(
        rows,
        vec![
            (0, "name", "string", false),
            (0, "child", "Node", true),
            (0, "children", "Node[]", false),
            (1, "[]", "Node", true),
        ]
    );
    // モックも止まる
    assert!(media.example.is_some());
}

#[tokio::test]
async fn schema_table_merges_all_of_and_shows_constraints() {
    let l = loaded(&fixture("split/openapi.yaml")).await;
    let get = op(&l, "getUser");
    let detail = describe_operation(get, &l.meta, &Default::default(), None, false);
    let ok = detail.responses.iter().find(|r| r.status == "200").unwrap();
    let m = &ok.contents[0];
    assert_eq!(m.schema_label.as_deref(), Some("User"));
    let by_name = |n: &str| m.rows.iter().find(|r| r.name == n).unwrap();
    // allOf（Base + 本体）がまとまって、id・name・email・status が並ぶ
    let names: Vec<_> = m.rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["id", "name", "email", "status"]);
    assert!(by_name("id").required && by_name("name").required);
    assert_eq!(by_name("id").constraints, vec!["readOnly"]);
    assert_eq!(
        by_name("name").constraints,
        vec!["minLength: 1", "maxLength: 50"]
    );
    assert_eq!(
        by_name("email").constraints,
        vec!["nullable", "format: email"]
    );
    assert_eq!(by_name("status").type_label, "Status");
    assert_eq!(
        by_name("status").constraints,
        vec!["enum: ACTIVE, INACTIVE"]
    );
    // 例: スキーマの example から作る
    assert_eq!(m.example_source, Some(ExampleSource::SchemaExample));
    let ex = m.example.as_ref().unwrap();
    assert_eq!(ex["name"], "太郎");
    assert_eq!(ex["status"], "ACTIVE");
    // 404 は「返すエラー」の表から code を出す
    let list = describe_operation(
        op(&l, "listUsers"),
        &l.meta,
        &Default::default(),
        None,
        false,
    );
    let r404 = list.responses.iter().find(|r| r.status == "404").unwrap();
    assert_eq!(r404.error_codes.len(), 1);
    assert_eq!(r404.error_codes[0].code, "NOT_FOUND");
}

#[tokio::test]
async fn missing_files_stop_the_import_with_a_list_of_referencing_places() {
    let dir = fixture("missing");
    let problems = blocked(&dir.join("openapi.yaml")).await;
    assert_eq!(problems.len(), 2, "{problems:#?}");
    let find_ref_lines = |needle: &str| -> Vec<u32> {
        std::fs::read_to_string(dir.join("openapi.yaml"))
            .unwrap()
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(needle))
            .map(|(i, _)| i as u32 + 1)
            .collect()
    };
    let mut seen = HashSet::new();
    for p in &problems {
        let LoadProblem::MissingFile {
            file,
            referenced_from,
            ..
        } = p
        else {
            panic!("MissingFile のはず: {p:?}")
        };
        seen.insert(file.clone());
        match file.as_str() {
            "paths/gone.yaml" => {
                let lines: Vec<_> = referenced_from
                    .iter()
                    .map(|s| (s.file.as_str(), s.line.unwrap()))
                    .collect();
                let expect: Vec<_> = find_ref_lines("gone.yaml")
                    .into_iter()
                    .map(|l| ("openapi.yaml", l))
                    .collect();
                assert_eq!(expect.len(), 2);
                assert_eq!(lines, expect);
            }
            "nothere/schema.yaml" => {
                assert_eq!(referenced_from.len(), 1);
                assert_eq!(referenced_from[0].line, Some(find_ref_lines("nothere")[0]));
            }
            other => panic!("想定外: {other}"),
        }
        assert!(p.describe().contains(file));
    }
    assert_eq!(seen.len(), 2);
}

#[tokio::test]
async fn broken_yaml_reports_file_line_and_reason() {
    let problems = blocked(&fixture("broken/openapi.yaml")).await;
    assert_eq!(problems.len(), 1, "{problems:#?}");
    let LoadProblem::YamlError {
        file, line, reason, ..
    } = &problems[0]
    else {
        panic!("{:?}", problems[0])
    };
    assert_eq!(file, "part.yaml");
    let line = line.expect("行が分かる");
    assert!((4..=6).contains(&line), "line = {line}");
    assert!(!reason.is_empty());
}

#[tokio::test]
async fn openapi_3_1_and_3_2_are_read() {
    let l = loaded(&fixture("v31/openapi.yaml")).await;
    assert_eq!(l.openapi_version, "3.1.0");
    let o = op(&l, "listThings");
    let detail = describe_operation(o, &l.meta, &Default::default(), None, false);
    let m = &detail.responses[0].contents[0];
    let name = m.rows.iter().find(|r| r.name == "name").unwrap();
    assert!(name.nullable, "type: [string, null] は nullable");
    assert_eq!(name.base_type, "string");
    assert!(name.required);
    // 3.1 の examples（配列）は例として使われる
    assert_eq!(m.example.as_ref().unwrap()["tags"], json!(["a"]));

    let l = loaded(&fixture("v32/openapi.yaml")).await;
    assert_eq!(l.openapi_version, "3.2.0");
    let methods: Vec<_> = l.operations.iter().map(|o| o.method.as_str()).collect();
    assert_eq!(methods, vec!["QUERY", "COPY"]);
    assert!(op(&l, "searchThings").request_body.is_some());
}

#[tokio::test]
async fn unsupported_or_non_openapi_entries_are_reported() {
    let p = blocked(&fixture("swagger2/openapi.yaml")).await;
    assert!(
        matches!(&p[0], LoadProblem::UnsupportedVersion { version, .. } if version.contains("2.0")),
        "{p:?}"
    );
    let p = blocked(&fixture("notopenapi/redocly.yaml")).await;
    assert!(matches!(&p[0], LoadProblem::NotOpenApi { .. }), "{p:?}");
    let p = blocked(&fixture("nothing/openapi.yaml")).await;
    assert!(matches!(&p[0], LoadProblem::Unreadable { .. }), "{p:?}");
}

#[tokio::test]
async fn duplicate_operation_ids_fall_back_to_method_and_path_keys() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("openapi.yaml");
    std::fs::write(
        &f,
        "openapi: 3.0.3\ninfo: {title: D, version: '1'}\npaths:\n  /a:\n    get:\n      operationId: same\n      responses: {'200': {description: ok}}\n  /b:\n    get:\n      operationId: same\n      responses: {'200': {description: ok}}\n  /c:\n    get:\n      operationId: unique\n      responses: {'200': {description: ok}}\n",
    )
    .unwrap();
    let l = loaded(&f).await;
    assert_eq!(l.inspection.duplicate_operation_ids.len(), 1);
    assert_eq!(l.inspection.duplicate_operation_ids[0].operation_id, "same");
    assert_eq!(
        l.inspection.duplicate_operation_ids[0].operations,
        vec!["GET /a", "GET /b"]
    );
    let keys: Vec<_> = l.operations.iter().map(|o| o.op_key.to_string()).collect();
    assert_eq!(keys, vec!["GET /a", "GET /b", "unique"]);
}

#[tokio::test]
async fn unresolved_pointers_are_reported_but_do_not_stop() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("openapi.yaml");
    std::fs::write(
        &f,
        "openapi: 3.0.3\ninfo: {title: U, version: '1'}\npaths:\n  /a:\n    get:\n      operationId: a\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                $ref: '#/components/schemas/Nope'\n",
    )
    .unwrap();
    let l = loaded(&f).await;
    assert_eq!(l.operations.len(), 1);
    assert_eq!(l.inspection.unresolved_refs.len(), 1);
    let u = &l.inspection.unresolved_refs[0];
    assert_eq!(u.reference, "#/components/schemas/Nope");
    assert_eq!(u.from.file, "openapi.yaml");
    assert_eq!(
        line_text(&f, u.from.line.unwrap()).trim(),
        "$ref: '#/components/schemas/Nope'"
    );
    let schema = l.operations[0].responses[0].content[0]
        .schema
        .as_ref()
        .unwrap();
    assert!(schema.get("x-nekoreq-unresolved").is_some());
}

#[tokio::test]
async fn staged_upload_of_a_folder_reads_like_the_original() {
    let dest = tempfile::tempdir().unwrap();
    let files = upload::stage(&[fixture("split")], dest.path()).unwrap();
    assert_eq!(files.len(), 5);
    assert_eq!(
        upload::entry_candidates(dest.path(), &files),
        vec!["openapi.yaml"]
    );
    let l = loaded(&dest.path().join("openapi.yaml")).await;
    let orig = loaded(&fixture("split/openapi.yaml")).await;
    assert_eq!(l.operations.len(), orig.operations.len());
    // 複数の入口候補
    let dest2 = tempfile::tempdir().unwrap();
    let files = upload::stage(&[fixture("multi_entry")], dest2.path()).unwrap();
    assert_eq!(
        upload::entry_candidates(dest2.path(), &files),
        vec!["openapi.bundled.yaml", "openapi.yaml"]
    );
}

#[tokio::test]
async fn description_change_shows_up_as_changed_and_other_changes_are_described() {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&fixture("split"), tmp.path());
    let before = loaded(&tmp.path().join("openapi.yaml")).await;

    // description の書き換え
    let users = tmp.path().join("paths/users.yaml");
    let text = std::fs::read_to_string(&users).unwrap();
    std::fs::write(
        &users,
        text.replace("ユーザーを返す。", "ユーザーを一覧で返す。"),
    )
    .unwrap();
    let after = loaded(&tmp.path().join("openapi.yaml")).await;
    let none = HashSet::new();
    let d = diff_operations(&before.operations, &after.operations, &none);
    assert!(d.added.is_empty() && d.removed.is_empty());
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].entry.op_key, OpKey::Id("listUsers".into()));
    assert_eq!(d.changed[0].changes.len(), 1);
    assert_eq!(d.changed[0].changes[0].field, ChangedField::Description);
    assert!(d.changed[0].changes[0]
        .after
        .as_deref()
        .unwrap()
        .contains("一覧で"));

    // 変更なし
    let same = diff_operations(&after.operations, &after.operations, &none);
    assert!(same.is_empty());

    // パラメータ追加・operationId の付け替え・API の削除
    let text = std::fs::read_to_string(&users).unwrap();
    let text = text
        .replace("operationId: getUser", "operationId: fetchUser")
        .replace("        - $ref: \"#/components/parameters/Page\"\n", "        - $ref: \"#/components/parameters/Page\"\n        - name: perPage\n          in: query\n          schema:\n            type: integer\n");
    std::fs::write(&users, text).unwrap();
    let tree = tmp.path().join("paths/tree.yaml");
    std::fs::write(&tree, "paths: {}\n").unwrap();
    let main = tmp.path().join("openapi.yaml");
    let latest = loaded(&main).await;
    let drafts: HashSet<OpKey> = [OpKey::Id("getUser".into()), OpKey::Id("getTree".into())].into();
    let d = diff_operations(&after.operations, &latest.operations, &drafts);
    // getTree は paths/tree.yaml から消えた（PathItem の $ref の先が空）ので、解決できず消える
    assert_eq!(d.removed.len(), 1, "{d:#?}");
    assert_eq!(d.removed[0].op_key, OpKey::Id("getTree".into()));
    assert!(d.removed[0].has_draft);
    let renamed = d.changed.iter().find(|c| c.renamed_from.is_some()).unwrap();
    assert_eq!(renamed.renamed_from, Some(OpKey::Id("getUser".into())));
    assert_eq!(renamed.entry.op_key, OpKey::Id("fetchUser".into()));
    assert!(renamed.entry.has_draft);
    assert!(renamed
        .changes
        .iter()
        .any(|c| c.field == ChangedField::OperationId));
    assert!(d
        .changed
        .iter()
        .any(|c| c.entry.op_key == OpKey::Id("listUsers".into())
            && c.changes
                .iter()
                .any(|ch| ch.field == ChangedField::Parameters && ch.note.contains("perPage"))));
}

#[tokio::test]
async fn source_changed_compares_modification_times_only() {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&fixture("split"), tmp.path());
    let main = tmp.path().join("openapi.yaml");
    let l = loaded(&main).await;
    let r = source_changed(&main, &l.files);
    assert!(!r.changed, "{r:?}");
    // 参照先のファイルの更新日時を変える
    std::thread::sleep(std::time::Duration::from_millis(30));
    let f = tmp.path().join("common/schemas.yaml");
    let text = std::fs::read_to_string(&f).unwrap();
    std::fs::write(&f, text).unwrap();
    let r = source_changed(&main, &l.files);
    assert!(r.changed);
    assert_eq!(r.changed_files, vec!["common/schemas.yaml"]);
    // 消えたら変わったとみなす
    std::fs::remove_file(&f).unwrap();
    assert!(source_changed(&main, &l.files).changed);
}

#[tokio::test]
async fn initial_request_fills_disabled_params_and_json_body() {
    let l = loaded(&fixture("split/openapi.yaml")).await;
    let opts = nekoreq_core::ImportOptions::default();
    let list = op(&l, "listUsers");
    let open = open_operation(list, &l.meta, &opts, None, &[], false);
    assert_eq!(open.request.url, "{{baseUrl}}/users");
    assert_eq!(open.request.method, "GET");
    assert!(!open.has_draft);
    let params: Vec<_> = open
        .request
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.value.as_str(), p.enabled))
        .collect();
    assert_eq!(params, vec![("q", "foo", false), ("page", "1", false)]);
    assert!(matches!(open.request.body, nekoreq_core::Body::None));
    // 既定の認証（cookieAuth → 環境変数 session）
    assert!(
        matches!(&open.default_auth, Some(DefaultAuth::ApiKey { variable, name, .. }) if variable == "session" && name == "SESSION")
    );

    let create = op(&l, "createUser");
    let open = open_operation(create, &l.meta, &opts, None, &[], false);
    let nekoreq_core::Body::Json(text) = &open.request.body else {
        panic!("{:?}", open.request.body)
    };
    let v: Value = serde_json::from_str(text).unwrap();
    assert_eq!(v, json!({"name": "string", "status": "ACTIVE"}));
    assert_eq!(open.body_content_type.as_deref(), Some("application/json"));
    // example から埋めないオプション
    let off = nekoreq_core::ImportOptions {
        fill_from_examples: false,
        ..opts
    };
    let open = open_operation(create, &l.meta, &off, None, &[], false);
    assert_eq!(open.request.body, nekoreq_core::Body::Json(String::new()));
    let open = open_operation(list, &l.meta, &off, None, &[], false);
    assert!(open
        .request
        .params
        .iter()
        .all(|p| p.value.is_empty() && !p.enabled));

    // security: [] は認証なし、bearer は token 変数
    let health = op(&l, "GET /health");
    assert!(default_auth(health, &l.meta, &opts).is_none());
    assert!(!security_doc_required(health, &l));
    let del = op(&l, "DELETE /users/{userId}");
    assert!(
        matches!(default_auth(del, &l.meta, &opts), Some(DefaultAuth::Bearer { variable, .. }) if variable == "token")
    );
}

fn security_doc_required(o: &nekoreq_core::Operation, l: &Loaded) -> bool {
    view::security_doc(o, &l.meta).required
}
