//! nekoreq-openapi: OpenAPI の yaml の読み込み（パス・URL・アップロード）、分割ファイルをまたぐ $ref の解決、
//! Operation への変換、同期時の差分、行番号。
//!
//! 流れ:
//! 1. `load`: 入口から `$ref` をたどって参照先のファイルをすべて読む（`load::crawl`）。位置つきの yaml パーサー
//!    （saphyr）で `Value` と「JSON Pointer → 行番号」の表を作る（`pos`）。足りないファイル・壊れた yaml は一覧にして止める。
//! 2. `convert`: paths を Operation にする（`schema::Inliner` が `$ref` を展開。循環は 2 回目で止める）。
//! 3. `diff`: 前回の Operation と比べて追加・変更・削除を出す。
//! 4. `view`: API ツリー・設計タブ・送信タブを開くときの初期状態を作る（`schema` がスキーマ表とモックを作る）。
//!
//! 保存（SQLite）は知らない。保存は storage クレート、組み合わせは app（src-tauri）。
//!
//! roas について: roas（OpenAPI 3.0 / 3.1 / 3.2 の型とローダー）は検証したうえで、本体の取り込みには使わない
//! （理由は docs/02-implementation-notes.md）。テストでは bundle 済みの fixture を roas で読み、こちらの結果と
//! 突き合わせる（dev-dependency）。

pub mod check;
pub mod convert;
pub mod diff;
pub mod doc;
pub mod load;
pub mod model;
pub mod pointer;
pub mod pos;
pub mod schema;
pub mod text;
pub mod upload;
pub mod view;

use std::time::Instant;

use nekoreq_core::{OpKey, Operation, SourceFile, SpecMeta};

pub use check::source_changed;
pub use diff::diff_operations;
pub use load::{LoadEntry, LoadParams};
pub use model::*;
pub use view::{default_auth, describe_operation, list_item, open_operation};

/// 読み込みと変換が終わったもの。
#[derive(Debug, Clone)]
pub struct Loaded {
    pub meta: SpecMeta,
    pub openapi_version: String,
    /// `id` と `project_id` は空（保存するときに付ける）。
    pub operations: Vec<Operation>,
    /// 読んだファイル（入口が先頭）。
    pub files: Vec<SourceFile>,
    pub inspection: Inspection,
    /// 入口ファイル（入口からの相対表示）。
    pub entry: String,
    pub reference: ReferenceStyle,
    pub total_bytes: u32,
    pub elapsed_ms: u32,
}

pub enum LoadResult {
    Loaded(Box<Loaded>),
    /// 入口と別のホストを指す参照がある（読んでいない。確認したら `allowed_hosts` に足して読み直す）。
    NeedsConfirmation(Vec<ForeignRef>),
    /// 足りないファイル・壊れた yaml など。取り込み（同期）はできない。
    Blocked(Vec<LoadProblem>),
}

/// 入口から参照先まで読み、Operation に変換する。
pub async fn load(params: &LoadParams) -> LoadResult {
    let started = Instant::now();
    match load::crawl(params).await {
        load::Crawl::NeedsConfirmation(f) => LoadResult::NeedsConfirmation(f),
        load::Crawl::Blocked(p) => LoadResult::Blocked(p),
        load::Crawl::Done(docs) => {
            let converted = convert::convert(&docs);
            let files: Vec<SourceFile> = docs
                .files
                .iter()
                .map(|f| SourceFile {
                    path: f.display.clone(),
                    modified_at: f.modified_at.clone(),
                    hash: f.hash.clone(),
                })
                .collect();
            let total: usize = docs.files.iter().map(|f| f.bytes).sum();
            LoadResult::Loaded(Box::new(Loaded {
                meta: converted.meta,
                openapi_version: converted.openapi_version,
                operations: converted.operations,
                reference: if files.len() > 1 {
                    ReferenceStyle::Split
                } else {
                    ReferenceStyle::Bundled
                },
                entry: docs.entry().display.clone(),
                files,
                inspection: converted.inspection,
                total_bytes: u32::try_from(total).unwrap_or(u32::MAX),
                elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
            }))
        }
    }
}

/// 確認画面のデータ。`diff` は既存プロジェクトへの統合のときの差分。
pub fn build_preview(l: &Loaded, diff: Option<SyncDiff>) -> ImportPreview {
    let mut groups: Vec<PreviewTag> = Vec::new();
    // 入口の `tags` の順を先にし、そこにないタグは出てきた順、タグのないものは最後
    for t in &l.meta.tags {
        groups.push(PreviewTag {
            name: Some(t.name.clone()),
            description: t.description.clone(),
            operations: vec![],
        });
    }
    let mut untagged: Vec<PreviewOperation> = Vec::new();
    for op in &l.operations {
        let item = PreviewOperation {
            op_key: op.op_key.clone(),
            operation_id: op.operation_id.clone(),
            method: op.method.clone(),
            path: op.path.clone(),
            summary: op.summary.clone(),
            security: view::security_badge(op),
            source_file: op.source_file.clone(),
            source_line: op.source_line,
        };
        match op.tags.first() {
            Some(tag) => match groups.iter_mut().find(|g| g.name.as_deref() == Some(tag)) {
                Some(g) => g.operations.push(item),
                None => groups.push(PreviewTag {
                    name: Some(tag.clone()),
                    description: None,
                    operations: vec![item],
                }),
            },
            None => untagged.push(item),
        }
    }
    groups.retain(|g| !g.operations.is_empty());
    if !untagged.is_empty() {
        groups.push(PreviewTag {
            name: None,
            description: None,
            operations: untagged,
        });
    }
    let default_auth = l
        .meta
        .root_security
        .iter()
        .flat_map(|r| r.schemes.iter())
        .map(|s| {
            l.meta
                .security_schemes
                .iter()
                .find(|i| i.name == s.name)
                .map(nekoreq_core::SecuritySchemeInfo::label)
                .unwrap_or_else(|| s.name.clone())
        })
        .collect();
    ImportPreview {
        title: l.meta.title.clone(),
        version: l.meta.version.clone(),
        openapi_version: l.openapi_version.clone(),
        description: l.meta.description.clone(),
        servers: l.meta.servers.clone(),
        default_auth,
        reference: l.reference,
        entry: l.entry.clone(),
        file_count: u32::try_from(l.files.len()).unwrap_or(u32::MAX),
        files: l.files.iter().map(|f| f.path.clone()).collect(),
        total_bytes: l.total_bytes,
        operation_count: u32::try_from(l.operations.len()).unwrap_or(u32::MAX),
        tags: groups,
        inspection: l.inspection.clone(),
        diff,
        elapsed_ms: l.elapsed_ms,
    }
}

/// `op_key` を探す（テスト・画面側の補助）。
pub fn find_operation<'a>(ops: &'a [Operation], key: &OpKey) -> Option<&'a Operation> {
    ops.iter().find(|o| &o.op_key == key)
}
