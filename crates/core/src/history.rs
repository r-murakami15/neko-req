//! 履歴の型。保存する前にシークレットは `••••` に置き換え済みであること（`redact`・`sanitize`）。
//!
//! - `HistoryEntry`: 1 件の全部（`history_get`）。送った内容（展開後）・受け取った内容・時間・環境・
//!   Operation のキーと、再送信・編集用の元の入力（`input`。`{{変数}}` のままの `RequestSpec`）。
//! - `HistoryListItem`: 一覧用の軽い項目（`history_list`。ボディ・ヘッダーは持たない）。
//! - 一覧の絞り込み（`HistoryQuery`）と、2 件の比較（`HistoryDiff`）。

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::build::RequestSpec;
use crate::operation::OpKey;

/// ヘッダー 1 件（同じ名前が複数あっても順番どおりに残す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HeaderPair {
    pub name: String,
    pub value: String,
}

/// 時間の内訳（ミリ秒）。取れなかった区間は `None`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub dns_ms: Option<f64>,
    /// TCP 接続。
    pub connect_ms: Option<f64>,
    pub tls_ms: Option<f64>,
    /// リクエスト送信。今は測っていないので常に `None`（送信の時間は応答待ちに含まれる）。
    pub send_ms: Option<f64>,
    /// 応答待ち（最初のバイトまで）。
    pub wait_ms: Option<f64>,
    pub download_ms: Option<f64>,
    pub total_ms: f64,
}

/// 送ったボディの種類。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum HistoryBodyKind {
    #[default]
    None,
    /// テキスト（`request_body` に入っている）。
    Text,
    /// テキストではないバイト列（中身は保存しない。大きさだけ）。
    Binary,
    /// ファイルをそのまま送った（パスを `request_file` に持つ。中身は保存しない）。
    File,
    /// multipart（パートは `request_parts`）。
    Multipart,
}

/// multipart の 1 パート（履歴用。値は伏せ字済み）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPart {
    pub name: String,
    /// ファイルのパートか（`value` はパス）。
    pub is_file: bool,
    /// テキストの値、またはファイルのパス。
    pub value: String,
    pub content_type: Option<String>,
}

/// 履歴 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub project_id: String,
    /// 送信した日時（UTC の RFC 3339）。
    pub sent_at: String,
    pub environment_id: Option<String>,
    /// 環境を後から消しても履歴で分かるように名前も持つ。
    pub environment_name: Option<String>,
    pub op_key: Option<OpKey>,
    /// Operation の summary（一覧の見出し。手で作ったリクエストは `None`）。
    pub op_summary: Option<String>,
    pub method: String,
    /// 変数を展開した後の URL（伏せ字済み）。
    pub url: String,
    /// リダイレクトを追ったあとの最終的な URL（伏せ字済み。追わなかった・失敗したときは `None`）。
    pub final_url: Option<String>,
    /// 実際に送ったヘッダー（アプリが付けた User-Agent などを含む。伏せ字済み）。
    pub request_headers: Vec<HeaderPair>,
    /// 送ったボディのテキスト（伏せ字済み）。multipart は、テキストのパートを `name=value` の行にしたもの
    /// （検索と簡単な表示用）。
    pub request_body: Option<String>,
    pub request_body_kind: HistoryBodyKind,
    /// 送ったボディの大きさ（バイト）。
    pub request_body_size: u32,
    /// 上限（1 MB）を超えて先頭だけ保存したとき `true`。
    pub request_body_truncated: bool,
    pub request_parts: Vec<HistoryPart>,
    /// ボディをファイルから送ったときのパス。
    pub request_file: Option<String>,
    /// 通信エラーなどで応答がなかったときは `None`。
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub response_headers: Vec<HeaderPair>,
    pub response_content_type: Option<String>,
    /// 受け取ったボディのテキスト（伏せ字済み）。バイナリは保存しない（`response_binary`）。
    pub response_body: Option<String>,
    /// ボディがバイナリだったので保存していない。
    pub response_binary: bool,
    /// 1 MB を超えて先頭だけ保存したとき `true`（画面の 10 MB の上限で切れていたときも `true`）。
    pub response_body_truncated: bool,
    /// 受け取ったボディの全体のバイト数。
    pub response_size: u32,
    pub timing: Option<Timing>,
    /// 送信に失敗したときのメッセージ（伏せ字済み）。
    pub error: Option<String>,
    /// 失敗の種類（`connect`・`tls`・`timeout`・`cancelled`・`tooLarge`・`other`）。
    pub error_kind: Option<String>,
    /// 送信に使った元の入力（`{{変数}}` のまま）。再送信・編集して開く・入力値として保存に使う。
    /// 手で書いた認証の値などは `••••` に置き換えてある（`masked_fields`）。
    pub input: Option<RequestSpec>,
    /// `input` で伏せたもの（「Authorization ヘッダー」「認証（Bearer のトークン）」など）。空なら伏せていない。
    pub masked_fields: Vec<String>,
    /// 送信のときに指定されていた変数の上書きの名前（値は保存しない。再送信では使えない）。
    pub override_names: Vec<String>,
}

/// 一覧用の軽い項目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryListItem {
    pub id: String,
    pub sent_at: String,
    /// ローカルの日付（`2026-11-17`）。画面の日付ごとの区切りに使う。
    pub local_date: String,
    pub method: String,
    /// 展開後の URL（伏せ字済み）。
    pub url: String,
    pub status: Option<u16>,
    pub status_text: Option<String>,
    /// 合計時間（ミリ秒）。失敗で測れなかったときは `None`。
    pub total_ms: Option<f64>,
    pub environment_id: Option<String>,
    pub environment_name: Option<String>,
    pub op_key: Option<OpKey>,
    pub op_summary: Option<String>,
    /// 失敗（応答がなかった）。
    pub failed: bool,
    pub error_kind: Option<String>,
    pub response_size: u32,
}

/// ステータスの絞り込み。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum StatusClass {
    #[serde(rename = "2xx")]
    S2xx,
    #[serde(rename = "3xx")]
    S3xx,
    #[serde(rename = "4xx")]
    S4xx,
    #[serde(rename = "5xx")]
    S5xx,
    /// 応答がなかった（接続できない・タイムアウト・中止など）。
    #[serde(rename = "failed")]
    Failed,
}

/// 一覧の絞り込みとページング。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryQuery {
    /// URL とボディの全文検索（3 文字以上は FTS5 の trigram、それ未満は部分一致）。空なら絞らない。
    #[serde(default)]
    pub text: Option<String>,
    /// 空なら絞らない。複数あれば、どれかに当てはまるもの。
    #[serde(default)]
    pub statuses: Vec<StatusClass>,
    #[serde(default)]
    pub environment_id: Option<String>,
    /// 期間の始め（ローカルの日付 `YYYY-MM-DD`。その日を含む）。
    #[serde(default)]
    pub from: Option<String>,
    /// 期間の終わり（ローカルの日付。その日を含む）。
    #[serde(default)]
    pub to: Option<String>,
    /// 日付の区切りに使う UTC との差（分。日本は 540）。画面が `-new Date().getTimezoneOffset()` で渡す。
    /// 指定がなければ OS のタイムゾーン。
    #[serde(default)]
    pub utc_offset_minutes: Option<i32>,
    /// 1 回に返す件数（既定 100、最大 500）。
    #[serde(default)]
    pub limit: Option<u32>,
    /// 前のページの `nextCursor`。
    #[serde(default)]
    pub cursor: Option<String>,
}

/// 一覧 1 ページ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub items: Vec<HistoryListItem>,
    /// 絞り込みに当てはまる全件数（ページに関係なく）。
    pub total: u32,
    /// 続きがあれば、次のページを取るためのカーソル。
    pub next_cursor: Option<String>,
}

/// 差分の 1 行の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DiffTag {
    Equal,
    /// 左にだけある行。
    Delete,
    /// 右にだけある行。
    Insert,
    /// 変わらない行が続くので省いた（`text` に「N 行省略」）。
    Skip,
}

/// 差分の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub tag: DiffTag,
    pub text: String,
    /// 左の行番号（1 始まり）。
    pub left_line: Option<u32>,
    /// 右の行番号（1 始まり）。
    pub right_line: Option<u32>,
}

/// 2 件の比較。左右は `history_compare` に渡した順。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDiff {
    pub left: HistoryListItem,
    pub right: HistoryListItem,
    /// ステータス（`200 OK` / `失敗: …`）。
    pub left_status: String,
    pub right_status: String,
    pub status_changed: bool,
    /// ヘッダー（名前順にそろえた `name: value` の行の差分）。
    pub headers: Vec<DiffLine>,
    pub headers_changed: bool,
    /// ボディの行の差分。JSON は整形してから比べる。バイナリは比べない（`body_note`）。
    pub body: Vec<DiffLine>,
    pub body_changed: bool,
    /// ボディについての注意（「JSON として整形して比べました」「バイナリのため比べません」「長いため先頭だけ比べました」）。
    pub body_note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_class_wire_names() {
        assert_eq!(
            serde_json::to_string(&StatusClass::S2xx).unwrap(),
            "\"2xx\""
        );
        assert_eq!(
            serde_json::to_string(&StatusClass::Failed).unwrap(),
            "\"failed\""
        );
        let q: HistoryQuery = serde_json::from_str(r#"{"statuses":["4xx","5xx"]}"#).unwrap();
        assert_eq!(q.statuses, vec![StatusClass::S4xx, StatusClass::S5xx]);
        assert_eq!(q.text, None);
    }
}
