use serde::{Deserialize, Serialize};
use specta::Type;

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

/// 履歴 1 件。保存する前にシークレットは `••••` に置き換え済みであること。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub project_id: String,
    /// 送信した日時（RFC 3339）。
    pub sent_at: String,
    pub environment_id: Option<String>,
    /// 環境を後から消しても履歴で分かるように名前も持つ。
    pub environment_name: Option<String>,
    pub op_key: Option<OpKey>,
    pub method: String,
    /// 変数を展開した後の URL（伏せ字済み）。
    pub url: String,
    pub request_headers: Vec<HeaderPair>,
    pub request_body: Option<String>,
    /// 通信エラーなどで応答がなかったときは `None`。
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub response_headers: Vec<HeaderPair>,
    pub response_body: Option<String>,
    /// 1 MB を超えて先頭だけ保存したとき `true`。
    pub response_body_truncated: bool,
    /// 受け取ったボディの全体のバイト数。
    pub response_size: u32,
    pub timing: Option<Timing>,
    /// 送信に失敗したときのメッセージ。
    pub error: Option<String>,
}
