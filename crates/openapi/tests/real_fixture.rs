//! 実際の API 定義（コンテナの /fixtures。環境変数 NEKOREQ_FIXTURE_DIR / NEKOREQ_FIXTURE_BUNDLED）での確認。
//! 無い環境ではスキップする。

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use common::*;
use nekoreq_openapi::*;

fn split_entry() -> Option<PathBuf> {
    let dir = std::env::var_os("NEKOREQ_FIXTURE_DIR")?;
    let p = PathBuf::from(dir).join("openapi.yaml");
    p.exists().then_some(p)
}

fn bundled_entry() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("NEKOREQ_FIXTURE_BUNDLED")?);
    p.exists().then_some(p)
}

macro_rules! need {
    ($e:expr, $name:literal) => {
        match $e {
            Some(p) => p,
            None => {
                eprintln!("{} が無いのでスキップ", $name);
                return;
            }
        }
    };
}

fn ids(l: &Loaded) -> BTreeSet<String> {
    l.operations.iter().map(|o| o.op_key.to_string()).collect()
}

fn tags(l: &Loaded) -> BTreeSet<String> {
    l.operations
        .iter()
        .flat_map(|o| o.tags.iter().cloned())
        .collect()
}

/// 数値を f64 にそろえた JSON にする（`25.0` と `25` を同じ値として比べるため）。
fn norm<T: serde::Serialize>(v: &T) -> serde_json::Value {
    fn walk(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Number(n) => serde_json::json!(n.as_f64()),
            serde_json::Value::Array(a) => a.into_iter().map(walk).collect(),
            serde_json::Value::Object(o) => o.into_iter().map(|(k, v)| (k, walk(v))).collect(),
            x => x,
        }
    }
    walk(serde_json::to_value(v).unwrap())
}

/// 分割版と bundle 版のどちらから読んでも、同じ件数の Operation が同じ中身で並ぶこと（M3 の完了条件）。
#[tokio::test]
async fn split_and_bundled_have_the_same_operations() {
    let split = need!(split_entry(), "NEKOREQ_FIXTURE_DIR");
    let bundled = need!(bundled_entry(), "NEKOREQ_FIXTURE_BUNDLED");

    let t = Instant::now();
    let a = loaded(&split).await;
    let split_time = t.elapsed();
    let t = Instant::now();
    let b = loaded(&bundled).await;
    let bundled_time = t.elapsed();
    eprintln!(
        "分割: {} 件・{} ファイル・{:?}（読み込み {} ms） / bundle: {} 件・{} ファイル・{:?}（{} ms）",
        a.operations.len(),
        a.files.len(),
        split_time,
        a.elapsed_ms,
        b.operations.len(),
        b.files.len(),
        bundled_time,
        b.elapsed_ms
    );
    // M3 の完了条件: 分割版と bundle 版のどちらから読んでも、同じ件数・同じ API・同じ中身になる。
    assert_eq!(a.operations.len(), b.operations.len());
    assert_eq!(ids(&a), ids(&b), "operationId の集合が一致する");
    assert_eq!(tags(&a), tags(&b), "タグの集合が一致する");
    assert!(a.operations.iter().all(|o| o.operation_id.is_some()));
    for oa in &a.operations {
        let ob = op(&b, &oa.op_key.to_string());
        assert_eq!(oa.method, ob.method, "{}", oa.op_key);
        assert_eq!(oa.path, ob.path, "{}", oa.op_key);
        assert_eq!(oa.tags, ob.tags, "{}", oa.op_key);
        assert_eq!(oa.summary, ob.summary, "{}", oa.op_key);
        assert_eq!(oa.description, ob.description, "{}", oa.op_key);
        assert_eq!(oa.security, ob.security, "{}", oa.op_key);
        // bundler（redocly）は `example: 25.00` を `25` に書き換えるので、数値は値として比べる
        for (what, x, y) in [
            ("parameters", norm(&oa.parameters), norm(&ob.parameters)),
            (
                "requestBody",
                norm(&oa.request_body),
                norm(&ob.request_body),
            ),
            ("responses", norm(&oa.responses), norm(&ob.responses)),
        ] {
            assert_eq!(x, y, "{} の {what}", oa.op_key);
        }
    }
    assert_eq!(a.meta.title, b.meta.title);
    assert_eq!(a.meta.version, b.meta.version);
    assert_eq!(a.meta.servers, b.meta.servers);
    assert_eq!(a.meta.security_schemes, b.meta.security_schemes);
    assert_eq!(a.meta.root_security, b.meta.root_security);
    assert!(
        a.inspection.unresolved_refs.is_empty(),
        "{:#?}",
        a.inspection.unresolved_refs
    );
    assert!(b.inspection.unresolved_refs.is_empty());
    assert!(a.inspection.duplicate_operation_ids.is_empty());
    assert_eq!(b.files.len(), 1);
    assert_eq!(b.reference, ReferenceStyle::Bundled);
    assert_eq!(a.reference, ReferenceStyle::Split);
}

#[tokio::test]
async fn split_import_takes_under_two_seconds() {
    let split = need!(split_entry(), "NEKOREQ_FIXTURE_DIR");
    // 1 回目はファイルのキャッシュが冷えていることがあるので、数回測って最短を見る
    let mut best = std::time::Duration::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let l = loaded(&split).await;
        best = best.min(t.elapsed());
        assert_eq!(l.operations.len(), 86);
    }
    eprintln!(
        "分割 25 ファイル（読んだのは {} 個）の取り込み: 最短 {best:?}",
        loaded(&split).await.files.len()
    );
    assert!(best < std::time::Duration::from_secs(2), "{best:?}");
}

#[tokio::test]
async fn get_client_list_is_found_with_environment_source() {
    let split = need!(split_entry(), "NEKOREQ_FIXTURE_DIR");
    let l = loaded(&split).await;
    let o = op(&l, "getClientList");
    assert_eq!(o.method, "GET");
    assert_eq!(o.path, "/api/v1/video/clients");
    assert_eq!(o.summary.as_deref(), Some("取引先一覧取得"));
    assert_eq!(o.tags, vec!["video-client"]);
    assert_eq!(o.source_file.as_deref(), Some("video/client.yaml"));
    assert_eq!(l.meta.servers.len(), 1);
    assert_eq!(l.meta.servers[0].url, "http://localhost:8080");
    assert_eq!(
        l.meta.servers[0].description.as_deref(),
        Some("ローカル (Dev Container)")
    );
    assert_eq!(
        l.meta.title,
        "日本アニメーション ライセンス契約管理システム API"
    );
    assert_eq!(l.openapi_version, "3.0.3");
    let q: Vec<_> = o.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        q,
        vec![
            "q",
            "contractType",
            "status",
            "contractState",
            "page",
            "perPage"
        ]
    );
    // コンテナの $ref 先（../common.yaml）の enum まで展開されている
    let ct = o.parameters[1].schema.as_ref().unwrap();
    assert_eq!(ct["x-nekoreq-ref"], "ContractType");
    assert!(ct["enum"].is_array());
}

#[tokio::test]
async fn six_operations_have_empty_security_and_tables_are_counted() {
    let split = need!(split_entry(), "NEKOREQ_FIXTURE_DIR");
    let l = loaded(&split).await;
    assert_eq!(l.inspection.no_security_count, 6);
    let none: Vec<_> = l
        .operations
        .iter()
        .filter(|o| matches!(&o.security, Some(s) if s.is_empty()))
        .map(|o| o.op_key.to_string())
        .collect();
    assert_eq!(none.len(), 6, "{none:?}");
    assert!(none.contains(&"createPasswordLogin".to_string()));
    eprintln!("security: [] の 6 件: {none:?}");
    eprintln!(
        "Markdown の表を含む description: {} 件",
        l.inspection.markdown_table_count
    );
    assert!(l.inspection.markdown_table_count > 0);
    let preview = build_preview(&l, None);
    eprintln!(
        "タグ {} 個、既定の認証 {:?}、ファイル {}",
        preview.tags.len(),
        preview.default_auth,
        preview.file_count
    );
    assert_eq!(preview.default_auth, vec!["cookieAuth（Cookie: SESSION）"]);
}

#[tokio::test]
async fn line_numbers_match_the_real_files() {
    let split = need!(split_entry(), "NEKOREQ_FIXTURE_DIR");
    let dir = split.parent().unwrap().to_path_buf();
    let l = loaded(&split).await;
    let mut per_file: BTreeMap<String, usize> = BTreeMap::new();
    for o in &l.operations {
        let file = dir.join(o.source_file.as_ref().unwrap());
        let line = o.source_line.unwrap();
        let text = line_text(&file, line);
        assert_eq!(
            text.trim(),
            format!("{}:", o.method.to_lowercase()),
            "{} -> {}:{line}",
            o.op_key,
            file.display()
        );
        // その上のどこかに、path のキーが書かれている
        let all = std::fs::read_to_string(&file).unwrap();
        let before: Vec<&str> = all.lines().take(line as usize - 1).collect();
        assert!(
            before
                .iter()
                .rev()
                .any(|l| l.trim() == format!("{}:", o.path)),
            "{} の path が {} の手前にない",
            o.op_key,
            file.display()
        );
        *per_file.entry(o.source_file.clone().unwrap()).or_default() += 1;
    }
    eprintln!("ファイルごとの Operation 数: {per_file:?}");
    // 設計書の例: video/client.yaml:5（getClientList は 5 行目の次あたり）
    let o = op(&l, "getClientList");
    assert_eq!(
        line_text(&dir.join("video/client.yaml"), o.source_line.unwrap()).trim(),
        "get:"
    );
    eprintln!(
        "getClientList: {}:{}",
        o.source_file.as_deref().unwrap(),
        o.source_line.unwrap()
    );
    // bundle 版も同じ確認
    if let Some(b) = bundled_entry() {
        let lb = loaded(&b).await;
        for o in &lb.operations {
            let text = line_text(&b, o.source_line.unwrap());
            assert_eq!(
                text.trim(),
                format!("{}:", o.method.to_lowercase()),
                "{}",
                o.op_key
            );
        }
        let o = op(&lb, "createPasswordLogin");
        eprintln!(
            "bundle 版 createPasswordLogin: {}:{}",
            o.source_file.as_deref().unwrap(),
            o.source_line.unwrap()
        );
    }
}

/// roas（別のパーサー）で bundle 版を読んだ operationId・method・path の集合と、こちらの結果を突き合わせる。
#[tokio::test]
async fn bundled_matches_what_roas_reads() {
    let bundled = need!(bundled_entry(), "NEKOREQ_FIXTURE_BUNDLED");
    use roas::validation::{Options, Validate};
    let text = std::fs::read_to_string(&bundled).unwrap();
    let value = pos::parse(&text).unwrap().value;
    let spec: roas::v3_0::spec::Spec = serde_json::from_value(value).expect("roas が読める");
    spec.validate(
        Options::IgnoreUnusedTags | Options::IgnoreUnusedSchemas,
        None,
    )
    .expect("roas の検証を通る");
    let mut theirs = BTreeSet::new();
    for (path, item) in &spec.paths.paths {
        for (method, op) in item.operations.iter().flatten() {
            theirs.insert((
                op.operation_id.clone().unwrap_or_default(),
                method.to_string().to_uppercase(),
                path.clone(),
            ));
        }
    }
    let l = loaded(&bundled).await;
    let ours: BTreeSet<_> = l
        .operations
        .iter()
        .map(|o| {
            (
                o.operation_id.clone().unwrap_or_default(),
                o.method.clone(),
                o.path.clone(),
            )
        })
        .collect();
    assert_eq!(theirs, ours);
}

/// 展開ノード数の上限（Operation あたり）に、実際の API 定義が当たらないこと。
#[tokio::test]
async fn real_operations_do_not_hit_the_expansion_limit() {
    for (name, entry) in [("分割", split_entry()), ("bundle", bundled_entry())] {
        let Some(entry) = entry else {
            eprintln!("{name} の fixture が無いのでスキップ");
            continue;
        };
        let t = Instant::now();
        let l = loaded(&entry).await;
        let elapsed = t.elapsed();
        let hit: Vec<String> = l
            .operations
            .iter()
            .filter(|o| {
                serde_json::to_string(o)
                    .unwrap()
                    .contains("x-nekoreq-truncated")
            })
            .map(|o| o.op_key.to_string())
            .collect();
        eprintln!(
            "{name}: {} 件・{elapsed:?}・打ち切り {}",
            l.operations.len(),
            hit.len()
        );
        assert!(hit.is_empty(), "上限に当たった: {hit:?}");
    }
}
