use serde::{Deserialize, Serialize};
use specta::Type;

use crate::spec::{ImportOptions, SpecMeta};

/// プロジェクト。取り込み元の `ApiSource`（まだ取り込んでいなければ `None`）と既定の環境を持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub source: Option<ApiSource>,
    /// 既定の環境の ID（`Environment.id`）。
    pub default_environment_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// OpenAPI の取り込み方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    /// ローカルのパスを指定（参照方式）。同期のたびに読み直す。
    Path,
    /// URL を指定（参照方式）。
    Url,
    /// ファイルをアップロードしてアプリのデータフォルダにコピー（コピー方式）。
    Copy,
}

/// 取り込んだ yaml の一覧に載る 1 ファイル。更新日時とハッシュは同期時の変更検出に使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    /// 入口ファイルからの相対パス、または URL。
    pub path: String,
    /// ファイルの更新日時（RFC 3339）。URL 方式など取れないときは `None`。
    pub modified_at: Option<String>,
    /// 内容のハッシュ（16 進）。
    pub hash: String,
}

/// 取り込み元（yaml）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApiSource {
    pub id: String,
    pub kind: SourceKind,
    /// 入口ファイルのパス（コピー方式ではコピー先の相対パス）または URL。
    pub entry: String,
    /// 入口と参照先の全ファイル。
    pub files: Vec<SourceFile>,
    /// OpenAPI の版（例: `3.0.3`）。
    pub openapi_version: String,
    /// 最後に同期した日時（RFC 3339）。
    pub synced_at: Option<String>,
    /// URL 方式のアクセストークンをキーチェーンに置いているか（値は返さない）。
    #[serde(default)]
    pub has_token: bool,
    /// URL 方式で、入口と別のホストの参照を読んでよいと確認したホスト（`host` または `host:port`）。
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// 取り込みのときに選んだ変換オプション。
    #[serde(default)]
    pub options: ImportOptions,
    /// yaml のタイトル・版・servers・認証の定義など（入口ファイルから読んだもの）。
    #[serde(default)]
    pub meta: SpecMeta,
}
