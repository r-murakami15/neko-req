//! 履歴（`history`）と全文検索（`history_fts`）のリポジトリ関数。
//!
//! - 保存するのは、呼び出し側（app）が伏せ字にした内容だけ。ここではシークレットを探さない。
//! - 一覧（`history_list`）は、軽い列（ボディ・ヘッダー・元の入力を含まない）だけを読む。
//!   表の列の並びも、軽い列を前・重い列を後ろにしてある（一覧が長いボディのオーバーフローページを読まないため）。
//! - 並びは `sent_at DESC, seq DESC`。ページングは最後に返した行（`sent_at`・`seq`）からのカーソル。
//! - 全文検索: 検索語が 3 文字以上なら FTS5 の trigram（日本語の部分一致）、3 文字未満は `LIKE`（trigram は
//!   3 文字未満を引けないため）。
//! - 件数の上限は `history_insert` に渡す。プロジェクトごとに、古いものから消す。
//! - 消したあとは WAL を空にする（`secure_delete` と合わせて、消した内容をファイルに残さない）。

use nekoreq_core::{
    HeaderPair, HistoryBodyKind, HistoryEntry, HistoryListItem, HistoryPart, OpKey, RequestSpec,
    StatusClass, Timing,
};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StorageError};
use crate::Database;

fn corrupt(what: &str, e: impl std::fmt::Display) -> StorageError {
    StorageError::Corrupt(format!("history.{what}: {e}"))
}

/// 一覧の絞り込み。日付は UTC の RFC 3339 に直したもの（ローカルの日付からの変換は app がする）。
#[derive(Debug, Clone, Default)]
pub struct HistoryFilter {
    pub text: Option<String>,
    pub statuses: Vec<StatusClass>,
    pub environment_id: Option<String>,
    /// この日時以降（含む）。
    pub from_utc: Option<String>,
    /// この日時より前（含まない）。
    pub to_utc: Option<String>,
    pub limit: u32,
    pub cursor: Option<String>,
}

/// 一覧の 1 ページ（`local_date` は空。app が埋める）。
#[derive(Debug, Clone)]
pub struct HistoryRows {
    pub items: Vec<HistoryListItem>,
    pub total: u32,
    pub next_cursor: Option<String>,
}

/// 送ったボディのうち、列にしないもの。
#[derive(Debug, Default, Serialize, Deserialize)]
struct RequestExtra {
    #[serde(default)]
    kind: HistoryBodyKind,
    #[serde(default)]
    size: u32,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    parts: Vec<HistoryPart>,
    #[serde(default)]
    file: Option<String>,
}

const LIST_COLUMNS: &str =
    "h.seq, h.id, h.sent_at, h.method, h.url, h.status, h.status_text, h.total_ms, \
     h.environment_id, h.environment_name, h.op_key, h.op_summary, h.error_kind, h.response_size";

fn op_key_of(s: Option<String>) -> Option<OpKey> {
    s.and_then(|s| OpKey::parse(&s).ok())
}

fn list_item(row: &Row<'_>) -> rusqlite::Result<(i64, HistoryListItem)> {
    let status: Option<i64> = row.get("status")?;
    let size: i64 = row.get("response_size")?;
    Ok((
        row.get("seq")?,
        HistoryListItem {
            id: row.get("id")?,
            sent_at: row.get("sent_at")?,
            local_date: String::new(),
            method: row.get("method")?,
            url: row.get("url")?,
            status: status.and_then(|s| u16::try_from(s).ok()),
            status_text: row.get("status_text")?,
            total_ms: row.get("total_ms")?,
            environment_id: row.get("environment_id")?,
            environment_name: row.get("environment_name")?,
            op_key: op_key_of(row.get("op_key")?),
            op_summary: row.get("op_summary")?,
            failed: status.is_none(),
            error_kind: row.get("error_kind")?,
            response_size: u32::try_from(size).unwrap_or(u32::MAX),
        },
    ))
}

/// FTS5 の phrase（`"..."`）にする。中の `"` は二重にする。
fn fts_phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// `LIKE` のパターン（`%`・`_`・`\` を文字として扱う）。
fn like_pattern(text: &str) -> String {
    let mut out = String::from("%");
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// 絞り込みの WHERE 句（`h` は history）と、その引数。
fn where_clause(project_id: &str, f: &HistoryFilter, with_cursor: bool) -> (String, Vec<Value>) {
    let mut sql = String::from("h.project_id = ?");
    let mut args: Vec<Value> = vec![Value::Text(project_id.to_string())];

    // NUL 文字（U+0000）は FTS5 の構文エラーになるので、検索語から取り除く
    let text_without_nul = f.text.as_deref().map(|t| t.replace('\0', ""));
    if let Some(text) = text_without_nul
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        if text.chars().count() >= 3 {
            sql.push_str(" AND h.seq IN (SELECT rowid FROM history_fts WHERE history_fts MATCH ?)");
            args.push(Value::Text(fts_phrase(text)));
        } else {
            sql.push_str(
                " AND (h.url LIKE ? ESCAPE '\\' OR h.request_body LIKE ? ESCAPE '\\' OR h.response_body LIKE ? ESCAPE '\\')",
            );
            let p = like_pattern(text);
            args.push(Value::Text(p.clone()));
            args.push(Value::Text(p.clone()));
            args.push(Value::Text(p));
        }
    }
    if !f.statuses.is_empty() {
        let parts: Vec<&str> = f
            .statuses
            .iter()
            .map(|s| match s {
                StatusClass::S2xx => "(h.status >= 200 AND h.status < 300)",
                StatusClass::S3xx => "(h.status >= 300 AND h.status < 400)",
                StatusClass::S4xx => "(h.status >= 400 AND h.status < 500)",
                StatusClass::S5xx => "(h.status >= 500 AND h.status < 600)",
                StatusClass::Failed => "h.status IS NULL",
            })
            .collect();
        sql.push_str(&format!(" AND ({})", parts.join(" OR ")));
    }
    if let Some(env) = &f.environment_id {
        sql.push_str(" AND h.environment_id = ?");
        args.push(Value::Text(env.clone()));
    }
    if let Some(from) = &f.from_utc {
        sql.push_str(" AND h.sent_at >= ?");
        args.push(Value::Text(from.clone()));
    }
    if let Some(to) = &f.to_utc {
        sql.push_str(" AND h.sent_at < ?");
        args.push(Value::Text(to.clone()));
    }
    if with_cursor {
        if let Some((at, seq)) = f.cursor.as_deref().and_then(parse_cursor) {
            sql.push_str(" AND (h.sent_at < ? OR (h.sent_at = ? AND h.seq < ?))");
            args.push(Value::Text(at.clone()));
            args.push(Value::Text(at));
            args.push(Value::Integer(seq));
        }
    }
    (sql, args)
}

fn parse_cursor(c: &str) -> Option<(String, i64)> {
    let (at, seq) = c.rsplit_once('|')?;
    Some((at.to_string(), seq.parse().ok()?))
}

impl Database {
    /// 履歴を 1 件足す。`max_entries` が 0 でなければ、そのプロジェクトの履歴を新しい順に `max_entries` 件だけ残して
    /// 古いものを消す（足したのと同じトランザクションで）。
    pub fn history_insert(&self, e: &HistoryEntry, max_entries: u32) -> Result<()> {
        fn to_json<T: Serialize + ?Sized>(what: &str, v: &T) -> Result<String> {
            serde_json::to_string(v).map_err(|er| corrupt(what, er))
        }
        let extra = RequestExtra {
            kind: e.request_body_kind,
            size: e.request_body_size,
            truncated: e.request_body_truncated,
            parts: e.request_parts.clone(),
            file: e.request_file.clone(),
        };
        let request_headers = to_json("request_headers_json", &e.request_headers)?;
        let response_headers = to_json("response_headers_json", &e.response_headers)?;
        let extra_json = to_json("request_extra_json", &extra)?;
        let timing_json = e
            .timing
            .as_ref()
            .map(|t| to_json("timing_json", t))
            .transpose()?;
        let input_json = e
            .input
            .as_ref()
            .map(|i| to_json("input_json", i))
            .transpose()?;
        let masked = to_json("masked_fields_json", &e.masked_fields)?;
        let overrides = to_json("override_names_json", &e.override_names)?;

        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut trimmed = 0usize;
        tx.execute(
            "INSERT INTO history (
                id, project_id, sent_at, environment_id, environment_name, op_key, op_summary,
                method, url, final_url, status, status_text, total_ms, error_kind, error,
                response_size, response_binary, response_body_truncated, response_content_type,
                input_json, masked_fields_json, override_names_json,
                request_headers_json, request_extra_json, timing_json, response_headers_json,
                request_body, response_body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
            params![
                e.id,
                e.project_id,
                e.sent_at,
                e.environment_id,
                e.environment_name,
                e.op_key.as_ref().map(OpKey::to_key_string),
                e.op_summary,
                e.method,
                e.url,
                e.final_url,
                e.status.map(i64::from),
                e.status_text,
                e.timing.as_ref().map(|t| t.total_ms),
                e.error_kind,
                e.error,
                i64::from(e.response_size),
                i64::from(e.response_binary),
                i64::from(e.response_body_truncated),
                e.response_content_type,
                input_json,
                masked,
                overrides,
                request_headers,
                extra_json,
                timing_json,
                response_headers,
                e.request_body,
                e.response_body,
            ],
        )?;
        if max_entries > 0 {
            // `max_entries` 番目の次の行（これ以降が古い）を索引でたどって探し、それ以前をまとめて消す
            let cut: Option<(String, i64)> = tx
                .query_row(
                    "SELECT sent_at, seq FROM history WHERE project_id = ?1
                     ORDER BY sent_at DESC, seq DESC LIMIT 1 OFFSET ?2",
                    params![e.project_id, i64::from(max_entries)],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((at, seq)) = cut {
                trimmed = tx.execute(
                    "DELETE FROM history WHERE project_id = ?1
                     AND (sent_at < ?2 OR (sent_at = ?2 AND seq <= ?3))",
                    params![e.project_id, at, seq],
                )?;
            }
        }
        tx.commit()?;
        // 件数の上限で消した行の文字を WAL に残さない（手動の削除と同じ）。消したときだけなので、毎回は走らない
        if trimmed > 0 {
            Self::truncate_wal(&conn);
        }
        Ok(())
    }

    /// 履歴 1 件（全部）。
    pub fn history_get(&self, id: &str) -> Result<Option<HistoryEntry>> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT id, project_id, sent_at, environment_id, environment_name, op_key, op_summary,
                        method, url, final_url, status, status_text, error_kind, error,
                        response_size, response_binary, response_body_truncated, response_content_type,
                        input_json, masked_fields_json, override_names_json,
                        request_headers_json, request_extra_json, timing_json, response_headers_json,
                        request_body, response_body
                 FROM history WHERE id = ?1",
                [id],
                |r| {
                    Ok(RawEntry {
                        id: r.get(0)?,
                        project_id: r.get(1)?,
                        sent_at: r.get(2)?,
                        environment_id: r.get(3)?,
                        environment_name: r.get(4)?,
                        op_key: r.get(5)?,
                        op_summary: r.get(6)?,
                        method: r.get(7)?,
                        url: r.get(8)?,
                        final_url: r.get(9)?,
                        status: r.get(10)?,
                        status_text: r.get(11)?,
                        error_kind: r.get(12)?,
                        error: r.get(13)?,
                        response_size: r.get(14)?,
                        response_binary: r.get(15)?,
                        response_body_truncated: r.get(16)?,
                        response_content_type: r.get(17)?,
                        input_json: r.get(18)?,
                        masked_fields_json: r.get(19)?,
                        override_names_json: r.get(20)?,
                        request_headers_json: r.get(21)?,
                        request_extra_json: r.get(22)?,
                        timing_json: r.get(23)?,
                        response_headers_json: r.get(24)?,
                        request_body: r.get(25)?,
                        response_body: r.get(26)?,
                    })
                },
            )
            .optional()?;
        row.map(RawEntry::finish).transpose()
    }

    /// 一覧用の軽い項目（比較の見出しに使う）。
    pub fn history_item(&self, id: &str) -> Result<Option<HistoryListItem>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!("SELECT {LIST_COLUMNS} FROM history h WHERE h.id = ?1"),
                [id],
                list_item,
            )
            .optional()?
            .map(|(_, item)| item))
    }

    /// 一覧（新しい順）。`f.limit` 件まで。続きがあれば `next_cursor` を返す。
    pub fn history_list(&self, project_id: &str, f: &HistoryFilter) -> Result<HistoryRows> {
        let conn = self.conn();
        let limit = i64::from(f.limit.clamp(1, 500));

        let (count_where, count_args) = where_clause(project_id, f, false);
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM history h WHERE {count_where}"),
            params_from_iter(count_args.iter()),
            |r| r.get(0),
        )?;

        let (page_where, mut page_args) = where_clause(project_id, f, true);
        page_args.push(Value::Integer(limit + 1));
        let mut stmt = conn.prepare(&format!(
            "SELECT {LIST_COLUMNS} FROM history h WHERE {page_where}
             ORDER BY h.sent_at DESC, h.seq DESC LIMIT ?"
        ))?;
        let mut rows = stmt
            .query_map(params_from_iter(page_args.iter()), list_item)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_cursor = if rows.len() as i64 > limit {
            rows.truncate(limit as usize);
            rows.last().map(|(seq, i)| format!("{}|{seq}", i.sent_at))
        } else {
            None
        };
        Ok(HistoryRows {
            items: rows.into_iter().map(|(_, i)| i).collect(),
            total: u32::try_from(total).unwrap_or(u32::MAX),
            next_cursor,
        })
    }

    /// プロジェクトの履歴の件数。
    pub fn history_count(&self, project_id: &str) -> Result<u32> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM history WHERE project_id = ?1",
            [project_id],
            |r| r.get(0),
        )?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    /// 1 件消す（全文検索の行も消える）。なければ `NotFound`。
    pub fn history_delete(&self, id: &str) -> Result<()> {
        let conn = self.conn();
        let changed = conn.execute("DELETE FROM history WHERE id = ?1", [id])?;
        if changed == 0 {
            return Err(StorageError::NotFound {
                what: "履歴",
                id: id.to_string(),
            });
        }
        Self::truncate_wal(&conn);
        Ok(())
    }

    /// プロジェクトの履歴をすべて消す。消した件数を返す。
    pub fn history_clear(&self, project_id: &str) -> Result<u32> {
        let conn = self.conn();
        let n = conn.execute("DELETE FROM history WHERE project_id = ?1", [project_id])?;
        Self::truncate_wal(&conn);
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}

/// 行の中身（JSON はまだ読んでいない）。
struct RawEntry {
    id: String,
    project_id: String,
    sent_at: String,
    environment_id: Option<String>,
    environment_name: Option<String>,
    op_key: Option<String>,
    op_summary: Option<String>,
    method: String,
    url: String,
    final_url: Option<String>,
    status: Option<i64>,
    status_text: Option<String>,
    error_kind: Option<String>,
    error: Option<String>,
    response_size: i64,
    response_binary: i64,
    response_body_truncated: i64,
    response_content_type: Option<String>,
    input_json: Option<String>,
    masked_fields_json: String,
    override_names_json: String,
    request_headers_json: String,
    request_extra_json: String,
    timing_json: Option<String>,
    response_headers_json: String,
    request_body: Option<String>,
    response_body: Option<String>,
}

impl RawEntry {
    fn finish(self) -> Result<HistoryEntry> {
        let extra: RequestExtra = serde_json::from_str(&self.request_extra_json)
            .map_err(|e| corrupt("request_extra_json", e))?;
        Ok(HistoryEntry {
            id: self.id,
            project_id: self.project_id,
            sent_at: self.sent_at,
            environment_id: self.environment_id,
            environment_name: self.environment_name,
            op_key: op_key_of(self.op_key),
            op_summary: self.op_summary,
            method: self.method,
            url: self.url,
            final_url: self.final_url,
            request_headers: serde_json::from_str::<Vec<HeaderPair>>(&self.request_headers_json)
                .map_err(|e| corrupt("request_headers_json", e))?,
            request_body: self.request_body,
            request_body_kind: extra.kind,
            request_body_size: extra.size,
            request_body_truncated: extra.truncated,
            request_parts: extra.parts,
            request_file: extra.file,
            status: self.status.and_then(|s| u16::try_from(s).ok()),
            status_text: self.status_text,
            response_headers: serde_json::from_str::<Vec<HeaderPair>>(&self.response_headers_json)
                .map_err(|e| corrupt("response_headers_json", e))?,
            response_content_type: self.response_content_type,
            response_body: self.response_body,
            response_binary: self.response_binary != 0,
            response_body_truncated: self.response_body_truncated != 0,
            response_size: u32::try_from(self.response_size).unwrap_or(u32::MAX),
            timing: self
                .timing_json
                .map(|j| serde_json::from_str::<Timing>(&j).map_err(|e| corrupt("timing_json", e)))
                .transpose()?,
            error: self.error,
            error_kind: self.error_kind,
            input: self
                .input_json
                .map(|j| {
                    serde_json::from_str::<RequestSpec>(&j).map_err(|e| corrupt("input_json", e))
                })
                .transpose()?,
            masked_fields: serde_json::from_str(&self.masked_fields_json)
                .map_err(|e| corrupt("masked_fields_json", e))?,
            override_names: serde_json::from_str(&self.override_names_json)
                .map_err(|e| corrupt("override_names_json", e))?,
        })
    }
}
