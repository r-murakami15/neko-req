//! $ref の解決・変換・行番号・差分・性能（M3・M4）。テスト担当が独立に書いた観点（既存の synthetic.rs と重ならないもの）。

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{blocked, load_path, loaded, op};
use nekoreq_core::{ImportOptions, OpKey, ParamIn};
use nekoreq_openapi::schema::{constraints, mock, schema_table};
use nekoreq_openapi::*;
use serde_json::json;

/// 木を作って入口（`openapi.yaml`）のパスを返す。
fn tree(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = dir.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    let entry = dir.path().join("openapi.yaml");
    (dir, entry)
}

const HEAD: &str = "openapi: 3.0.3\ninfo: {title: T, version: '1'}\n";

fn spec(body: &str) -> (tempfile::TempDir, PathBuf) {
    tree(&[("openapi.yaml", &format!("{HEAD}{body}"))])
}

fn ok_get(id: &str) -> String {
    format!("get:\n  operationId: {id}\n  responses:\n    '200': {{description: ok}}\n")
}

// ================= $ref の解決 =================

#[tokio::test]
async fn different_spellings_of_the_same_file_are_read_once() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}paths:\n  /1: {{$ref: './a.yaml'}}\n  /2: {{$ref: 'a.yaml'}}\n  /3: {{$ref: 'sub/../a.yaml'}}\n  /4: {{$ref: './sub/./../a.yaml#'}}\n  /5: {{$ref: 'sub/b.yaml'}}\n"
            ),
        ),
        ("a.yaml", &ok_get("getA")),
        ("sub/b.yaml", "get:\n  operationId: getB\n  parameters:\n    - $ref: '../a.yaml#/x-param'\n  responses:\n    '200': {description: ok}\n"),
    ]);
    // sub/b.yaml が a.yaml を `../a.yaml` で参照するのも同じファイル
    fs::write(
        _d.path().join("a.yaml"),
        format!(
            "{}x-param: {{name: q, in: query, schema: {{type: string}}}}\n",
            ok_get("getA")
        ),
    )
    .unwrap();
    let l = loaded(&e).await;
    let names: Vec<_> = l.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        names,
        vec!["openapi.yaml", "a.yaml", "sub/b.yaml"],
        "{names:?}"
    );
    assert_eq!(l.operations.len(), 5);
}

#[tokio::test]
async fn refs_without_a_file_name_point_at_the_file_they_are_written_in() {
    // 入口にも part.yaml にも components.schemas.Thing がある。part.yaml の `#/...` は part.yaml 自身
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}paths:\n  /own:\n    get:\n      operationId: own\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {{$ref: '#/components/schemas/Thing'}}\n  /part: {{$ref: './part.yaml#/paths/~1part'}}\ncomponents:\n  schemas:\n    Thing: {{type: string}}\n"
            ),
        ),
        (
            "part.yaml",
            "paths:\n  /part:\n    get:\n      operationId: part\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {$ref: '#/components/schemas/Thing'}\ncomponents:\n  schemas:\n    Thing: {type: integer}\n",
        ),
    ]);
    let l = loaded(&e).await;
    let ty = |k: &str| op(&l, k).responses[0].content[0].schema.as_ref().unwrap()["type"].clone();
    assert_eq!(ty("own"), json!("string"));
    assert_eq!(ty("part"), json!("integer"));
    assert_eq!(op(&l, "part").source_file.as_deref(), Some("part.yaml"));
}

#[tokio::test]
async fn json_pointer_escapes_and_percent_encoding_are_decoded() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}paths:\n  /users/{{id}}: {{$ref: './p.yaml#/paths/~1users~1%7Bid%7D'}}\n  /tilde: {{$ref: './p.yaml#/paths/~1a~0b'}}\n  /sp:\n    get:\n      operationId: sp\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {{$ref: './p.yaml#/components/schemas/Name%20With%20Space'}}\n"
            ),
        ),
        (
            "p.yaml",
            "paths:\n  /users/{id}:\n    get:\n      operationId: byId\n      responses:\n        '200': {description: ok}\n  /a~b:\n    get:\n      operationId: tilde\n      responses:\n        '200': {description: ok}\ncomponents:\n  schemas:\n    Name With Space: {type: boolean}\n",
        ),
    ]);
    let l = loaded(&e).await;
    assert!(
        l.inspection.unresolved_refs.is_empty(),
        "{:#?}",
        l.inspection.unresolved_refs
    );
    assert_eq!(l.operations.len(), 3);
    assert_eq!(
        op(&l, "sp").responses[0].content[0]
            .schema
            .as_ref()
            .unwrap()["x-nekoreq-ref"],
        json!("Name With Space")
    );
}

#[tokio::test]
async fn json_files_can_be_referenced_and_path_items_can_chain() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}paths:\n  /x: {{$ref: './chain1.yaml'}}\n  /j:\n    get:\n      operationId: j\n      parameters:\n        - $ref: './defs.json#/parameters/Limit'\n      responses:\n        '200': {{description: ok}}\n"
            ),
        ),
        ("chain1.yaml", "$ref: './chain2.yaml#'\n"),
        ("chain2.yaml", &ok_get("viaChain")),
        (
            "defs.json",
            "{\n  \"parameters\": {\n    \"Limit\": {\"name\": \"limit\", \"in\": \"query\", \"schema\": {\"type\": \"integer\", \"maximum\": 100}}\n  }\n}\n",
        ),
    ]);
    let l = loaded(&e).await;
    assert_eq!(
        op(&l, "viaChain").source_file.as_deref(),
        Some("chain2.yaml")
    );
    let p = &op(&l, "j").parameters;
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].name, "limit");
    assert_eq!(p[0].schema.as_ref().unwrap()["maximum"], json!(100));
    assert_eq!(l.files.len(), 4);
}

#[tokio::test]
async fn parameters_responses_request_bodies_and_schemas_resolve_relative_to_the_writing_file() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!("{HEAD}paths:\n  /a: {{$ref: './api/a.yaml#/path'}}\n"),
        ),
        (
            "api/a.yaml",
            "path:\n  post:\n    operationId: a\n    parameters:\n      - $ref: '../common/params.yaml#/Limit'\n    requestBody: {$ref: '../common/bodies.yaml#/Create'}\n    responses:\n      '200': {$ref: '../common/responses.yaml#/Ok'}\n      '404': {$ref: '../common/responses.yaml#/NotFound'}\n",
        ),
        ("common/params.yaml", "Limit: {name: limit, in: query, schema: {$ref: './types.yaml#/Int'}}\n"),
        ("common/types.yaml", "Int: {type: integer, minimum: 1}\n"),
        (
            "common/bodies.yaml",
            "Create:\n  required: true\n  content:\n    application/json:\n      schema: {$ref: './types.yaml#/Obj'}\n",
        ),
        (
            "common/responses.yaml",
            "Ok:\n  description: ok\n  content:\n    application/json:\n      schema: {$ref: './types.yaml#/Obj'}\nNotFound: {description: nf}\n",
        ),
    ]);
    fs::write(
        _d.path().join("common/types.yaml"),
        "Int: {type: integer, minimum: 1}\nObj: {type: object, properties: {n: {$ref: '#/Int'}}}\n",
    )
    .unwrap();
    let l = loaded(&e).await;
    let o = op(&l, "a");
    assert_eq!(
        o.parameters[0].schema.as_ref().unwrap()["x-nekoreq-ref"],
        json!("Int")
    );
    assert!(o.request_body.as_ref().unwrap().required);
    assert_eq!(
        o.request_body.as_ref().unwrap().content[0]
            .schema
            .as_ref()
            .unwrap()["properties"]["n"]["minimum"],
        json!(1)
    );
    assert_eq!(o.responses.len(), 2);
    assert!(l.inspection.unresolved_refs.is_empty());
}

#[tokio::test]
async fn cycles_across_files_stop_and_are_marked() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}paths:\n  /n:\n    get:\n      operationId: n\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {{$ref: './a.yaml#/A'}}\n"
            ),
        ),
        ("a.yaml", "A:\n  type: object\n  properties:\n    b: {$ref: './b.yaml#/B'}\n"),
        ("b.yaml", "B:\n  type: object\n  properties:\n    a: {$ref: './a.yaml#/A'}\n"),
    ]);
    let l = loaded(&e).await;
    let s = op(&l, "n").responses[0].content[0].schema.clone().unwrap();
    assert_eq!(
        s["properties"]["b"]["properties"]["a"]["x-nekoreq-circular"],
        json!(true)
    );
    let t = schema_table(&s);
    assert!(t.rows.iter().any(|r| r.circular), "{:#?}", t.rows);
    assert_eq!(l.files.len(), 3);
}

#[tokio::test]
async fn ref_chains_that_loop_without_a_schema_are_reported_not_hung() {
    // A -> B -> A（どちらも $ref だけ）。解決できない $ref として記録され、止まらずに終わる
    let (_d, e) = spec(
        "paths:\n  /n:\n    get:\n      operationId: n\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {$ref: '#/components/schemas/A'}\ncomponents:\n  schemas:\n    A: {$ref: '#/components/schemas/B'}\n    B: {$ref: '#/components/schemas/A'}\n    Self: {$ref: '#/components/schemas/Self'}\n",
    );
    let l = loaded(&e).await;
    assert!(!l.inspection.unresolved_refs.is_empty());
    assert!(
        l.inspection.unresolved_refs[0].reason.contains("循環"),
        "{:#?}",
        l.inspection.unresolved_refs
    );
}

#[tokio::test]
async fn missing_files_list_the_displayed_name_and_the_line_of_each_referencing_ref() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}# コメント\npaths:\n  /a: {{$ref: './sub/a.yaml#/p'}}\n  /b:\n    $ref: './gone.yaml#/p'\n"
            ),
        ),
        (
            "sub/a.yaml",
            "# 日本語のコメント\np:\n  get:\n    operationId: a\n    parameters:\n      - $ref: '../gone.yaml#/x'\n      - { $ref: '../gone2.yaml#/x' }\n    responses:\n      '200': {description: ok}\n",
        ),
    ]);
    let problems = blocked(&e).await;
    let missing: Vec<_> = problems
        .iter()
        .filter_map(|p| match p {
            LoadProblem::MissingFile {
                file,
                referenced_from,
                ..
            } => Some((file.clone(), referenced_from.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(missing.len(), 2, "{problems:#?}");
    let gone = missing
        .iter()
        .find(|(f, _)| f == "gone.yaml")
        .expect("gone.yaml");
    // gone.yaml: 入口の 7 行目（`    $ref:`）と sub/a.yaml の 6 行目
    let mut sites: Vec<(String, Option<u32>)> =
        gone.1.iter().map(|s| (s.file.clone(), s.line)).collect();
    sites.sort();
    assert_eq!(
        sites,
        vec![
            ("openapi.yaml".to_string(), Some(7)),
            ("sub/a.yaml".to_string(), Some(6))
        ]
    );
    let gone2 = missing
        .iter()
        .find(|(f, _)| f == "gone2.yaml")
        .expect("gone2.yaml");
    assert_eq!(gone2.1[0].line, Some(7), "フロー形式 {{ $ref: ... }} の行");
}

#[tokio::test]
async fn refs_to_a_directory_or_an_empty_file_are_reported_without_panicking() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}paths:\n  /a: {{$ref: './dir#/p'}}\n  /b: {{$ref: './empty.yaml#/p'}}\n"
            ),
        ),
        ("dir/keep.yaml", "a: 1\n"),
        ("empty.yaml", ""),
    ]);
    let problems = blocked(&e).await;
    assert!(problems.len() >= 2, "{problems:#?}");
}

#[tokio::test]
async fn a_ref_to_the_whole_document_or_with_an_empty_fragment_works() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!("{HEAD}paths:\n  /a: {{$ref: './pi.yaml#'}}\n  /b: {{$ref: './pi.yaml'}}\n"),
        ),
        ("pi.yaml", &ok_get("same")),
    ]);
    let l = loaded(&e).await;
    // 同じ PathItem を 2 つの path で使う。operationId は重複するので METHOD path のキーになる
    assert_eq!(l.operations.len(), 2);
    assert_eq!(l.inspection.duplicate_operation_ids.len(), 1);
    assert!(l
        .operations
        .iter()
        .all(|o| matches!(o.op_key, OpKey::MethodPath { .. })));
}

#[tokio::test]
async fn unreferenced_files_in_the_folder_are_not_read() {
    let (_d, e) = tree(&[
        ("openapi.yaml", &format!("{HEAD}paths: {{}}\n")),
        ("broken.yaml", "a: [1, 2\n"),
    ]);
    let l = loaded(&e).await;
    assert_eq!(l.files.len(), 1);
}

// ================= 変換 =================

#[tokio::test]
async fn path_level_and_operation_level_parameters_merge_with_the_operation_winning_in_place() {
    let (_d, e) = spec(
        "paths:\n  /u/{id}:\n    parameters:\n      - {name: id, in: path, required: true, schema: {type: string}}\n      - {name: q, in: query, description: from-path, schema: {type: string}}\n      - {name: X-Trace, in: header, schema: {type: string}}\n    get:\n      operationId: g\n      parameters:\n        - {name: q, in: query, description: from-op, required: true, schema: {type: integer}}\n        - {name: q, in: header, schema: {type: string}}\n        - {name: extra, in: cookie, schema: {type: string}}\n      responses:\n        '200': {description: ok}\n    delete:\n      operationId: d\n      responses:\n        '204': {description: ok}\n",
    );
    let l = loaded(&e).await;
    let g = op(&l, "g");
    let names: Vec<_> = g
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location))
        .collect();
    assert_eq!(
        names,
        vec![
            ("id", ParamIn::Path),
            ("q", ParamIn::Query),
            ("X-Trace", ParamIn::Header),
            ("q", ParamIn::Header),
            ("extra", ParamIn::Cookie)
        ]
    );
    let q = &g.parameters[1];
    assert_eq!(q.description.as_deref(), Some("from-op"));
    assert!(q.required);
    assert_eq!(q.schema.as_ref().unwrap()["type"], json!("integer"));
    // delete は path レベルのパラメータだけ
    assert_eq!(op(&l, "d").parameters.len(), 3);
    assert_eq!(
        op(&l, "d").parameters[1].description.as_deref(),
        Some("from-path")
    );
}

#[tokio::test]
async fn path_parameters_are_always_required_and_content_style_parameters_get_a_schema() {
    let (_d, e) = spec(
        "paths:\n  /a/{id}:\n    get:\n      operationId: g\n      parameters:\n        - {name: id, in: path, schema: {type: string}}\n        - name: filter\n          in: query\n          content:\n            application/json:\n              schema: {type: object, properties: {a: {type: string}}}\n        - {name: bad, in: nowhere}\n        - {in: query}\n      responses:\n        '200': {description: ok}\n",
    );
    let l = loaded(&e).await;
    let g = op(&l, "g");
    assert_eq!(g.parameters.len(), 2, "{:#?}", g.parameters);
    assert!(g.parameters[0].required);
    assert_eq!(
        g.parameters[1].schema.as_ref().unwrap()["type"],
        json!("object")
    );
}

#[tokio::test]
async fn security_inheritance_empty_array_and_combinations() {
    let (_d, e) = spec(
        "security:\n  - cookieAuth: []\npaths:\n  /inherit: {get: {operationId: inherit, responses: {'200': {description: ok}}}}\n  /none: {get: {operationId: none, security: [], responses: {'200': {description: ok}}}}\n  /or: {get: {operationId: or, security: [{a: []}, {b: []}], responses: {'200': {description: ok}}}}\n  /and: {get: {operationId: and, security: [{a: [], b: [x, y]}], responses: {'200': {description: ok}}}}\n  /optional: {get: {operationId: optional, security: [{}, {a: []}], responses: {'200': {description: ok}}}}\ncomponents:\n  securitySchemes:\n    cookieAuth: {type: apiKey, in: cookie, name: SESSION}\n    a: {type: http, scheme: Bearer}\n    b: {type: apiKey, in: header, name: X-Key}\n",
    );
    let l = loaded(&e).await;
    assert_eq!(op(&l, "inherit").security, None);
    assert_eq!(op(&l, "none").security, Some(vec![]));
    assert_eq!(op(&l, "or").security.as_ref().unwrap().len(), 2);
    let and = &op(&l, "and").security.as_ref().unwrap()[0];
    assert_eq!(and.schemes.len(), 2);
    assert_eq!(and.schemes[1].scopes, vec!["x", "y"]);
    // `security: [{}]` を含むと認証は任意
    let items: Vec<_> = l
        .operations
        .iter()
        .map(|o| {
            (
                o.op_key.to_string(),
                nekoreq_openapi::view::requires_auth(o, &l.meta),
            )
        })
        .collect();
    let req = |k: &str| items.iter().find(|(n, _)| n == k).unwrap().1;
    assert!(req("inherit"));
    assert!(!req("none"));
    assert!(req("or"));
    assert!(
        !req("optional"),
        "security に {{}} があるなら認証は任意: {items:?}"
    );
    assert_eq!(l.inspection.no_security_count, 1);
    // 既定の認証: none は認証なし、or は最初に対応できるもの（bearer の a）
    let auth = |k: &str| default_auth(op(&l, k), &l.meta, &ImportOptions::default());
    assert!(auth("none").is_none());
    assert!(matches!(auth("or"), Some(DefaultAuth::Bearer { .. })));
    assert!(matches!(auth("inherit"), Some(DefaultAuth::ApiKey { .. })));
}

#[tokio::test]
async fn all_of_merges_in_the_table_and_the_mock() {
    let (_d, e) = spec(
        "paths:\n  /a:\n    post:\n      operationId: a\n      requestBody:\n        content:\n          application/json:\n            schema: {$ref: '#/components/schemas/Child'}\n      responses:\n        '200': {description: ok}\ncomponents:\n  schemas:\n    Base:\n      type: object\n      required: [id]\n      properties:\n        id: {type: integer, readOnly: true}\n        name: {type: string, description: base-name}\n    Child:\n      allOf:\n        - $ref: '#/components/schemas/Base'\n        - type: object\n          required: [kind]\n          properties:\n            kind: {type: string, enum: [A, B]}\n            name: {type: string, description: child-name}\n      description: child\n",
    );
    let l = loaded(&e).await;
    let s = op(&l, "a").request_body.as_ref().unwrap().content[0]
        .schema
        .clone()
        .unwrap();
    let t = schema_table(&s);
    let rows: Vec<_> = t
        .rows
        .iter()
        .map(|r| (r.name.as_str(), r.required))
        .collect();
    assert_eq!(
        rows,
        vec![("id", true), ("name", false), ("kind", true)],
        "{rows:?}"
    );
    // 同名の項目は先に出てきたもの（Base）を使う（実装の決め）。少なくとも 1 行だけ
    assert_eq!(t.rows.iter().filter(|r| r.name == "name").count(), 1);
    // リクエストのモックは readOnly を除く
    let m = mock(&s, true).value;
    assert_eq!(m, json!({"name": "string", "kind": "A"}));
    let m = mock(&s, false).value;
    assert!(m.get("id").is_some());
}

#[test]
fn nullable_in_3_0_and_type_arrays_in_3_1_are_both_shown() {
    let rows = |s| schema_table(&s).rows;
    let r30 = rows(json!({"type":"object","properties":{"a":{"type":"string","nullable":true}}}));
    assert!(r30[0].nullable && r30[0].constraints.contains(&"nullable".to_string()));
    assert_eq!(r30[0].type_label, "string");
    let r31 = rows(
        json!({"type":"object","properties":{"a":{"type":["string","null"]},"b":{"type":["null","integer"],"minimum":1},"c":{"type":["null"]}}}),
    );
    assert!(r31[0].nullable);
    assert_eq!(r31[0].type_label, "string");
    assert_eq!(r31[1].type_label, "integer");
    assert!(r31[1].nullable);
    assert!(r31[2].nullable);
    // 3.1 の oneOf: [{type: string}, {type: 'null'}] は「string | null」
    let r = rows(
        json!({"type":"object","properties":{"a":{"oneOf":[{"type":"string"},{"type":"null"}]}}}),
    );
    assert_eq!(r[0].type_label, "string | null");
}

#[test]
fn constraints_are_shown_for_enum_format_ranges_and_patterns() {
    let c = constraints(&json!({
        "type": "string", "format": "date-time", "minLength": 1, "maxLength": 20,
        "pattern": "^[A-Z]+$", "default": "X", "enum": ["A", "B"]
    }));
    for want in [
        "enum: A, B",
        "format: date-time",
        "minLength: 1",
        "maxLength: 20",
        "pattern: ^[A-Z]+$",
        "default: X",
    ] {
        assert!(c.contains(&want.to_string()), "{want} が無い: {c:?}");
    }
    let c = constraints(
        &json!({"type":"integer","minimum":1,"maximum":9,"exclusiveMinimum":0,"exclusiveMaximum":10,"multipleOf":2}),
    );
    for want in [
        "min: 1",
        "max: 9",
        "exclusiveMin: 0",
        "exclusiveMax: 10",
        "multipleOf: 2",
    ] {
        assert!(c.contains(&want.to_string()), "{want} が無い: {c:?}");
    }
    // 長い enum は 12 個で切り、日本語は文字数で切る（途中で壊れない）
    let many: Vec<_> = (0..30).map(|i| json!(format!("値{i}"))).collect();
    let c = constraints(&json!({"enum": many}));
    assert!(c[0].ends_with(", …"), "{c:?}");
    let long = "あ".repeat(100);
    let c = constraints(&json!({"default": long}));
    assert!(c[0].ends_with('…'));
}

#[test]
fn boolean_exclusive_minimum_of_3_0_is_not_lost() {
    let c = constraints(
        &json!({"type":"integer","minimum":0,"exclusiveMinimum":true,"maximum":10,"exclusiveMaximum":true}),
    );
    // 3.0 の真偽値は「> 0」「< 10」と出す（0・10 を含まない）。「min: 0」とは出さない
    assert_eq!(c, vec!["> 0", "< 10"], "{c:?}");
    // 真偽値が false なら、これまでどおり含む
    let c = constraints(&json!({"minimum": 0, "exclusiveMinimum": false, "maximum": 10}));
    assert_eq!(c, vec!["min: 0", "max: 10"], "{c:?}");
}

#[tokio::test]
async fn examples_example_and_the_example_source_are_distinguished() {
    let (_d, e) = spec(
        "paths:\n  /a:\n    post:\n      operationId: a\n      parameters:\n        - {name: p1, in: query, example: from-example, schema: {type: string, example: from-schema, default: dflt}}\n        - {name: p2, in: query, examples: {one: {value: first}, two: {value: second}}, schema: {type: string}}\n        - {name: p3, in: query, schema: {type: string, default: dflt3}}\n        - {name: p4, in: query, schema: {type: string, example: schema-ex}}\n        - {name: p5, in: query, examples: {ext: {externalValue: 'http://x/y'}}, schema: {type: string}}\n      requestBody:\n        content:\n          application/json:\n            examples:\n              a: {summary: A, value: {n: 1}}\n              b: {$ref: '#/components/examples/B'}\n            schema: {type: object, properties: {n: {type: integer}}}\n          text/plain:\n            schema: {type: string, format: date-time}\n      responses:\n        '200': {description: ok, content: {application/json: {schema: {type: object, properties: {s: {type: string, enum: [X, Y]}, d: {type: string, format: date-time}, arr: {type: array, items: {type: integer}}}}}}}\ncomponents:\n  examples:\n    B: {value: {n: 2}}\n",
    );
    let l = loaded(&e).await;
    let o = op(&l, "a");
    let by = |n: &str| o.parameters.iter().find(|p| p.name == n).unwrap();
    assert_eq!(by("p1").example, Some(json!("from-example")));
    assert_eq!(by("p2").example, Some(json!("first")));
    assert_eq!(by("p3").example, None);
    assert_eq!(by("p3").default, Some(json!("dflt3")));
    assert_eq!(by("p4").example, Some(json!("schema-ex")));
    assert_eq!(by("p5").example, None);
    let init = nekoreq_openapi::view::initial_params(o, true);
    let val = |n: &str| init.iter().find(|p| p.name == n).unwrap().value.clone();
    assert_eq!(val("p1"), "from-example");
    assert_eq!(val("p3"), "dflt3");
    assert!(init.iter().all(|p| !p.enabled), "query は無効の状態");
    // 例（複数）: 先頭が example。設計タブの出どころは Yaml
    let d = describe_operation(o, &l.meta, &ImportOptions::default(), None, false);
    let rb = d.request_body.unwrap();
    assert_eq!(rb.contents[0].example, Some(json!({"n": 1})));
    assert_eq!(rb.contents[0].example_source, Some(ExampleSource::Yaml));
    // text/plain はスキーマだけ → 生成（format date-time）
    assert_eq!(
        rb.contents[1].example_source,
        Some(ExampleSource::Generated)
    );
    assert_eq!(
        rb.contents[1].example,
        Some(json!("2026-01-01T00:00:00+09:00"))
    );
    // レスポンス: enum を使うので SchemaExample
    let r = &d.responses[0].contents[0];
    assert_eq!(r.example_source, Some(ExampleSource::SchemaExample));
    assert_eq!(
        r.example,
        Some(json!({"s": "X", "d": "2026-01-01T00:00:00+09:00", "arr": [0]}))
    );
}

#[tokio::test]
async fn all_named_examples_are_kept() {
    let (_d, e) = spec(
        "paths:\n  /a:\n    post:\n      operationId: a\n      requestBody:\n        content:\n          application/json:\n            examples:\n              first: {value: {n: 1}}\n              second: {value: {n: 2}}\n            schema: {type: object}\n      responses:\n        '200': {description: ok}\n",
    );
    let l = loaded(&e).await;
    let blob = serde_json::to_string(&op(&l, "a").request_body).unwrap();
    assert!(
        blob.contains("\"n\":2"),
        "2 つ目の例が保存されていない: {blob}"
    );
    let media = &op(&l, "a").request_body.as_ref().unwrap().content[0];
    let names: Vec<_> = media.examples.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, vec!["first", "second"]);
    assert_eq!(media.examples[1].value, json!({"n": 2}));
    // `example` は先頭と同じ（これまでの動き）
    assert_eq!(media.example, Some(json!({"n": 1})));
}

#[tokio::test]
async fn examples_keep_summary_description_refs_and_array_form_and_skip_external_values() {
    let (_d, e) = spec(
        "paths:\n  /a:\n    post:\n      operationId: a\n      requestBody:\n        content:\n          application/json:\n            example: {n: 0}\n            examples:\n              plain:\n                summary: 普通\n                description: 説明です\n                value: {n: 1}\n              viaRef: {$ref: '#/components/examples/Shared'}\n              external: {externalValue: 'https://example.com/x.json'}\n              broken: {$ref: '#/components/examples/Nope'}\n            schema: {type: object}\n      responses:\n        '200':\n          description: ok\n          content:\n            text/plain:\n              examples: [first, second]\n              schema: {type: string}\ncomponents:\n  examples:\n    Shared: {summary: 共通, value: {n: 3}}\n",
    );
    let l = loaded(&e).await;
    let o = op(&l, "a");
    let media = &o.request_body.as_ref().unwrap().content[0];
    let got: Vec<_> = media
        .examples
        .iter()
        .map(|x| (x.name.as_str(), x.value.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("example", json!({"n": 0})),
            ("plain", json!({"n": 1})),
            ("viaRef", json!({"n": 3})),
        ]
    );
    assert_eq!(media.examples[1].summary.as_deref(), Some("普通"));
    assert_eq!(media.examples[1].description.as_deref(), Some("説明です"));
    assert_eq!(media.examples[2].summary.as_deref(), Some("共通"));
    // `example` と `examples` の両方があれば `example` が先頭
    assert_eq!(media.example, Some(json!({"n": 0})));
    // 配列で書かれた例
    let r = &o.responses[0].content[0];
    let got: Vec<_> = r
        .examples
        .iter()
        .map(|x| (x.name.as_str(), x.value.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("example 1", json!("first")),
            ("example 2", json!("second"))
        ]
    );
}

#[tokio::test]
async fn design_tab_lists_every_example_and_changing_one_counts_as_a_change() {
    let body = |second: i32| {
        format!(
            "paths:\n  /a:\n    post:\n      operationId: a\n      requestBody:\n        content:\n          application/json:\n            examples:\n              first: {{value: {{n: 1}}}}\n              second: {{value: {{n: {second}}}}}\n            schema: {{type: object}}\n      responses:\n        '200': {{description: ok}}\n"
        )
    };
    let (_d, e) = spec(&body(2));
    let l = loaded(&e).await;
    let detail = describe_operation(op(&l, "a"), &l.meta, &ImportOptions::default(), None, false);
    let media = &detail.request_body.as_ref().unwrap().contents[0];
    assert_eq!(media.examples.len(), 2);
    assert_eq!(media.examples[1].name, "second");
    // 表示する例は先頭の yaml の例（出どころは yaml）
    assert_eq!(media.example, Some(json!({"n": 1})));
    assert_eq!(media.example_source, Some(ExampleSource::Yaml));
    // example が 1 つもないメディア型は空
    let (_d2, e2) = spec(
        "paths:\n  /b:\n    post:\n      operationId: b\n      requestBody:\n        content:\n          application/json:\n            schema: {type: object}\n      responses:\n        '200': {description: ok}\n",
    );
    let l2 = loaded(&e2).await;
    let d2 = describe_operation(
        op(&l2, "b"),
        &l2.meta,
        &ImportOptions::default(),
        None,
        false,
    );
    assert!(d2.request_body.unwrap().contents[0].examples.is_empty());

    // 2 つ目の例だけを書き換えたら、差分に「変更」で出る
    let (_d3, e3) = spec(&body(9));
    let l3 = loaded(&e3).await;
    let diff = diff_operations(&l.operations, &l3.operations, &HashSet::new());
    assert_eq!(diff.changed.len(), 1, "{diff:?}");
}

#[test]
fn mock_handles_enum_format_arrays_cycles_and_composition() {
    let s = json!({
        "type": "object",
        "properties": {
            "when": {"type": "string", "format": "date-time"},
            "day": {"type": "string", "format": "date"},
            "id": {"type": "string", "format": "uuid"},
            "list": {"type": "array", "items": {"type": "object", "properties": {"k": {"type": "boolean"}}}},
            "one": {"oneOf": [{"type": "null"}, {"type": "integer"}]},
            "nested": {"type": "object", "properties": {"self": {"x-nekoreq-circular": true, "x-nekoreq-ref": "N"}}},
            "num": {"type": "number", "minimum": 5},
            "nul": {"type": ["string", "null"]},
            "short": {"type": "string", "minLength": 3, "maxLength": 3},
            "unres": {"x-nekoreq-unresolved": "#/x"}
        }
    });
    let m = mock(&s, true);
    assert!(!m.used_schema_example);
    assert_eq!(m.value["when"], json!("2026-01-01T00:00:00+09:00"));
    assert_eq!(m.value["day"], json!("2026-01-01"));
    assert_eq!(m.value["id"], json!("3fa85f64-5717-4562-b3fc-2c963f66afa6"));
    assert_eq!(m.value["list"], json!([{"k": false}]));
    assert_eq!(m.value["one"], json!(0));
    assert_eq!(m.value["nested"], json!({"self": null}));
    assert_eq!(m.value["num"], json!(5));
    assert_eq!(m.value["nul"], json!("string"));
    assert_eq!(m.value["short"].as_str().unwrap().chars().count(), 3);
    assert_eq!(m.value["unres"], json!(null));
}

#[test]
fn mock_respects_maximum_and_min_items() {
    let m = mock(&json!({"type": "integer", "maximum": -5}), true).value;
    assert!(m.as_i64().unwrap() <= -5, "{m}");
    let m = mock(
        &json!({"type": "array", "minItems": 3, "items": {"type": "string"}}),
        true,
    )
    .value;
    assert!(m.as_array().unwrap().len() >= 3, "{m}");
    // maxItems を超えない・minimum と maximum の間・3.0 の真偽値の排他
    let m = mock(
        &json!({"type": "array", "maxItems": 0, "items": {"type": "string"}}),
        true,
    )
    .value;
    assert_eq!(m, json!([]));
    assert_eq!(
        mock(
            &json!({"type": "integer", "minimum": 5, "maximum": 9}),
            true
        )
        .value,
        json!(5)
    );
    assert_eq!(
        mock(
            &json!({"type": "integer", "minimum": 0, "exclusiveMinimum": true, "maximum": 10}),
            true
        )
        .value,
        json!(1)
    );
    assert_eq!(
        mock(&json!({"type": "integer", "exclusiveMaximum": 0}), true).value,
        json!(-1)
    );
    // minItems が入れ子で重なっても膨れない
    let mut nested = json!({"type": "string"});
    for _ in 0..12 {
        nested = json!({"type": "array", "minItems": 10, "items": nested});
    }
    let m = mock(&nested, true).value;
    assert!(serde_json::to_string(&m).unwrap().len() < 1_000_000);
}

#[tokio::test]
async fn openapi_3_1_and_3_2_specific_syntax_is_read() {
    let y = "openapi: 3.1.0\ninfo: {title: T, version: '1'}\npaths:\n  /a:\n    $ref: '#/components/pathItems/A'\n  /q:\n    query:\n      operationId: q\n      responses:\n        '200': {description: ok}\n    additionalOperations:\n      LINK:\n        operationId: link\n        responses:\n          '200': {description: ok}\nwebhooks:\n  hook:\n    post:\n      operationId: hook\n      responses:\n        '200': {description: ok}\ncomponents:\n  pathItems:\n    A:\n      get:\n        operationId: a\n        responses:\n          '200':\n            description: ok\n            content:\n              application/json:\n                schema: {$ref: '#/$defs/Pt'}\n$defs:\n  Pt:\n    type: object\n    properties:\n      xy: {type: array, prefixItems: [{type: number}, {type: number}], minItems: 2}\n      c: {const: fixed}\n      n: {type: integer, exclusiveMinimum: 0}\n";
    let d = tempfile::tempdir().unwrap();
    let e = d.path().join("openapi.yaml");
    fs::write(&e, y).unwrap();
    let l = loaded(&e).await;
    let keys: Vec<_> = l
        .operations
        .iter()
        .map(|o| (o.method.clone(), o.op_key.to_string()))
        .collect();
    assert!(keys.contains(&("GET".into(), "a".into())), "{keys:?}");
    assert!(keys.contains(&("QUERY".into(), "q".into())), "{keys:?}");
    assert!(keys.contains(&("LINK".into(), "link".into())), "{keys:?}");
    assert!(
        !keys.iter().any(|(_, k)| k == "hook"),
        "webhooks は API ではない: {keys:?}"
    );
    let s = op(&l, "a").responses[0].content[0].schema.clone().unwrap();
    let t = schema_table(&s);
    let n = t.rows.iter().find(|r| r.name == "n").unwrap();
    assert!(n.constraints.contains(&"exclusiveMin: 0".to_string()));
    assert!(t
        .rows
        .iter()
        .find(|r| r.name == "c")
        .unwrap()
        .constraints
        .iter()
        .any(|c| c.starts_with("const")));
}

#[tokio::test]
async fn three_x_versions_are_accepted_or_refused_precisely() {
    for (v, ok) in [
        ("3.0.0", true),
        ("'3.0'", true),
        ("3.0", true),
        ("3.1", true),
        ("3.2.0", true),
        ("'3.10.0'", false),
        ("'3.3.0'", false),
        ("'30.0.0'", false),
        ("2.0", false),
        ("'3.0.3-rc1'", true),
    ] {
        let (_d, e) = tree(&[(
            "openapi.yaml",
            &format!("openapi: {v}\ninfo: {{title: T, version: '1'}}\npaths: {{}}\n"),
        )]);
        let r = load_path(&e).await;
        assert_eq!(matches!(r, LoadResult::Loaded(_)), ok, "openapi: {v}");
    }
}

// ================= 行番号・文字の扱い =================

fn line_of_op(l: &Loaded, key: &str) -> Option<u32> {
    op(l, key).source_line
}

#[tokio::test]
async fn line_numbers_survive_multiline_strings_flow_style_comments_and_anchors() {
    let y = format!(
        "{HEAD}# コメント 1\n# コメント 2\nx-anchors:\n  base: &base\n    responses:\n      '200': {{description: ok}}\npaths:\n  /a:\n    description: |\n      複数行の\n      説明です\n      3 行目\n    get:\n      operationId: aGet\n      summary: >\n        折りたたみ\n        文字列\n      responses:\n        '200': {{description: ok}}\n    post: {{operationId: aPost, responses: {{'200': {{description: ok}}}}}}\n    put:\n      <<: *base\n      operationId: aPut\n  /b:\n    # ここにコメント\n    get: &getb\n      operationId: bGet\n      responses:\n        '200': {{description: \"line1\n          line2\"}}\n    delete: *getb\n"
    );
    let (_d, e) = tree(&[("openapi.yaml", &y)]);
    let l = loaded(&e).await;
    let text = fs::read_to_string(&e).unwrap();
    let find = |needle: &str| -> u32 {
        text.lines()
            .position(|l| l.trim_start().starts_with(needle))
            .unwrap() as u32
            + 1
    };
    assert_eq!(line_of_op(&l, "aGet"), Some(find("get:")));
    assert_eq!(line_of_op(&l, "aPost"), Some(find("post:")));
    assert_eq!(line_of_op(&l, "aPut"), Some(find("put:")));
    assert_eq!(line_of_op(&l, "GET /b"), Some(find("get: &getb")));
    // エイリアスで使い回した delete: *getb は delete のキーの行（getb の定義の行ではない）
    let deleted = l
        .operations
        .iter()
        .find(|o| o.method == "DELETE")
        .expect("DELETE");
    assert_eq!(deleted.source_line, Some(find("delete:")), "{deleted:?}");
}

#[tokio::test]
async fn merge_keys_are_expanded() {
    let y = format!(
        "{HEAD}x-anchors:\n  base: &base\n    responses:\n      '200': {{description: ok}}\npaths:\n  /a:\n    put:\n      <<: *base\n      operationId: aPut\n"
    );
    let (_d, e) = tree(&[("openapi.yaml", &y)]);
    let l = loaded(&e).await;
    assert_eq!(
        op(&l, "aPut").responses.len(),
        1,
        "{:#?}",
        op(&l, "aPut").responses
    );
    // `<<` というキーは残らない
    assert!(
        !l.operations.iter().any(|o| o.path.contains("<<")),
        "{:#?}",
        l.operations
    );
}

#[tokio::test]
async fn merge_key_lists_and_precedence_follow_yaml() {
    // `<<: [*a, *b]` は前のものが勝つ。マッピングに直接書いたキーは、`<<` の前でも後でも勝つ
    let y = format!(
        "{HEAD}x-anchors:\n  a: &a\n    summary: from-a\n    description: from-a\n  b: &b\n    summary: from-b\n    description: from-b\n    tags: [from-b]\npaths:\n  /x:\n    get:\n      <<: [*a, *b]\n      operationId: x\n      description: direct\n      responses:\n        '200': {{description: ok}}\n  /y:\n    get:\n      description: direct-first\n      <<: *a\n      operationId: y\n      responses:\n        '200': {{description: ok}}\n"
    );
    let (_d, e) = tree(&[("openapi.yaml", &y)]);
    let l = loaded(&e).await;
    let x = op(&l, "x");
    assert_eq!(x.summary.as_deref(), Some("from-a"));
    assert_eq!(x.description.as_deref(), Some("direct"));
    assert_eq!(x.tags, vec!["from-b"]);
    let yy = op(&l, "y");
    assert_eq!(yy.summary.as_deref(), Some("from-a"));
    assert_eq!(yy.description.as_deref(), Some("direct-first"));
    assert_eq!(x.source_line, Some(13));
}

#[tokio::test]
async fn bom_crlf_and_cr_only_files_give_the_same_line_numbers() {
    let base = "openapi: 3.0.3\ninfo: {title: T, version: '1'}\npaths:\n  /a:\n    description: |\n      一行目\n      二行目\n    get:\n      operationId: g\n      responses:\n        '200': {description: ok}\n  /b:\n    $ref: './part.yaml#/p'\n";
    let part =
        "# c\np:\n  post:\n    operationId: p\n    responses:\n      '200': {description: ok}\n";
    for (name, conv) in [
        (
            "lf",
            Box::new(|s: &str| s.to_string()) as Box<dyn Fn(&str) -> String>,
        ),
        ("bom", Box::new(|s: &str| format!("\u{feff}{s}"))),
        ("crlf", Box::new(|s: &str| s.replace('\n', "\r\n"))),
        (
            "bom+crlf",
            Box::new(|s: &str| format!("\u{feff}{}", s.replace('\n', "\r\n"))),
        ),
        ("cr", Box::new(|s: &str| s.replace('\n', "\r"))),
    ] {
        let (_d, e) = tree(&[("openapi.yaml", &conv(base)), ("part.yaml", &conv(part))]);
        let l = loaded(&e).await;
        assert_eq!(line_of_op(&l, "g"), Some(8), "{name}");
        assert_eq!(line_of_op(&l, "p"), Some(3), "{name}");
        // 説明の複数行（CRLF でも \r が残らない）
        let d = op(&l, "g").description.clone().unwrap_or_default();
        assert!(!d.contains('\r'), "{name}: {d:?}");
        // 足りないファイルの参照元の行
        fs::remove_file(e.parent().unwrap().join("part.yaml")).unwrap();
        let p = blocked(&e).await;
        let LoadProblem::MissingFile {
            referenced_from, ..
        } = &p[0]
        else {
            panic!("{p:?}")
        };
        assert_eq!(referenced_from[0].line, Some(13), "{name}");
    }
}

#[tokio::test]
async fn broken_yaml_in_a_referenced_file_reports_that_file_line_and_column() {
    let (_d, e) = tree(&[
        (
            "openapi.yaml",
            &format!("{HEAD}paths:\n  /a: {{$ref: './sub/bad.yaml#/p'}}\n"),
        ),
        (
            "sub/bad.yaml",
            "p:\n  get:\n    operationId: x\n    responses: [1, 2\n    summary: y\n",
        ),
    ]);
    let p = blocked(&e).await;
    let LoadProblem::YamlError {
        file,
        line,
        column,
        reason,
    } = &p[0]
    else {
        panic!("{p:?}")
    };
    assert_eq!(file, "sub/bad.yaml");
    assert!(line.is_some_and(|l| (4..=6).contains(&l)), "{line:?}");
    assert!(column.is_some());
    assert!(!reason.is_empty());
}

#[tokio::test]
async fn duplicate_keys_tabs_and_invalid_utf8_are_reported_not_ignored() {
    // 重複キー
    let (_d, e) = spec("paths:\n  /a:\n    get: {operationId: one, responses: {'200': {description: ok}}}\n    get: {operationId: two, responses: {'200': {description: ok}}}\n");
    let r = load_path(&e).await;
    match r {
        LoadResult::Blocked(p) => assert!(p[0].describe().contains("openapi.yaml")),
        LoadResult::Loaded(l) => {
            // 読めてしまう場合でも、どちらかに決まっていて、2 件にはならない
            assert_eq!(l.operations.len(), 1);
        }
        _ => panic!(),
    }
    // UTF-8 でないファイル（Shift_JIS）
    let d = tempfile::tempdir().unwrap();
    let e = d.path().join("openapi.yaml");
    let mut bytes = format!("{HEAD}paths: {{}}\nx-memo: ").into_bytes();
    bytes.extend_from_slice(&[0x83, 0x65, 0x83, 0x58, 0x83, 0x67]); // テスト (Shift_JIS)
    fs::write(&e, bytes).unwrap();
    let p = blocked(&e).await;
    assert!(
        matches!(&p[0], LoadProblem::YamlError { reason, .. } if reason.contains("UTF-8")),
        "{p:?}"
    );
}

#[tokio::test]
async fn yaml_keys_that_look_like_booleans_or_null_keep_their_text() {
    // 3.0 の example やプロパティ名の `on` `no` `yes` `y` `n` は文字列（YAML 1.2）
    let (_d, e) = spec(
        "paths:\n  /a:\n    post:\n      operationId: a\n      requestBody:\n        content:\n          application/json:\n            schema:\n              type: object\n              properties:\n                on: {type: string}\n                no: {type: string}\n                yes: {type: string}\n                y: {type: string}\n                'null': {type: string}\n                '001': {type: string}\n      responses:\n        '200': {description: ok}\n",
    );
    let l = loaded(&e).await;
    let s = op(&l, "a").request_body.as_ref().unwrap().content[0]
        .schema
        .clone()
        .unwrap();
    let names: Vec<_> = s["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(names, vec!["on", "no", "yes", "y", "null", "001"]);
}

#[tokio::test]
async fn info_version_keeps_its_written_form_and_other_numbers_follow_yaml() {
    // 決定: info.version は OpenAPI では string なので、数値に見えても書かれた文字列のまま。
    // example など、ほかの値は YAML の仕様どおり（0.10 は数値の 0.1）
    for (written, want) in [
        ("1.10", "1.10"),
        ("001", "001"),
        ("1.0", "1.0"),
        ("2", "2"),
        ("1e3", "1e3"),
        ("true", "true"),
        ("'1.10'", "1.10"),
        ("v1.10", "v1.10"),
    ] {
        let (_d, e) = tree(&[(
            "openapi.yaml",
            &format!("openapi: 3.0.3\ninfo: {{title: T, version: {written}}}\npaths: {{}}\n"),
        )]);
        let l = loaded(&e).await;
        assert_eq!(l.meta.version, want, "version: {written}");
    }
    // ブロック形式・行末のコメント付きでも同じ
    let (_d, e) = tree(&[(
        "openapi.yaml",
        "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1.10 # 書いたまま\npaths:\n  /a:\n    get:\n      operationId: a\n      parameters:\n        - {name: q, in: query, example: 0.10, schema: {type: number}}\n      responses:\n        '200': {description: ok}\n",
    )]);
    let l = loaded(&e).await;
    assert_eq!(l.meta.version, "1.10");
    assert_eq!(op(&l, "a").parameters[0].example, Some(json!(0.1)));
}

#[tokio::test]
async fn huge_integers_and_floats_do_not_panic() {
    let (_d, e) = spec(
        "paths:\n  /a:\n    get:\n      operationId: a\n      parameters:\n        - {name: big, in: query, schema: {type: integer, maximum: 18446744073709551615, minimum: -9223372036854775809, example: 99999999999999999999}}\n        - {name: f, in: query, schema: {type: number, maximum: .inf, minimum: -.inf, default: .nan, example: 1e400}}\n      responses:\n        '200': {description: ok}\n",
    );
    let l = loaded(&e).await;
    let d = describe_operation(op(&l, "a"), &l.meta, &ImportOptions::default(), None, false);
    assert_eq!(d.parameters.len(), 2);
    let _ = nekoreq_openapi::view::initial_params(op(&l, "a"), true);
    // 変換の結果が JSON として保存できる（NaN / Infinity は null か文字列になっている）
    serde_json::to_string(&l.operations).unwrap();
}

#[tokio::test]
async fn no_absolute_local_paths_leak_into_what_the_screen_receives() {
    let (d, e) = tree(&[
        (
            "openapi.yaml",
            &format!(
                "{HEAD}paths:\n  /a: {{$ref: './sub/a.yaml#/p'}}\n  /m: {{$ref: './gone.yaml'}}\n"
            ),
        ),
        (
            "sub/a.yaml",
            &format!("p:\n  {}", ok_get("a").replace('\n', "\n  ")),
        ),
    ]);
    let abs = d.path().to_string_lossy().to_string();
    let p = blocked(&e).await;
    assert!(!format!("{p:#?}").contains(&abs), "{p:#?}");
    fs::write(d.path().join("gone.yaml"), ok_get("m")).unwrap();
    let l = loaded(&e).await;
    let preview = build_preview(&l, None);
    let blob = format!("{preview:#?}");
    assert!(!blob.contains(&abs), "確認画面のデータに絶対パスが出ている");
    let det = describe_operation(op(&l, "a"), &l.meta, &ImportOptions::default(), None, false);
    assert!(!format!("{det:#?}").contains(&abs));
}

#[tokio::test]
async fn html_in_descriptions_is_passed_through_untouched_for_the_screen_to_sanitize() {
    let (_d, e) = spec(
        "paths:\n  /a:\n    get:\n      operationId: a\n      summary: <img src=x onerror=alert(1)>\n      description: |\n        <script>alert('x')</script>\n        [x](javascript:alert(1))\n      responses:\n        '200': {description: '<b>ok</b>'}\n",
    );
    let l = loaded(&e).await;
    let d = describe_operation(op(&l, "a"), &l.meta, &ImportOptions::default(), None, false);
    assert!(d
        .description
        .unwrap()
        .contains("<script>alert('x')</script>"));
    assert_eq!(d.responses[0].description.as_deref(), Some("<b>ok</b>"));
}

// ================= 差分 =================

fn ops_of(l: &Loaded) -> Vec<nekoreq_core::Operation> {
    l.operations.clone()
}

fn diff_of(a: &Loaded, b: &Loaded, drafts: &[&str]) -> SyncDiff {
    let d: HashSet<OpKey> = drafts.iter().map(|k| OpKey::Id((*k).to_string())).collect();
    diff_operations(&ops_of(a), &ops_of(b), &d)
}

const BASE: &str = "paths:\n  /a:\n    get:\n      operationId: getA\n      summary: A\n      parameters:\n        - {name: q, in: query, schema: {type: string}}\n        - {name: limit, in: query, schema: {type: integer}}\n      responses:\n        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/S'}}}}\n  /b:\n    get:\n      operationId: getB\n      responses:\n        '200': {description: ok}\ncomponents:\n  schemas:\n    S: {type: object, properties: {id: {type: integer}}}\n";

async fn two(base: &str, edited: &str) -> (Loaded, Loaded) {
    let (_d1, e1) = spec(base);
    let a = loaded(&e1).await;
    let (_d2, e2) = spec(edited);
    let b = loaded(&e2).await;
    (a, b)
}

#[tokio::test]
async fn identical_reads_have_an_empty_diff() {
    let (a, b) = two(BASE, BASE).await;
    assert!(diff_of(&a, &b, &[]).is_empty());
}

#[tokio::test]
async fn moving_lines_comments_and_reordering_paths_are_not_changes() {
    let edited = format!(
        "# 先頭にコメント\n\n{}",
        BASE.replace("paths:\n", "paths:\n  # 説明\n")
            .replace("  /a:", "\n\n  /a:")
    );
    let (a, b) = two(BASE, &edited).await;
    assert!(
        diff_of(&a, &b, &[]).is_empty(),
        "{:#?}",
        diff_of(&a, &b, &[])
    );
    // path の順序が入れ替わっても変更ではない
    let swapped = "paths:\n  /b:\n    get:\n      operationId: getB\n      responses:\n        '200': {description: ok}\n  /a:\n    get:\n      operationId: getA\n      summary: A\n      parameters:\n        - {name: q, in: query, schema: {type: string}}\n        - {name: limit, in: query, schema: {type: integer}}\n      responses:\n        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/S'}}}}\ncomponents:\n  schemas:\n    S: {type: object, properties: {id: {type: integer}}}\n";
    let (a, b) = two(BASE, swapped).await;
    assert!(diff_of(&a, &b, &[]).is_empty());
}

#[tokio::test]
async fn each_kind_of_change_is_classified() {
    let cases: Vec<(&str, String, ChangedField)> = vec![
        ("summary", BASE.replace("summary: A", "summary: A2"), ChangedField::Summary),
        ("param added", BASE.replace("{name: limit,", "{name: page, in: query, schema: {type: integer}}\n        - {name: limit,"), ChangedField::Parameters),
        ("param removed", BASE.replace("        - {name: limit, in: query, schema: {type: integer}}\n", ""), ChangedField::Parameters),
        ("param type", BASE.replace("{name: limit, in: query, schema: {type: integer}}", "{name: limit, in: query, schema: {type: string}}"), ChangedField::Parameters),
        ("shared schema", BASE.replace("id: {type: integer}", "id: {type: string}"), ChangedField::Responses),
        ("response status", BASE.replace("'200': {description: ok, content", "'201': {description: ok, content"), ChangedField::Responses),
        ("security", BASE.replace("      summary: A\n", "      summary: A\n      security: []\n"), ChangedField::Security),
        ("tags", BASE.replace("      summary: A\n", "      summary: A\n      tags: [x]\n"), ChangedField::Tags),
        ("body", BASE.replace("      responses:\n        '200': {description: ok, content: {application/json", "      requestBody: {content: {application/json: {schema: {type: object}}}}\n      responses:\n        '200': {description: ok, content: {application/json"), ChangedField::RequestBody),
        ("description", BASE.replace("      summary: A\n", "      summary: A\n      description: new\n"), ChangedField::Description),
    ];
    for (name, edited, field) in cases {
        let (a, b) = two(BASE, &edited).await;
        let d = diff_of(&a, &b, &[]);
        assert!(d.added.is_empty() && d.removed.is_empty(), "{name}: {d:#?}");
        let keys: Vec<_> = d
            .changed
            .iter()
            .map(|c| c.entry.op_key.to_string())
            .collect();
        assert_eq!(keys, vec!["getA"], "{name}: {d:#?}");
        assert!(
            d.changed[0].changes.iter().any(|c| c.field == field),
            "{name}: {:#?}",
            d.changed[0].changes
        );
    }
}

#[tokio::test]
async fn removed_parameters_are_named_and_drafts_are_flagged() {
    let edited = BASE.replace(
        "        - {name: limit, in: query, schema: {type: integer}}\n",
        "",
    );
    let (a, b) = two(BASE, &edited).await;
    let d = diff_of(&a, &b, &["getA"]);
    assert_eq!(d.changed[0].removed_params, vec!["limit"]);
    assert!(d.changed[0].entry.has_draft);
    assert!(
        d.changed[0].changes[0].note.contains("削除: limit"),
        "{:?}",
        d.changed[0].changes[0].note
    );
}

#[tokio::test]
async fn adding_and_removing_apis_and_operation_id_renames() {
    let edited = BASE
        .replace("operationId: getA", "operationId: fetchA")
        .replace("  /b:\n    get:\n      operationId: getB\n      responses:\n        '200': {description: ok}\n", "  /c:\n    post:\n      operationId: postC\n      responses:\n        '200': {description: ok}\n");
    let (a, b) = two(BASE, &edited).await;
    let d = diff_of(&a, &b, &["getA", "getB"]);
    assert_eq!(d.added.len(), 1);
    assert_eq!(d.added[0].op_key.to_string(), "postC");
    assert_eq!(d.removed.len(), 1);
    assert_eq!(d.removed[0].op_key.to_string(), "getB");
    assert!(d.removed[0].has_draft);
    assert_eq!(d.changed.len(), 1);
    let c = &d.changed[0];
    assert_eq!(c.renamed_from, Some(OpKey::Id("getA".into())));
    assert!(c
        .changes
        .iter()
        .any(|c| c.field == ChangedField::OperationId));
    assert!(c.entry.has_draft, "付け替え元の入力値があること");
}

#[tokio::test]
async fn removing_the_operation_id_falls_back_to_method_path_and_back() {
    let (a, b) = two(BASE, &BASE.replace("      operationId: getB\n", "")).await;
    let d = diff_of(&a, &b, &[]);
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].renamed_from, Some(OpKey::Id("getB".into())));
    assert_eq!(d.changed[0].entry.op_key, OpKey::method_path("GET", "/b"));
    assert!(d.added.is_empty() && d.removed.is_empty());
}

#[tokio::test]
async fn moving_an_api_to_another_file_is_not_a_change() {
    let (_d1, e1) = spec(BASE);
    let a = loaded(&e1).await;
    // /a を別ファイルへ移す
    let moved = BASE.replace(
        "  /a:\n    get:\n      operationId: getA\n      summary: A\n      parameters:\n        - {name: q, in: query, schema: {type: string}}\n        - {name: limit, in: query, schema: {type: integer}}\n      responses:\n        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/S'}}}}\n",
        "  /a: {$ref: './a.yaml'}\n",
    );
    let (_d2, e2) = tree(&[
        ("openapi.yaml", &format!("{HEAD}{moved}")),
        ("a.yaml", "get:\n  operationId: getA\n  summary: A\n  parameters:\n    - {name: q, in: query, schema: {type: string}}\n    - {name: limit, in: query, schema: {type: integer}}\n  responses:\n    '200': {description: ok, content: {application/json: {schema: {$ref: './openapi.yaml#/components/schemas/S'}}}}\n"),
    ]);
    let b = loaded(&e2).await;
    assert_eq!(op(&b, "getA").source_file.as_deref(), Some("a.yaml"));
    assert!(
        diff_of(&a, &b, &[]).is_empty(),
        "{:#?}",
        diff_of(&a, &b, &[])
    );
}

// ================= 性能 =================

fn synthetic_split(dir: &Path, n_ops: usize, files: usize) -> PathBuf {
    // タグ 20 個、共有スキーマ 50 個（5 段の入れ子）、ファイルあたり n_ops/files 件
    // 5 つごとに入れ子が終わる（i % 5 == 4 は葉）。展開しても爆発しない、実際の API に近い形
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
    let mut entry = format!("{HEAD}tags:\n");
    for t in 0..20 {
        entry.push_str(&format!("  - name: tag{t}\n    description: タグ {t}\n"));
    }
    entry.push_str("paths:\n");
    let mut k = 0;
    for f in 0..files {
        let mut body = String::new();
        for _ in 0..per {
            body.push_str(&format!(
                "/r/{k}/{{id}}:\n  parameters:\n    - {{name: id, in: path, required: true, schema: {{type: integer}}}}\n  get:\n    operationId: get{k}\n    summary: 取得 {k}\n    description: |\n      説明 {k}\n      | status | code | 状況 |\n      | --- | --- | --- |\n      | 404 | NOT_FOUND | なし |\n    tags: [tag{}]\n    parameters:\n      - {{name: q, in: query, schema: {{type: string}}}}\n    responses:\n      '200':\n        description: ok\n        content:\n          application/json:\n            schema: {{$ref: '../common.yaml#/S{}'}}\n      '404': {{description: nf}}\n",
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
            let esc = format!("~1r~1{k}~1{{id}}")
                .replace('{', "%7B")
                .replace('}', "%7D");
            entry.push_str(&format!(
                "  /r/{k}/{{id}}: {{$ref: './paths/f{f}.yaml#/{esc}'}}\n"
            ));
            k += 1;
        }
    }
    let e = dir.join("openapi.yaml");
    fs::write(&e, entry).unwrap();
    e
}

#[tokio::test]
async fn synthetic_1000_operations_load_and_describe_in_bounded_time() {
    let d = tempfile::tempdir().unwrap();
    let e = synthetic_split(d.path(), 1000, 25);
    let t = Instant::now();
    let l = loaded(&e).await;
    let load_t = t.elapsed();
    assert_eq!(l.operations.len(), 1000);
    assert_eq!(l.files.len(), 27);
    assert!(l.inspection.unresolved_refs.is_empty());
    assert_eq!(l.inspection.markdown_table_count, 1000);
    let t = Instant::now();
    let preview = build_preview(&l, None);
    let prev_t = t.elapsed();
    assert_eq!(preview.operation_count, 1000);
    let t = Instant::now();
    let a = ops_of(&l);
    let l2 = loaded(&e).await;
    let diff = diff_operations(&a, &l2.operations, &HashSet::new());
    let diff_t = t.elapsed();
    assert!(diff.is_empty());
    let t = Instant::now();
    let mut rows = 0;
    for o in l.operations.iter().take(100) {
        let det = describe_operation(o, &l.meta, &ImportOptions::default(), None, false);
        rows += det
            .responses
            .iter()
            .flat_map(|r| &r.contents)
            .map(|c| c.rows.len())
            .sum::<usize>();
    }
    let desc_t = t.elapsed();
    let json = serde_json::to_string(&l.operations).unwrap();
    eprintln!(
        "PERF 1000 ops / 27 files: load={load_t:?} preview={prev_t:?} reload+diff={diff_t:?} describe(100 ops, {rows} rows)={desc_t:?} stored_json={} KB",
        json.len() / 1024
    );
    assert!(load_t.as_secs() < 60, "{load_t:?}");
}

#[tokio::test]
async fn a_fan_out_of_refs_is_cut_at_the_node_limit_and_marked() {
    // S0 -> (a, b) -> S1 -> (a, b) -> ... 展開すると 2^n。1 Operation あたりのノード数の上限で止める
    let n = 22;
    let mut y = String::from("paths:\n  /a:\n    get:\n      operationId: getA\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {$ref: '#/components/schemas/S0'}\n  /small:\n    get:\n      operationId: small\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema: {$ref: '#/components/schemas/S21'}\ncomponents:\n  schemas:\n");
    for i in 0..n {
        y.push_str(&format!(
            "    S{i}:\n      type: object\n      description: level {i}\n      properties:\n        a: {{$ref: '#/components/schemas/S{}'}}\n        b: {{$ref: '#/components/schemas/S{}'}}\n",
            i + 1,
            i + 1
        ));
    }
    y.push_str(&format!("    S{n}:\n      type: string\n"));
    let (_d, e) = spec(&y);
    let t = Instant::now();
    let l = loaded(&e).await;
    assert!(t.elapsed().as_secs() < 10, "{:?}", t.elapsed());
    // 上限は Operation ごと: 小さい方は打ち切られない
    let small = serde_json::to_string(op(&l, "small")).unwrap();
    assert!(!small.contains("x-nekoreq-truncated"));
    let big = op(&l, "getA");
    let json = serde_json::to_string(big).unwrap();
    assert!(json.contains("\"x-nekoreq-truncated\":true"));
    assert!(json.len() < 20 * 1024 * 1024, "{} bytes", json.len());
    // 設計タブ: 行の説明に「大きすぎるため省略」が入る。モックも作れる
    let d = describe_operation(big, &l.meta, &ImportOptions::default(), None, false);
    let schema = big.responses[0].content[0].schema.clone().unwrap();
    let t = schema_table(&schema);
    assert!(
        t.rows.iter().any(|r| {
            r.constraints.iter().any(|c| c == "省略")
                && r.description
                    .as_deref()
                    .is_some_and(|d| d.contains("大きすぎるため省略"))
        }),
        "省略の行がない"
    );
    assert!(t.rows.len() < 40_000, "{}", t.rows.len());
    let _ = d;
    let _ = mock(&schema, false);
    // 同期の差分: 同じものを読み直しても変わらない（打ち切りは決まった場所で起きる）
    let l2 = loaded(&e).await;
    assert_eq!(op(&l2, "getA").content_hash, big.content_hash);
}
