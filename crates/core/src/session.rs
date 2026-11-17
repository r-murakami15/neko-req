//! タブの復元（起動時に前回のタブを開き直す）。プロジェクトごとに保存・読み込みする。
//!
//! - 保存するのは、開いているタブ（yaml の API・手で作ったリクエスト・まだ保存していない新規）と、タブごとの
//!   未保存の入力、選んでいるタブ。画面の状態（開いているパネルなど）は画面が決める文字列（`view_state`）で持つ。
//! - 未保存の入力のうち、手で書かれたシークレット（認証欄・Authorization / Cookie / X-XSRF-TOKEN ヘッダー・
//!   Cookie の値）は**保存しない**（空にして `dropped_fields` に名前を残す）。`{{変数}}` で参照したものは残る。
//!   変数の上書き（`VarOverride`）も保存しない。

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::build::RequestSpec;
use crate::operation::OpKey;

/// 1 つのタブが何を開いているか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SessionTabKind {
    /// yaml の API（`op_key`）。
    Operation,
    /// 保存済みの手で作ったリクエスト（`adhoc_id`）。
    Adhoc,
    /// まだ保存していない新規のリクエスト（`request` が内容）。
    New,
}

/// 開いているタブ 1 つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionTab {
    /// 画面が付けるタブの ID。
    pub id: String,
    pub kind: SessionTabKind,
    pub op_key: Option<OpKey>,
    pub adhoc_id: Option<String>,
    /// タブの見出し。
    pub title: String,
    /// 未保存の入力（保存済みの内容から変えていないタブは `None`）。`New` はいつも持つ。
    pub request: Option<RequestSpec>,
    /// 保存していない変更がある。
    pub dirty: bool,
    /// 画面の状態（選んでいるパネルなど。画面が決める文字列。Rust は中身を見ない）。
    pub view_state: Option<String>,
    /// 保存のときに空にした手書きのシークレット（「Authorization ヘッダー」など）。画面は「認証の値は復元されません」と出す。
    #[serde(default)]
    pub dropped_fields: Vec<String>,
}

/// 開いているタブの一覧（プロジェクトごと）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionState {
    pub tabs: Vec<SessionTab>,
    /// 選んでいるタブの `id`。
    pub active_tab_id: Option<String>,
    /// 保存した日時（UTC の RFC 3339。保存のとき Rust が入れる）。
    pub saved_at: Option<String>,
}

/// 保存できるタブの数の上限。
pub const MAX_SESSION_TABS: usize = 200;
