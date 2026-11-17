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

/// 分割版と bundle 版で、共通の Operation の中身（parameters・requestBody・responses など）が一致すること。
///
/// 手元の fixture は、bundle 版（openapi.bundled.yaml）が分割版より古く、分割版にない 2 件
/// （createFileUploadUrl・getFileDownloadUrl）が bundle 版にだけ残っている（分割版は 86 件、bundle 版は 88 件）。
/// そのため「同じ 88 件」ではなく、「分割版の 86 件がすべて bundle 版にあり、内容も同じ。差の 2 件は分割版の
/// どのファイルにも書かれていない」ことを確かめる。
#[tokio::test]
async fn split_and_bundled_agree_on_every_common_operation() {
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
    assert_eq!(a.operations.len(), 86);
    assert_eq!(b.operations.len(), 88);
    assert!(
        ids(&a).is_subset(&ids(&b)),
        "分割版の operationId はすべて bundle 版にある"
    );
    let extra: Vec<String> = ids(&b).difference(&ids(&a)).cloned().collect();
    assert_eq!(extra, vec!["createFileUploadUrl", "getFileDownloadUrl"]);
    // 差の 2 件は、分割版のどのファイルにも書かれていない（読み込みの取りこぼしではない）
    let dir = split.parent().unwrap().to_path_buf();
    let mut all_text = String::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "yaml") {
                all_text.push_str(&std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    for id in &extra {
        assert!(
            !all_text.contains(id.as_str()),
            "{id} が分割版のファイルにある"
        );
    }
    assert_eq!(tags(&a).len(), 18);
    assert!(tags(&a).is_subset(&tags(&b)));
    assert!(a.operations.iter().all(|o| o.operation_id.is_some()));
    // 共通の 86 件: method・path・タグ・summary・parameters・security はすべて一致する。
    // description・requestBody・responses が違うのは 6 件だけ（bundle 版が古いため）。
    let mut stale: Vec<String> = Vec::new();
    for oa in &a.operations {
        let ob = op(&b, &oa.op_key.to_string());
        assert_eq!(oa.method, ob.method);
        assert_eq!(oa.path, ob.path);
        assert_eq!(oa.tags, ob.tags, "{}", oa.op_key);
        assert_eq!(oa.summary, ob.summary, "{}", oa.op_key);
        assert_eq!(oa.parameters, ob.parameters, "{}", oa.op_key);
        assert_eq!(oa.security, ob.security, "{}", oa.op_key);
        if oa.content_hash == ob.content_hash {
            assert_eq!(oa.description, ob.description);
            assert_eq!(oa.request_body, ob.request_body);
            assert_eq!(oa.responses, ob.responses);
        } else {
            stale.push(oa.op_key.to_string());
        }
    }
    eprintln!(
        "bundle 版と内容が違う Operation（{} 件）: {stale:?}",
        stale.len()
    );
    assert_eq!(
        stale,
        vec![
            "createContract",
            "updateContract",
            "getInvoiceList",
            "getCompanyPayoutDetail",
            "createPayoutNotice",
            "getWriterPayoutDetail"
        ]
    );
    // その 6 件が書かれた分割版のファイルは、bundle 版より新しい（bundle 版が古い証拠）
    let bundle_mtime = std::fs::metadata(&bundled).unwrap().modified().unwrap();
    for key in &stale {
        let o = op(&a, key);
        let f = dir.join(o.source_file.as_ref().unwrap());
        let m = std::fs::metadata(&f).unwrap().modified().unwrap();
        assert!(
            m > bundle_mtime,
            "{key}: {} は bundle 版より新しくない",
            f.display()
        );
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
    assert_eq!(theirs.len(), 88);
    assert_eq!(theirs, ours);
}
