//! 履歴。送信のたびに残す（`record`）・一覧・取得・削除・再送信・編集して開く・入力値として保存・cURL としてコピー・比較。
//!
//! 保存するもの（設計書「履歴」）:
//! - 送った内容（展開後の method・URL・ヘッダー・ボディ）、受け取ったステータス・ヘッダー・ボディ、時間の内訳、環境、
//!   Operation のキー、送信日時。
//! - 送信に使った元の入力（`{{変数}}` のままの `RequestSpec` と `op_key`）。再送信・編集して開く・入力値として保存に使う。
//!
//! 伏せ字（保存する前に `••••` にする）:
//! - `Authorization`・`Proxy-Authorization`・`Cookie`・`Set-Cookie`・`X-XSRF-TOKEN` ヘッダーの値。
//! - シークレットの値が現れた箇所（URL・最終 URL・ヘッダー・ボディ・エラー文。送った・受け取った両方）。対象は
//!   `PreparedRequest.secrets`（環境のシークレット・自動付与の SESSION / XSRF-TOKEN・Basic の資格情報）に、
//!   このレスポンスの `Set-Cookie` で受け取った SESSION / XSRF-TOKEN の値と、手で認証欄・ヘッダー・Cookie に書かれた
//!   値（`sanitize_spec` の `literals`）を加えたもの。
//! - 元の入力は、手で書かれた認証の値などを `••••` にする（`masked_fields` に何を伏せたか残す）。
//!   再送信・編集して開く・入力値として保存は、`••••` が入った行・認証を外してから使う。
//! - 変数の上書き（`VarOverride`）は値を保存しない（名前だけ `override_names`）。
//!
//! 上限: 設定の件数（プロジェクトごと。既定 1,000、古いものから消す）。1 MB を超えるボディは先頭だけ（設定でオフ）。
//! バイナリのレスポンスのボディは保存しない。
//! 送信していないもの（変数が展開できない・URL が正しくない・ボディのファイルを読めない）は残さない。

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, TimeZone, Utc};
use nekoreq_core::{
    cookie_changes, drop_masked, drop_partial_secret_tail, redact_headers, redact_text,
    sanitize_spec, AdHocRequest, Body, BodyEncoding, DiffLine, DiffTag, Fill, HeaderPair,
    HistoryBodyKind, HistoryDiff, HistoryEntry, HistoryListItem, HistoryPage, HistoryPart,
    HistorySettings, OpKey, PreparedBody, PreparedPartValue, PreparedRequest, RequestDraft,
    RequestSpec, ResponseData, SendError, SendSettings, DEFAULT_ACCEPT, DEFAULT_ACCEPT_ENCODING,
    DEFAULT_USER_AGENT,
};
use nekoreq_core::{CookieChange, HistoryQuery, KeyValue, ParamIn};
use nekoreq_http::RedirectHop;
use nekoreq_storage::HistoryFilter;
use serde::Serialize;
use specta::Type;

use crate::error::AppError;
use crate::send::{RequestFinished, RequestInput};
use crate::state::AppState;

/// 1 MB を超えるボディは先頭だけ保存する（設定でオフにできる）。
pub const BODY_LIMIT: usize = 1024 * 1024;

/// 元の入力のボディを残さなかったときの、`masked_fields` と `dropped` に入る名前（`ボディ` で始まる）。
const LARGE_INPUT_BODY: &str = "ボディ（1 MB を超えるため保存していません）";

/// 履歴を作るための、送信の文脈。
#[derive(Debug, Clone)]
pub(crate) struct HistoryCtx {
    pub project_id: String,
    pub sent_at: String,
    pub environment: Option<(String, String)>,
    pub op_key: Option<OpKey>,
    pub op_summary: Option<String>,
    /// 送信に使った元の入力（`{{変数}}` のまま）。
    pub input: RequestSpec,
    pub override_names: Vec<String>,
    pub settings: HistorySettings,
}

/// 送信の結果（履歴に残す形）。
pub(crate) enum Outcome<'a> {
    Response(&'a ResponseData),
    Error(&'a SendError),
}

/// 送信の日時（UTC の RFC 3339、ミリ秒）。
pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `limit`（バイト）を超える文字列を、文字の途中で切らずに先頭だけにする。
fn truncate_text(text: String, limit: Option<usize>) -> (String, bool) {
    match limit {
        Some(limit) if text.len() > limit => {
            let mut end = limit;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            (text[..end].to_string(), true)
        }
        _ => (text, false),
    }
}

fn error_kind(e: &SendError) -> &'static str {
    match e {
        SendError::Unresolved { .. } => "unresolved",
        SendError::InvalidRequest { .. } => "invalidRequest",
        SendError::BodyFile { .. } => "bodyFile",
        SendError::Connect { .. } => "connect",
        SendError::Tls { .. } => "tls",
        SendError::Timeout { .. } => "timeout",
        SendError::Cancelled { .. } => "cancelled",
        SendError::TooLarge { .. } => "tooLarge",
        SendError::Other { .. } => "other",
    }
}

/// 履歴 1 件を作る（伏せ字・上限の処理を含む）。`extra_secrets` は、このレスポンスで受け取った SESSION などの値。
pub(crate) fn build_entry(
    ctx: &HistoryCtx,
    prepared: &PreparedRequest,
    outcome: Outcome<'_>,
    extra_secrets: &[String],
) -> HistoryEntry {
    // 入力の手書きの認証を伏せ、その値を伏せ字の対象に加える
    let sanitized = sanitize_spec(&ctx.input, Fill::Mask);
    let mut input = sanitized.spec.clone();
    let mut masked_fields = sanitized.fields.clone();
    // 元の入力のボディも、1 MB を超えるなら残さない（送ったボディの先頭だけでは再送信できず、二重に持つことにもなる）
    if let Some(limit) = ctx.settings.omit_large_bodies.then_some(BODY_LIMIT) {
        if let Body::Json(t) | Body::Text(t) = &input.body {
            if t.len() > limit {
                input.body = Body::None;
                masked_fields.push(LARGE_INPUT_BODY.to_string());
            }
        }
    }
    let mut needles: Vec<String> = prepared.secrets.clone();
    needles.extend(sanitized.literals.iter().cloned());
    needles.extend(extra_secrets.iter().cloned());
    needles.retain(|n| !n.is_empty());
    needles.sort();
    needles.dedup();
    let red = |s: &str| redact_text(s, &needles);
    let body_limit = ctx.settings.omit_large_bodies.then_some(BODY_LIMIT);

    // 送った内容。アプリが付ける既定のヘッダー（User-Agent など）も、実際に送ったものとして並べる
    let mut sent_headers = prepared.headers.clone();
    for (name, value) in [
        ("User-Agent", DEFAULT_USER_AGENT),
        ("Accept", DEFAULT_ACCEPT),
        ("Accept-Encoding", DEFAULT_ACCEPT_ENCODING),
    ] {
        if !sent_headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case(name))
        {
            sent_headers.push(HeaderPair {
                name: name.to_string(),
                value: value.to_string(),
            });
        }
    }
    let request_headers = redact_headers(&sent_headers, &needles);

    let mut request_body: Option<String> = None;
    let mut request_body_kind = HistoryBodyKind::None;
    let mut request_body_size: u32 = 0;
    let mut request_body_truncated = false;
    let mut request_parts: Vec<HistoryPart> = Vec::new();
    let mut request_file: Option<String> = None;
    match &prepared.body {
        PreparedBody::None => {}
        PreparedBody::Bytes(bytes) => {
            request_body_size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    request_body_kind = HistoryBodyKind::Text;
                    let (t, cut) = truncate_text(red(text), body_limit);
                    request_body = Some(t);
                    request_body_truncated = cut;
                }
                Err(_) => request_body_kind = HistoryBodyKind::Binary,
            }
        }
        PreparedBody::File(path) => {
            request_body_kind = HistoryBodyKind::File;
            request_file = Some(red(path));
            request_body_size = std::fs::metadata(path)
                .map(|m| u32::try_from(m.len()).unwrap_or(u32::MAX))
                .unwrap_or(0);
        }
        PreparedBody::Multipart { parts, .. } => {
            request_body_kind = HistoryBodyKind::Multipart;
            let mut lines: Vec<String> = Vec::new();
            let mut size: u64 = 0;
            for p in parts {
                match &p.value {
                    PreparedPartValue::Text(t) => {
                        size += t.len() as u64;
                        let value = red(t);
                        lines.push(format!("{}={value}", red(&p.name)));
                        request_parts.push(HistoryPart {
                            name: red(&p.name),
                            is_file: false,
                            value,
                            content_type: p.content_type.clone(),
                        });
                    }
                    PreparedPartValue::File(path) => {
                        size += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                        request_parts.push(HistoryPart {
                            name: red(&p.name),
                            is_file: true,
                            value: red(path),
                            content_type: p.content_type.clone(),
                        });
                    }
                }
            }
            request_body_size = u32::try_from(size).unwrap_or(u32::MAX);
            let (t, cut) = truncate_text(lines.join("\n"), body_limit);
            request_body = Some(t).filter(|t| !t.is_empty());
            request_body_truncated = cut;
        }
    }

    let url = red(&prepared.url);
    let mut e = HistoryEntry {
        id: nekoreq_core::new_id(),
        project_id: ctx.project_id.clone(),
        sent_at: ctx.sent_at.clone(),
        environment_id: ctx.environment.as_ref().map(|(id, _)| id.clone()),
        environment_name: ctx.environment.as_ref().map(|(_, name)| name.clone()),
        op_key: ctx.op_key.clone(),
        op_summary: ctx.op_summary.clone(),
        method: prepared.method.clone(),
        url,
        final_url: None,
        request_headers,
        request_body,
        request_body_kind,
        request_body_size,
        request_body_truncated,
        request_parts,
        request_file,
        status: None,
        status_text: None,
        response_headers: Vec::new(),
        response_content_type: None,
        response_body: None,
        response_binary: false,
        response_body_truncated: false,
        response_size: 0,
        timing: None,
        error: None,
        error_kind: None,
        input: Some(input),
        masked_fields,
        override_names: ctx.override_names.clone(),
    };

    match outcome {
        Outcome::Response(r) => {
            e.status = Some(r.status);
            e.status_text = Some(r.status_text.clone()).filter(|t| !t.is_empty());
            // 名前で伏せるヘッダー（Set-Cookie など）と、値で伏せる箇所（ヘッダーの中にエコーされたもの）
            e.response_headers = redact_headers(&r.headers, &needles);
            e.response_content_type = r.body.content_type.clone();
            e.response_size = r.body.size;
            e.timing = Some(r.timing.clone());
            let final_url = red(&r.final_url);
            if final_url != e.url {
                e.final_url = Some(final_url);
            }
            match r.body.encoding {
                BodyEncoding::Text => {
                    // 先に伏せてから切る（切り口をまたぐシークレットを取り残さない）。受け取るときの表示上限で
                    // すでに切れているボディは、切り口にシークレットの先頭だけが残りうるので、その断片も落とす
                    let mut content = red(&r.body.content);
                    if r.body.truncated {
                        content = drop_partial_secret_tail(&content, &needles);
                    }
                    let (t, cut) = truncate_text(content, body_limit);
                    e.response_body = Some(t);
                    e.response_body_truncated = r.body.truncated || cut;
                }
                BodyEncoding::Base64 => {
                    e.response_binary = true;
                    e.response_body_truncated = r.body.truncated;
                }
            }
        }
        Outcome::Error(err) => {
            e.error = Some(red(err.message()));
            e.error_kind = Some(error_kind(err).to_string());
        }
    }
    e
}

/// レスポンスの `Set-Cookie` で受け取った SESSION・XSRF-TOKEN の値（伏せ字の対象に加える）。
/// リダイレクトの途中のレスポンス（`hops`）で受け取ったものも含む。
pub(crate) fn received_secrets(r: &ResponseData, hops: &[RedirectHop]) -> Vec<String> {
    let now = Utc::now();
    hops.iter()
        .map(|h| cookie_changes(&h.set_cookies, now))
        .chain([cookie_changes(&r.headers, now)])
        .flat_map(|changes| [changes.session, changes.xsrf])
        .flatten()
        .filter_map(|c| match c {
            CookieChange::Set(v) => Some(v),
            CookieChange::Clear => None,
        })
        .collect()
}

// ---- 日付（ローカル） ----

/// 日付の区切りに使うタイムゾーン。
#[derive(Clone, Copy)]
enum Tz {
    Fixed(FixedOffset),
    Local,
}

impl Tz {
    fn new(offset_minutes: Option<i32>) -> Self {
        offset_minutes
            .and_then(|m| FixedOffset::east_opt(m.clamp(-14 * 60, 14 * 60) * 60))
            .map_or(Tz::Local, Tz::Fixed)
    }

    /// UTC の RFC 3339 → ローカルの日付（`2026-11-17`）。読めなければ空。
    fn local_date(self, rfc3339: &str) -> String {
        let Ok(t) = DateTime::parse_from_rfc3339(rfc3339) else {
            return String::new();
        };
        let d = match self {
            Tz::Fixed(o) => t.with_timezone(&o).date_naive(),
            Tz::Local => t.with_timezone(&Local).date_naive(),
        };
        format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
    }

    /// ローカルの日付の 0 時（UTC の RFC 3339）。
    fn day_start_utc(self, date: NaiveDate) -> Option<String> {
        let midnight = date.and_hms_opt(0, 0, 0)?;
        let utc = match self {
            Tz::Fixed(o) => o
                .from_local_datetime(&midnight)
                .earliest()?
                .with_timezone(&Utc),
            Tz::Local => Local
                .from_local_datetime(&midnight)
                .earliest()?
                .with_timezone(&Utc),
        };
        Some(utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    }
}

fn parse_date(s: &str, what: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| {
        AppError::Validation(format!(
            "{what}の日付を読めません（2026-11-17 の形で指定してください）: {s}"
        ))
    })
}

// ---- 再送信・編集して開く・入力値として保存 ----

/// 「編集して開く」で返す、元の入力。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOpen {
    /// yaml の API なら、そのキー（新しいタブでその API を開き、`request` を未保存の入力として入れる）。
    /// `None` なら、手で作ったリクエストとして新しいタブで開く。
    pub op_key: Option<OpKey>,
    pub summary: Option<String>,
    /// 送信に使った元の入力。`••••` に伏せた認証・ヘッダー・Cookie は外してある。
    pub request: RequestSpec,
    /// 送信したときの環境（今は使わない。表示用）。
    pub environment_id: Option<String>,
    pub environment_name: Option<String>,
    /// 外したもの（「認証（Bearer のトークン）」など）。画面は「手で入力した認証の値は保存していないので、入れ直してください」と出す。
    pub dropped_fields: Vec<String>,
    /// 送信のときにあった変数の上書きの名前（値は保存していない。必要なら入れ直す）。
    pub override_names: Vec<String>,
}

/// 「入力値として保存」の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum SavedInput {
    /// yaml の API の入力値（RequestDraft）に反映した。
    Draft {
        draft: RequestDraft,
        dropped: Vec<String>,
    },
    /// 手で作ったリクエスト（「その他」）として保存した。
    Adhoc {
        request: AdHocRequest,
        dropped: Vec<String>,
    },
}

impl AppState {
    // ---- 記録 ----

    /// 履歴を残す。設定が「保存しない」なら何もしない。失敗しても送信は止めない。
    pub(crate) fn history_record(
        &self,
        ctx: &HistoryCtx,
        prepared: &PreparedRequest,
        outcome: Outcome<'_>,
        extra_secrets: &[String],
    ) {
        if ctx.settings.limit == 0 {
            return;
        }
        let entry = build_entry(ctx, prepared, outcome, extra_secrets);
        if let Err(e) = self.db().history_insert(&entry, ctx.settings.limit) {
            // 送信の結果は変えない。標準エラーに記録する（シークレットが入っていても伏せる）
            eprintln!(
                "[history] 履歴を保存できませんでした: {}",
                redact_text(&e.to_string(), &prepared.secrets)
            );
        }
    }

    /// `request_send` の入力から、履歴の文脈（Operation の見出しなど）を作る。
    pub(crate) fn history_ctx(
        &self,
        input: &RequestInput,
        environment: Option<(String, String)>,
        settings: HistorySettings,
    ) -> HistoryCtx {
        let op_summary = input.op_key.as_ref().and_then(|k| {
            self.db()
                .operation_get(&input.project_id, k)
                .ok()
                .flatten()
                .and_then(|s| s.operation.summary)
        });
        HistoryCtx {
            project_id: input.project_id.clone(),
            sent_at: now_rfc3339(),
            environment,
            op_key: input.op_key.clone(),
            op_summary,
            input: input.request.clone(),
            override_names: input.overrides.iter().map(|o| o.name.clone()).collect(),
            settings,
        }
    }

    // ---- 一覧・取得・削除 ----

    /// 履歴の一覧（新しい順）。日付（ローカル）ごとの区切りに使う `localDate` つき。
    pub fn history_list(
        &self,
        project_id: &str,
        query: &HistoryQuery,
    ) -> Result<HistoryPage, AppError> {
        let tz = Tz::new(query.utc_offset_minutes);
        let from_utc = match query.from.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(s) => tz.day_start_utc(parse_date(s, "期間の始め")?),
            None => None,
        };
        let to_utc = match query.to.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(s) => {
                let next = parse_date(s, "期間の終わり")?.succ_opt().ok_or_else(|| {
                    AppError::Validation("期間の終わりの日付が大きすぎます".into())
                })?;
                tz.day_start_utc(next)
            }
            None => None,
        };
        let filter = HistoryFilter {
            text: query.text.clone(),
            statuses: query.statuses.clone(),
            environment_id: query.environment_id.clone(),
            from_utc,
            to_utc,
            limit: query.limit.unwrap_or(100).clamp(1, 500),
            cursor: query.cursor.clone(),
        };
        let mut rows = self.db().history_list(project_id, &filter)?;
        for item in &mut rows.items {
            item.local_date = tz.local_date(&item.sent_at);
        }
        Ok(HistoryPage {
            items: rows.items,
            total: rows.total,
            next_cursor: rows.next_cursor,
        })
    }

    pub fn history_get(&self, id: &str) -> Result<HistoryEntry, AppError> {
        self.db()
            .history_get(id)?
            .ok_or_else(|| AppError::NotFound(format!("履歴が見つかりません: {id}")))
    }

    pub fn history_delete(&self, id: &str) -> Result<(), AppError> {
        Ok(self.db().history_delete(id)?)
    }

    /// プロジェクトの履歴をすべて消す。消した件数を返す。
    pub fn history_clear(&self, project_id: &str) -> Result<u32, AppError> {
        Ok(self.db().history_clear(project_id)?)
    }

    // ---- 再送信・編集して開く・入力値として保存 ----

    fn history_input(
        &self,
        id: &str,
    ) -> Result<(HistoryEntry, RequestSpec, Vec<String>), AppError> {
        let entry = self.history_get(id)?;
        let input = entry.input.clone().ok_or_else(|| {
            AppError::Validation("この履歴には送信に使った入力が残っていません".into())
        })?;
        let (spec, mut dropped) = drop_masked(&input);
        // 大きすぎて残さなかったボディも、使えなかったものとして知らせる
        dropped.extend(
            entry
                .masked_fields
                .iter()
                .filter(|f| f.starts_with("ボディ"))
                .cloned(),
        );
        Ok((entry, spec, dropped))
    }

    /// 編集して開く: 送信に使った元の入力を返す（`{{変数}}` のまま。伏せ字の行・認証は外す）。
    pub fn history_open(&self, id: &str) -> Result<HistoryOpen, AppError> {
        let (entry, request, dropped_fields) = self.history_input(id)?;
        Ok(HistoryOpen {
            op_key: entry.op_key,
            summary: entry.op_summary,
            request,
            environment_id: entry.environment_id,
            environment_name: entry.environment_name,
            dropped_fields,
            override_names: entry.override_names,
        })
    }

    /// 再送信: 元の入力を、今の環境（プロジェクトの「選んでいる環境」）で送り直す。
    /// 送信 ID をすぐ返し、結果は `request://finished` で届く（`request_send` と同じ。新しい履歴も残る）。
    /// 履歴にある変数の上書きの値は保存していないので、使わない（必要な変数は環境に入れておく）。
    /// tokio のランタイムの中で呼ぶこと。
    pub fn history_resend<F>(
        &self,
        id: &str,
        settings: Option<SendSettings>,
        on_finished: F,
    ) -> Result<String, AppError>
    where
        F: FnOnce(RequestFinished) + Send + 'static,
    {
        let (entry, request, _dropped) = self.history_input(id)?;
        self.request_send(
            RequestInput {
                project_id: entry.project_id,
                environment_id: None,
                request,
                overrides: vec![],
                op_key: entry.op_key,
            },
            settings,
            on_finished,
        )
    }

    /// 入力値として保存: yaml の API なら RequestDraft（`op_key` が同じ入力値を置き換える）、
    /// 手で作ったものなら AdHocRequest（`name` がなければ `METHOD /path`）として保存する。
    /// 伏せ字の認証・ヘッダー・Cookie は保存しない（`dropped` に名前を返す）。
    pub fn history_save_as_input(
        &self,
        id: &str,
        name: Option<&str>,
    ) -> Result<SavedInput, AppError> {
        let (entry, spec, dropped) = self.history_input(id)?;
        match entry.op_key {
            Some(op_key) => {
                let draft = RequestDraft {
                    id: String::new(),
                    project_id: entry.project_id.clone(),
                    op_key,
                    params: spec.params,
                    body: spec.body,
                    auth: spec.auth,
                    updated_at: String::new(),
                };
                Ok(SavedInput::Draft {
                    draft: self.db().draft_save(&draft)?,
                    dropped,
                })
            }
            None => {
                let default_name = adhoc_name(&spec.method, &spec.url);
                let req = AdHocRequest {
                    id: String::new(),
                    project_id: entry.project_id.clone(),
                    name: name
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map_or(default_name, str::to_string),
                    description: String::new(),
                    method: spec.method,
                    url: spec.url,
                    params: spec.params,
                    body: spec.body,
                    auth: spec.auth,
                    created_at: String::new(),
                    updated_at: String::new(),
                };
                Ok(SavedInput::Adhoc {
                    request: self.db().adhoc_save(&req)?,
                    dropped,
                })
            }
        }
    }

    // ---- cURL としてコピー ----

    /// 履歴の送った内容（伏せ字済み）を cURL コマンドにする。伏せ字（`••••`）はそのまま入る。
    pub fn history_curl(&self, id: &str) -> Result<String, AppError> {
        let entry = self.history_get(id)?;
        Ok(crate::curl::history_to_curl(&entry))
    }

    // ---- 比較 ----

    /// 2 件のレスポンスの差分（ステータス・ヘッダー・ボディの行単位）。左右は渡した順。
    pub fn history_compare(&self, left_id: &str, right_id: &str) -> Result<HistoryDiff, AppError> {
        let left = self.history_get(left_id)?;
        let right = self.history_get(right_id)?;
        let item = |e: &HistoryEntry| HistoryListItem {
            id: e.id.clone(),
            sent_at: e.sent_at.clone(),
            local_date: Tz::Local.local_date(&e.sent_at),
            method: e.method.clone(),
            url: e.url.clone(),
            status: e.status,
            status_text: e.status_text.clone(),
            total_ms: e.timing.as_ref().map(|t| t.total_ms),
            environment_id: e.environment_id.clone(),
            environment_name: e.environment_name.clone(),
            op_key: e.op_key.clone(),
            op_summary: e.op_summary.clone(),
            failed: e.status.is_none(),
            error_kind: e.error_kind.clone(),
            response_size: e.response_size,
        };
        Ok(compare_entries(&left, &right, item(&left), item(&right)))
    }
}

/// 手で作ったリクエストの既定の名前（`GET /api/v1/x`。長すぎれば切る）。
fn adhoc_name(method: &str, url: &str) -> String {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_start_matches("{{baseUrl}}");
    let path = match path.find("://") {
        Some(i) => path[i + 3..].find('/').map_or("/", |j| &path[i + 3 + j..]),
        None => path,
    };
    let name = format!("{} {}", method.trim().to_ascii_uppercase(), path);
    name.chars().take(200).collect()
}

// ---- 差分 ----

/// 差分にかける行数の上限（片側）。超えたら先頭だけ比べる。
const MAX_DIFF_LINES: usize = 20_000;

fn status_text(e: &HistoryEntry) -> String {
    match (e.status, &e.error) {
        (Some(s), _) => match &e.status_text {
            Some(t) => format!("{s} {t}"),
            None => s.to_string(),
        },
        (None, Some(err)) => format!("失敗: {err}"),
        (None, None) => "失敗".to_string(),
    }
}

fn header_lines(headers: &[HeaderPair]) -> Vec<String> {
    let mut v: Vec<(String, String)> = headers
        .iter()
        .map(|h| {
            (
                h.name.to_ascii_lowercase(),
                format!("{}: {}", h.name, h.value),
            )
        })
        .collect();
    // 順番の違いは差にしない
    v.sort();
    v.into_iter().map(|(_, line)| line).collect()
}

/// JSON なら整形して（キーの順は保つ）、そうでなければそのまま。整形したら `true`。
fn pretty_body(content_type: Option<&str>, body: &str) -> (String, bool) {
    let trimmed = body.trim_start();
    let looks_json = content_type.is_some_and(|c| c.to_ascii_lowercase().contains("json"))
        || trimmed.starts_with('{')
        || trimmed.starts_with('[');
    if looks_json {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            if let Ok(p) = serde_json::to_string_pretty(&v) {
                return (p, true);
            }
        }
    }
    (body.to_string(), false)
}

/// 2 つの行のリストの差分。変わらない行は前後 3 行だけ残して、あいだを `Skip` にする。
fn diff_lines(left: &[String], right: &[String]) -> (Vec<DiffLine>, bool) {
    let a: Vec<&str> = left.iter().map(String::as_str).collect();
    let b: Vec<&str> = right.iter().map(String::as_str).collect();
    let diff = similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .timeout(std::time::Duration::from_secs(2))
        .diff_slices(&a, &b);
    let changed = diff
        .ops()
        .iter()
        .any(|op| !matches!(op, similar::DiffOp::Equal { .. }));
    if !changed {
        return (Vec::new(), false);
    }
    let mut out: Vec<DiffLine> = Vec::new();
    let skip = |n: usize| DiffLine {
        tag: DiffTag::Skip,
        text: format!("{n} 行省略"),
        left_line: None,
        right_line: None,
    };
    let mut prev_old_end = 0usize;
    for (gi, group) in diff.grouped_ops(3).iter().enumerate() {
        let first_old = group.first().map_or(0, |op| op.old_range().start);
        let gap = first_old.saturating_sub(prev_old_end);
        if gap > 0 && (gi > 0 || first_old > 0) {
            out.push(skip(gap));
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let tag = match change.tag() {
                    similar::ChangeTag::Equal => DiffTag::Equal,
                    similar::ChangeTag::Delete => DiffTag::Delete,
                    similar::ChangeTag::Insert => DiffTag::Insert,
                };
                out.push(DiffLine {
                    tag,
                    text: change.value().to_string(),
                    left_line: change
                        .old_index()
                        .map(|i| u32::try_from(i + 1).unwrap_or(u32::MAX)),
                    right_line: change
                        .new_index()
                        .map(|i| u32::try_from(i + 1).unwrap_or(u32::MAX)),
                });
            }
        }
        prev_old_end = group.last().map_or(0, |op| op.old_range().end);
    }
    let tail = a.len().saturating_sub(prev_old_end);
    if tail > 0 {
        out.push(skip(tail));
    }
    (out, true)
}

fn lines_of(text: &str) -> (Vec<String>, bool) {
    let all: Vec<String> = text.lines().map(str::to_string).collect();
    if all.len() > MAX_DIFF_LINES {
        (all.into_iter().take(MAX_DIFF_LINES).collect(), true)
    } else {
        (all, false)
    }
}

pub(crate) fn compare_entries(
    left: &HistoryEntry,
    right: &HistoryEntry,
    left_item: HistoryListItem,
    right_item: HistoryListItem,
) -> HistoryDiff {
    let left_status = status_text(left);
    let right_status = status_text(right);

    let (headers, headers_changed) = diff_lines(
        &header_lines(&left.response_headers),
        &header_lines(&right.response_headers),
    );

    let mut notes: Vec<String> = Vec::new();
    let (body, body_changed) = if left.response_binary || right.response_binary {
        notes.push("バイナリのボディは比べません".to_string());
        (Vec::new(), left.response_size != right.response_size)
    } else {
        let side = |e: &HistoryEntry| {
            let raw = e.response_body.clone().unwrap_or_default();
            pretty_body(e.response_content_type.as_deref(), &raw)
        };
        let (l, l_pretty) = side(left);
        let (r, r_pretty) = side(right);
        if l_pretty || r_pretty {
            notes.push("JSON は整形してから比べました".to_string());
        }
        if left.response_body_truncated || right.response_body_truncated {
            notes.push("保存してある先頭の部分だけを比べました".to_string());
        }
        let (ll, lcut) = lines_of(&l);
        let (rl, rcut) = lines_of(&r);
        if lcut || rcut {
            notes.push(format!("長いため先頭の {MAX_DIFF_LINES} 行だけ比べました"));
        }
        diff_lines(&ll, &rl)
    };

    HistoryDiff {
        left: left_item,
        right: right_item,
        status_changed: left_status != right_status,
        left_status,
        right_status,
        headers,
        headers_changed,
        body,
        body_changed,
        body_note: (!notes.is_empty()).then(|| notes.join("。")),
    }
}

/// `KeyValue` の Header 行（履歴の送ったヘッダー → cURL に使う）。
pub(crate) fn header_rows(headers: &[HeaderPair]) -> Vec<KeyValue> {
    headers
        .iter()
        .map(|h| KeyValue {
            location: ParamIn::Header,
            name: h.name.clone(),
            value: h.value.clone(),
            enabled: true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    #[test]
    fn diff_collapses_unchanged_runs() {
        let a: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        let mut b = a.clone();
        b[14] = "CHANGED".into();
        let (d, changed) = diff_lines(&a, &b);
        assert!(changed);
        // 先頭の省略（11 行）・前後 3 行・削除と追加・後ろの 3 行・末尾の省略（12 行）
        assert_eq!(d[0].tag, DiffTag::Skip);
        assert_eq!(d[0].text, "11 行省略");
        let del = d.iter().find(|l| l.tag == DiffTag::Delete).unwrap();
        assert_eq!(del.text, "line 15");
        assert_eq!(del.left_line, Some(15));
        let ins = d.iter().find(|l| l.tag == DiffTag::Insert).unwrap();
        assert_eq!(ins.text, "CHANGED");
        assert_eq!(ins.right_line, Some(15));
        assert_eq!(d.last().unwrap().tag, DiffTag::Skip);
        assert_eq!(d.last().unwrap().text, "12 行省略");
        let equal = d.iter().filter(|l| l.tag == DiffTag::Equal).count();
        assert_eq!(equal, 6);
    }

    #[test]
    fn identical_inputs_have_no_diff() {
        let a = lines("a\nb\nc");
        let (d, changed) = diff_lines(&a, &a);
        assert!(d.is_empty() && !changed);
    }

    #[test]
    fn json_is_pretty_printed_before_comparing() {
        let (p, pretty) = pretty_body(Some("application/json"), r#"{"a":1,"b":[1,2]}"#);
        assert!(pretty);
        assert!(p.contains("\n  \"a\": 1"));
        // JSON でなければそのまま
        let (p, pretty) = pretty_body(Some("text/plain"), "hello");
        assert_eq!((p.as_str(), pretty), ("hello", false));
        // 壊れた JSON もそのまま
        let (p, pretty) = pretty_body(Some("application/json"), "{broken");
        assert_eq!((p.as_str(), pretty), ("{broken", false));
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let (t, cut) = truncate_text("あいう".to_string(), Some(4));
        assert_eq!((t.as_str(), cut), ("あ", true));
        let (t, cut) = truncate_text("abc".to_string(), Some(10));
        assert_eq!((t.as_str(), cut), ("abc", false));
        let (t, cut) = truncate_text("abc".to_string(), None);
        assert_eq!((t.as_str(), cut), ("abc", false));
    }

    #[test]
    fn local_dates_follow_the_offset() {
        let jst = Tz::new(Some(540));
        // UTC 2026-11-17 16:00 は JST で 18 日の 1 時
        assert_eq!(jst.local_date("2026-11-17T16:00:00.000Z"), "2026-11-18");
        assert_eq!(jst.local_date("2026-11-17T14:59:59.000Z"), "2026-11-17");
        let d = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        assert_eq!(
            jst.day_start_utc(d).as_deref(),
            Some("2026-11-17T15:00:00.000Z")
        );
        let ny = Tz::new(Some(-300));
        assert_eq!(ny.local_date("2026-11-17T04:00:00.000Z"), "2026-11-16");
        assert_eq!(jst.local_date("garbage"), "");
    }

    // ---- 履歴を作る・残す ----

    use nekoreq_core::{ConnectionInfo, MultipartValue, PreparedPart, ResponseBody, Timing, MASK};
    use nekoreq_secrets::MemorySecretStore;
    use std::sync::Arc;

    fn open() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open_with_secrets(
            &dir.path().join("data"),
            Arc::new(MemorySecretStore::new()),
        )
        .unwrap();
        (dir, state)
    }

    fn ctx(project_id: &str, sent_at: &str) -> HistoryCtx {
        HistoryCtx {
            project_id: project_id.into(),
            sent_at: sent_at.into(),
            environment: None,
            op_key: None,
            op_summary: None,
            input: RequestSpec {
                method: "GET".into(),
                url: "{{baseUrl}}/a".into(),
                params: vec![],
                body: nekoreq_core::Body::None,
                auth: None,
            },
            override_names: vec![],
            settings: HistorySettings::default(),
        }
    }

    fn prepared(url: &str) -> PreparedRequest {
        PreparedRequest {
            method: "GET".into(),
            url: url.into(),
            headers: vec![],
            body: PreparedBody::None,
            secrets: vec![],
        }
    }

    fn response(body: &str) -> ResponseData {
        ResponseData {
            status: 200,
            status_text: "OK".into(),
            headers: vec![],
            body: ResponseBody {
                content_type: Some("text/plain".into()),
                encoding: BodyEncoding::Text,
                content: body.into(),
                size: u32::try_from(body.len()).unwrap(),
                truncated: false,
            },
            timing: Timing {
                dns_ms: None,
                connect_ms: Some(1.0),
                tls_ms: None,
                send_ms: None,
                wait_ms: Some(2.0),
                download_ms: Some(1.0),
                total_ms: 4.0,
            },
            connection: ConnectionInfo {
                remote_addr: None,
                http_version: "HTTP/1.1".into(),
                tls: false,
            },
            final_url: "http://localhost/a".into(),
            redirect_count: 0,
            sent_bytes: 10,
            received_bytes: 10,
        }
    }

    #[test]
    fn a_secret_cut_by_the_body_limit_is_not_left_behind() {
        // 1 MB の境目をまたぐ位置にシークレット（伏せてから切る）
        let secret = "CROSSING-SECRET-VALUE";
        let body = format!("{}{secret}{}", "x".repeat(BODY_LIMIT - 8), "y".repeat(100));
        let mut p = prepared("http://localhost/a");
        p.secrets = vec![secret.to_string()];
        let e = build_entry(
            &ctx("p", "2026-11-17T00:00:00.000Z"),
            &p,
            Outcome::Response(&response(&body)),
            &[],
        );
        let stored = e.response_body.unwrap();
        assert!(e.response_body_truncated);
        assert!(stored.len() <= BODY_LIMIT);
        assert!(
            !stored.contains("CROSSING"),
            "切り口の手前にシークレットの一部が残った"
        );
        assert!(stored.ends_with(MASK) || stored.ends_with('x') || stored.ends_with("••"));
    }

    #[test]
    fn multipart_and_file_bodies_keep_names_and_types_but_not_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.bin");
        std::fs::write(&file, b"FILE-CONTENTS").unwrap();
        let mut p = prepared("http://localhost/up");
        p.secrets = vec!["TOP-SECRET".into()];
        p.body = PreparedBody::Multipart {
            boundary: "B".into(),
            parts: vec![
                PreparedPart {
                    name: "note".into(),
                    value: PreparedPartValue::Text("hello TOP-SECRET".into()),
                    content_type: Some("text/plain".into()),
                },
                PreparedPart {
                    name: "file".into(),
                    value: PreparedPartValue::File(file.display().to_string()),
                    content_type: None,
                },
            ],
        };
        let _ = MultipartValue::Text(String::new());
        let e = build_entry(
            &ctx("p", "2026-11-17T00:00:00.000Z"),
            &p,
            Outcome::Error(&SendError::Connect {
                message: "接続できません TOP-SECRET".into(),
            }),
            &[],
        );
        assert_eq!(e.request_body_kind, HistoryBodyKind::Multipart);
        assert_eq!(e.request_body.as_deref(), Some("note=hello ••••"));
        assert_eq!(e.request_parts.len(), 2);
        assert_eq!(
            e.request_parts[0].content_type.as_deref(),
            Some("text/plain")
        );
        assert!(e.request_parts[1].is_file);
        assert_eq!(
            e.request_body_size as usize,
            "hello TOP-SECRET".len() + "FILE-CONTENTS".len()
        );
        assert_eq!(e.error.as_deref(), Some("接続できません ••••"));
        assert_eq!(e.error_kind.as_deref(), Some("connect"));
        assert!(!serde_json::to_string(&e).unwrap().contains("FILE-CONTENTS"));

        p.body = PreparedBody::File(file.display().to_string());
        let e = build_entry(
            &ctx("p", "2026-11-17T00:00:00.000Z"),
            &p,
            Outcome::Response(&response("ok")),
            &[],
        );
        assert_eq!(e.request_body_kind, HistoryBodyKind::File);
        assert_eq!(
            e.request_file.as_deref(),
            Some(file.display().to_string().as_str())
        );
        assert_eq!(e.request_body, None);
        assert_eq!(e.request_body_size, 13);
        // バイナリのバイト列は中身を残さない
        p.body = PreparedBody::Bytes(vec![0xff, 0xfe, 0x00]);
        let e = build_entry(
            &ctx("p", "2026-11-17T00:00:00.000Z"),
            &p,
            Outcome::Response(&response("ok")),
            &[],
        );
        assert_eq!(e.request_body_kind, HistoryBodyKind::Binary);
        assert_eq!(e.request_body, None);
        assert_eq!(e.request_body_size, 3);
    }

    #[test]
    fn received_set_cookie_values_are_redacted_everywhere() {
        let mut r = response(r#"{"issued":"SESS-ABC and XSRF-DEF","other":"keep"}"#);
        r.headers = vec![
            HeaderPair {
                name: "Set-Cookie".into(),
                value: "SESSION=SESS-ABC; Path=/".into(),
            },
            HeaderPair {
                name: "Set-Cookie".into(),
                value: "XSRF-TOKEN=XSRF-DEF; Path=/".into(),
            },
            HeaderPair {
                name: "X-Echo".into(),
                value: "SESS-ABC".into(),
            },
        ];
        r.final_url = "http://localhost/a?x=SESS-ABC".into();
        let extra = received_secrets(&r, &[]);
        assert_eq!(extra, vec!["SESS-ABC".to_string(), "XSRF-DEF".to_string()]);
        let e = build_entry(
            &ctx("p", "2026-11-17T00:00:00.000Z"),
            &prepared("http://localhost/a?x=SESS-ABC"),
            Outcome::Response(&r),
            &extra,
        );
        let dump = serde_json::to_string(&e).unwrap();
        assert!(
            !dump.contains("SESS-ABC") && !dump.contains("XSRF-DEF"),
            "{dump}"
        );
        assert_eq!(e.response_headers[2].value, "••••");
        assert!(e.response_body.unwrap().contains(r#""other":"keep""#));
        assert_eq!(e.final_url, None, "URL が同じなら最終 URL は持たない");
    }

    #[test]
    fn the_oldest_entries_are_dropped_beyond_the_limit() {
        let (_d, state) = open();
        let p = state.project_create("p").unwrap();
        for n in 0..1005 {
            let c = ctx(
                &p.id,
                &format!(
                    "2026-11-17T{:02}:{:02}:{:02}.000Z",
                    n / 3600,
                    (n / 60) % 60,
                    n % 60
                ),
            );
            state.history_record(
                &c,
                &prepared(&format!("http://localhost/a?n={n}")),
                Outcome::Response(&response("ok")),
                &[],
            );
        }
        let page = state
            .history_list(
                &p.id,
                &HistoryQuery {
                    limit: Some(500),
                    ..HistoryQuery::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 1000, "既定の上限は 1,000 件");
        assert_eq!(page.items[0].url, "http://localhost/a?n=1004");
        // いちばん古い 5 件（n = 0..5）が消えている
        let oldest = state
            .history_list(
                &p.id,
                &HistoryQuery {
                    text: Some("a?n=0".into()),
                    ..HistoryQuery::default()
                },
            )
            .unwrap();
        assert_eq!(oldest.total, 0);
        // 設定を 5,000 件にすれば、そのぶん残る
        let mut s = state.app_settings_get().unwrap();
        s.history.limit = 5000;
        state.app_settings_set(&s).unwrap();
        let mut c = ctx(&p.id, "2026-11-18T00:00:00.000Z");
        c.settings = s.history.clone();
        for n in 0..10 {
            state.history_record(
                &c,
                &prepared(&format!("http://localhost/b?n={n}")),
                Outcome::Response(&response("ok")),
                &[],
            );
        }
        assert_eq!(
            state
                .history_list(&p.id, &HistoryQuery::default())
                .unwrap()
                .total,
            1010
        );
    }

    #[test]
    fn ten_thousand_entries_list_and_filter_quickly() {
        let (_d, state) = open();
        let p = state.project_create("p").unwrap();
        let t = std::time::Instant::now();
        for n in 0..10_000u32 {
            let mut c = ctx(
                &p.id,
                &format!(
                    "2026-11-{:02}T{:02}:{:02}:00.000Z",
                    1 + n / 1440 % 28,
                    n / 60 % 24,
                    n % 60
                ),
            );
            c.settings.limit = 10_000; // 上限の確認ではないので、DB に直接入れる
            let mut e = build_entry(
                &c,
                &prepared(&format!(
                    "http://localhost:8080/api/v1/video/clients/{n}?q=Netflix"
                )),
                Outcome::Response(&response(&format!(
                    "{{\"name\":\"取引先{n}\",\"pad\":\"{}\"}}",
                    "あ".repeat(200)
                ))),
                &[],
            );
            e.status = Some(if n % 7 == 0 {
                500
            } else if n % 5 == 0 {
                404
            } else {
                200
            });
            state.db().history_insert(&e, 10_000).unwrap();
        }
        eprintln!("10,000 件の用意: {:?}", t.elapsed());
        let time = |q: HistoryQuery| {
            let t = std::time::Instant::now();
            let page = state.history_list(&p.id, &q).unwrap();
            (page, t.elapsed())
        };
        let (page, first) = time(HistoryQuery {
            utc_offset_minutes: Some(540),
            ..HistoryQuery::default()
        });
        assert_eq!((page.items.len(), page.total), (100, 10_000));
        assert!(page.items.iter().all(|i| !i.local_date.is_empty()));
        let (page, filtered) = time(HistoryQuery {
            statuses: vec![
                nekoreq_core::StatusClass::S5xx,
                nekoreq_core::StatusClass::S4xx,
            ],
            from: Some("2026-11-03".into()),
            to: Some("2026-11-05".into()),
            cursor: None,
            ..HistoryQuery::default()
        });
        assert!(page.total > 0 && page.total < 10_000);
        let (page, search) = time(HistoryQuery {
            text: Some("取引先9999".into()),
            ..HistoryQuery::default()
        });
        assert_eq!(page.total, 1);
        let (page, url_search) = time(HistoryQuery {
            text: Some("clients/5000?".into()),
            ..HistoryQuery::default()
        });
        assert_eq!(page.total, 1);
        eprintln!("10,000 件: 1 ページ目 {first:?} / 絞り込み {filtered:?} / 本文の検索 {search:?} / URL の検索 {url_search:?}");
        for d in [first, filtered, search, url_search] {
            assert!(d.as_millis() < 300, "{d:?}");
        }
    }

    #[test]
    fn default_adhoc_names() {
        assert_eq!(
            adhoc_name("post", "{{baseUrl}}/api/v1/auth/login?x=1"),
            "POST /api/v1/auth/login"
        );
        assert_eq!(adhoc_name("GET", "http://localhost:8080/a/b"), "GET /a/b");
        assert_eq!(adhoc_name("GET", "http://localhost:8080"), "GET /");
    }
}
