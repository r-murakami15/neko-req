//! 画面に渡す型（取り込みの確認・同期の差分・設計タブ・API ツリー）。すべて `specta::Type`（TypeScript に出る）。

use nekoreq_core::{
    ApiKeyIn, NamedExample, OpKey, ParamIn, RequestSpec, SecurityRequirement, ServerInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

// ---- 取り込みを止めるもの ----

/// 問題のある場所（`file:line`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefSite {
    /// 入口からの相対パス（または URL）。
    pub file: String,
    pub line: Option<u32>,
}

/// 取り込み・同期を止める問題。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LoadProblem {
    /// `$ref` の参照先のファイルがない・読めない。足りないファイル名と参照元の一覧。
    #[serde(rename_all = "camelCase")]
    MissingFile {
        file: String,
        reason: String,
        referenced_from: Vec<RefSite>,
    },
    /// yaml が壊れている。
    #[serde(rename_all = "camelCase")]
    YamlError {
        file: String,
        line: Option<u32>,
        column: Option<u32>,
        reason: String,
    },
    /// OpenAPI の文書ではない（`openapi:` がない）。
    #[serde(rename_all = "camelCase")]
    NotOpenApi { file: String, reason: String },
    /// 対応していない版（Swagger 2.0 など）。
    #[serde(rename_all = "camelCase")]
    UnsupportedVersion { file: String, version: String },
    /// 入口が読めない（URL の取得の失敗など）。
    #[serde(rename_all = "camelCase")]
    Unreadable { file: String, reason: String },
}

impl LoadProblem {
    /// 1 行の説明。
    pub fn describe(&self) -> String {
        match self {
            LoadProblem::MissingFile {
                file,
                reason,
                referenced_from,
            } => {
                let from = referenced_from
                    .iter()
                    .take(3)
                    .map(|s| match s.line {
                        Some(l) => format!("{}:{l}", s.file),
                        None => s.file.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("、");
                format!("{file} を読めません（{reason}）。参照元: {from}")
            }
            LoadProblem::YamlError {
                file,
                line,
                column,
                reason,
            } => match (line, column) {
                (Some(l), Some(c)) => format!("{file}:{l}:{c} yaml を読めません: {reason}"),
                (Some(l), None) => format!("{file}:{l} yaml を読めません: {reason}"),
                _ => format!("{file} yaml を読めません: {reason}"),
            },
            LoadProblem::NotOpenApi { file, reason } => format!("{file}: {reason}"),
            LoadProblem::UnsupportedVersion { file, version } => {
                format!(
                    "{file}: {version} には対応していません（OpenAPI 3.0 / 3.1 / 3.2 を読めます）"
                )
            }
            LoadProblem::Unreadable { file, reason } => format!("{file} を読めません: {reason}"),
        }
    }
}

/// 入口と別のホストを指す参照（読む前に確認が必要）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ForeignRef {
    /// 参照先の origin（`https://assets.example.com`・`http://127.0.0.1:8080` の形。スキーム・ホスト（小文字）・
    /// 既定でないポート。既定のポートは省く）。確認したら `allowedHosts` に入れて読み直す。
    pub host: String,
    pub url: String,
    pub referenced_from: RefSite,
}

// ---- 検査 ----

/// 解決できなかった `$ref`（ファイルはあるが、指すポインターがない・読めない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedRef {
    pub reference: String,
    pub from: RefSite,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateOperationId {
    pub operation_id: String,
    /// `GET /path` の形。
    pub operations: Vec<String>,
}

/// 確認画面の「検査結果」。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub unresolved_refs: Vec<UnresolvedRef>,
    pub duplicate_operation_ids: Vec<DuplicateOperationId>,
    /// `security: []`（認証なし）の Operation の数。
    pub no_security_count: u32,
    /// description に Markdown の表を含む Operation の数。
    pub markdown_table_count: u32,
}

// ---- 確認画面 ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceStyle {
    /// 外部ファイルの `$ref` がない（bundle 済み）。
    Bundled,
    /// 複数のファイルに分かれている。
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SecurityBadge {
    /// 全体の security に従う。
    Default,
    /// `security: []`（認証なし）。
    None,
    /// その Operation 独自の security。
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOperation {
    pub op_key: OpKey,
    pub operation_id: Option<String>,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub security: SecurityBadge,
    pub source_file: Option<String>,
    pub source_line: Option<u32>,
}

/// タグごとの一覧（確認画面のチェックリスト）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTag {
    /// タグのない Operation は `None`。
    pub name: Option<String>,
    pub description: Option<String>,
    pub operations: Vec<PreviewOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub title: String,
    pub version: String,
    pub openapi_version: String,
    pub description: Option<String>,
    pub servers: Vec<ServerInfo>,
    /// 既定の認証（ルートの security）の説明。例: `cookieAuth（Cookie: SESSION）`。なければ空。
    pub default_auth: Vec<String>,
    pub reference: ReferenceStyle,
    /// 入口ファイル（入口からの相対パス。通常はファイル名）。
    pub entry: String,
    /// 読んだファイルの数と、入口からの相対パス。
    pub file_count: u32,
    pub files: Vec<String>,
    pub total_bytes: u32,
    pub operation_count: u32,
    pub tags: Vec<PreviewTag>,
    pub inspection: Inspection,
    /// 既存プロジェクトへの統合のときの差分（新規プロジェクトでは `None`）。
    pub diff: Option<SyncDiff>,
    /// 読み込みにかかった時間（ミリ秒）。
    pub elapsed_ms: u32,
}

// ---- 同期の差分 ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ChangedField {
    Summary,
    Description,
    Parameters,
    RequestBody,
    Responses,
    Security,
    Tags,
    /// operationId の付け替え（`METHOD path` が同じなので入力値は引き継ぐ）。
    OperationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub field: ChangedField,
    /// 変わった中身の短い説明（例: `追加: perPage、削除: q`）。
    pub note: String,
    /// summary・description・operationId のときは前後の値。
    pub before: Option<String>,
    pub after: Option<String>,
}

/// 差分の 1 件の見出し。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiffEntry {
    pub op_key: OpKey,
    pub operation_id: Option<String>,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub tag: Option<String>,
    /// この API の入力値（RequestDraft）があるか。
    pub has_draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChangedEntry {
    pub entry: DiffEntry,
    /// operationId が変わったときの、前の `op_key`。
    pub renamed_from: Option<OpKey>,
    pub changes: Vec<FieldChange>,
    /// yaml から消えたパラメータ（入力値は残り、「yaml にない」と表示して送信から外す）。
    pub removed_params: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncDiff {
    pub added: Vec<DiffEntry>,
    pub changed: Vec<ChangedEntry>,
    /// yaml から消えた API。入力値があれば「削除された API」に移る。
    pub removed: Vec<DiffEntry>,
}

impl SyncDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }
}

// ---- 更新日時の確認 ----

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceChanged {
    pub changed: bool,
    /// 更新日時が変わった（または消えた）ファイル。
    pub changed_files: Vec<String>,
}

// ---- API ツリー ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OperationListItem {
    pub op_key: OpKey,
    pub operation_id: Option<String>,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    /// ツリーのフォルダ（`tags[0]`）。「タグごとにフォルダを作る」がオフ、またはタグがなければ `None`。
    pub folder: Option<String>,
    pub tags: Vec<String>,
    /// 認証が要るか（`security` が `[]` なら false、未指定なら全体の security に従う）。
    pub requires_auth: bool,
    /// yaml から消えた（「削除された API」。入力値が残っている）。
    pub deleted: bool,
    pub has_draft: bool,
}

// ---- 設計タブ ----

/// スキーマ表の 1 行（深さつき。入れ子のオブジェクト・配列は depth を 1 ずつ増やして続く）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SchemaRow {
    pub depth: u32,
    pub name: String,
    /// 画面に出す型。`$ref` の名前があればそれ（例: `ClientStatus`、`ClientListItem[]`）。
    pub type_label: String,
    /// `string`・`integer`・`object`・`array` など（`$ref` の名前ではなく実際の型）。
    pub base_type: String,
    pub required: bool,
    pub nullable: bool,
    /// 制約（`enum: A, B`・`format: date-time`・`min: 1`・`maxLength: 255`・`pattern: ...`・`default: 1` など）。
    pub constraints: Vec<String>,
    pub description: Option<String>,
    /// スキーマに書かれた example（文字列にしたもの）。
    pub example: Option<String>,
    /// `$ref` の名前。
    pub ref_name: Option<String>,
    /// 循環参照で、ここで展開を止めた（画面には「循環」と出す）。
    pub circular: bool,
}

/// 例の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ExampleSource {
    /// yaml の `example` / `examples` に書かれていた。
    Yaml,
    /// スキーマの各項目の example（`example` / `default` / `enum`）から組み立てた。
    SchemaExample,
    /// スキーマの型から自動生成したモック（「スキーマから自動生成したモック」と明記する）。
    Generated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MediaDoc {
    pub content_type: String,
    /// ルートの型の表示（例: `PasswordLoginRequest`、`ClientListItem[]`）。スキーマがなければ `None`。
    pub schema_label: Option<String>,
    pub rows: Vec<SchemaRow>,
    /// 表示する例（yaml の example があればその最初のもの、なければ生成したもの）。
    pub example: Option<Value>,
    pub example_source: Option<ExampleSource>,
    /// yaml に書かれた名前つきの例のすべて（設計タブで切り替えて見る）。なければ空。`example` は先頭の値。
    pub examples: Vec<NamedExample>,
    /// スキーマだけから生成した例（「スキーマから例を生成」ボタン用）。
    pub generated_example: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ParamDoc {
    pub name: String,
    pub location: ParamIn,
    pub required: bool,
    pub type_label: String,
    pub constraints: Vec<String>,
    pub description: Option<String>,
    pub example: Option<String>,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestBodyDoc {
    pub required: bool,
    pub description: Option<String>,
    pub contents: Vec<MediaDoc>,
}

/// description の「返すエラー」の表から読んだ、返す code。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ErrorCodeEntry {
    pub status: String,
    pub code: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResponseDoc {
    pub status: String,
    pub description: Option<String>,
    pub contents: Vec<MediaDoc>,
    /// このステータスで返す code（description の表から）。
    pub error_codes: Vec<ErrorCodeEntry>,
}

/// 設計タブの見出しの「認証」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SecurityDoc {
    pub badge: SecurityBadge,
    /// 認証が要るか。
    pub required: bool,
    /// 適用される security の説明。例: `cookieAuth（Cookie: SESSION）`。認証なしなら空。
    pub labels: Vec<String>,
    /// 適用される security（Operation の値、または全体の値）。
    pub requirements: Vec<SecurityRequirement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OperationDetail {
    pub op_key: OpKey,
    pub operation_id: Option<String>,
    pub method: String,
    pub path: String,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    /// Markdown（GitHub 形式。表を含む）。「description を設計タブのドキュメントにする」がオフなら `None`。
    pub description: Option<String>,
    pub security: SecurityDoc,
    /// 実際に書かれたファイル（入口からの相対パス）と行番号。
    pub source_file: Option<String>,
    pub source_line: Option<u32>,
    /// 最後に取り込んだ（同期した）日時。
    pub imported_at: Option<String>,
    pub parameters: Vec<ParamDoc>,
    pub request_body: Option<RequestBodyDoc>,
    pub responses: Vec<ResponseDoc>,
    /// description の表から読んだ、この API で返す code（全ステータス）。
    pub error_codes: Vec<ErrorCodeEntry>,
    /// yaml から消えた API。
    pub deleted: bool,
}

// ---- 送信タブを開く ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ParamRef {
    pub location: ParamIn,
    pub name: String,
}

/// `operation_open` の戻り値。送信タブを開くための初期状態。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OperationOpen {
    pub op_key: OpKey,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    /// `{{baseUrl}}` + path。`auth` は `None`（= yaml の security に従う既定の認証。送信時に決まる）。
    pub request: RequestSpec,
    /// 入力値（RequestDraft）が保存されているか。なければ example・default から作った初期値。
    pub has_draft: bool,
    /// ボディの初期値に使った Content-Type。
    pub body_content_type: Option<String>,
    /// yaml から消えたパラメータ（値は `request.params` に無効のまま残してあり、送信されない。画面で「yaml にない」と出す）。
    pub missing_in_yaml: Vec<ParamRef>,
    /// yaml の必須パラメータの名前（空なら警告を出す対象）。
    pub required_params: Vec<ParamRef>,
    /// yaml から消えた API。
    pub deleted: bool,
    /// yaml の security から決まる既定の認証（表示用。実際に使うときは変数が展開できるときだけ付く）。
    pub default_auth: Option<DefaultAuth>,
}

/// yaml の security から決まる既定の認証（表示用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum DefaultAuth {
    #[serde(rename_all = "camelCase")]
    ApiKey {
        scheme: String,
        location: ApiKeyIn,
        name: String,
        variable: String,
    },
    #[serde(rename_all = "camelCase")]
    Bearer { scheme: String, variable: String },
    #[serde(rename_all = "camelCase")]
    Basic {
        scheme: String,
        username_variable: String,
        password_variable: String,
    },
}
