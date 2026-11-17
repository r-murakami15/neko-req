//! 手で作ったリクエスト（AdHocRequest。「その他」フォルダに入る）のリポジトリ関数。
//!
//! 値には `{{変数}}` を書く。シークレットの実体はここに保存しない。

use nekoreq_core::{new_id, AdHocRequest, AuthOverride, Body, KeyValue};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::error::{Result, StorageError};
use crate::{now, Database};

/// リクエスト名の最大文字数。
pub const MAX_REQUEST_NAME_CHARS: usize = 200;

fn not_found(id: &str) -> StorageError {
    StorageError::NotFound {
        what: "リクエスト",
        id: id.to_string(),
    }
}

fn corrupt(what: &str, e: serde_json::Error) -> StorageError {
    StorageError::Corrupt(format!("adhoc_requests.{what}: {e}"))
}

const COLUMNS: &str = "id, project_id, name, description, method, url, params_json, body_json, auth_json, created_at, updated_at";

fn adhoc_from_row(
    row: &Row<'_>,
) -> rusqlite::Result<(AdHocRequest, String, String, Option<String>)> {
    Ok((
        AdHocRequest {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            name: row.get("name")?,
            description: row.get("description")?,
            method: row.get("method")?,
            url: row.get("url")?,
            params: Vec::new(),
            body: Body::None,
            auth: None,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        },
        row.get("params_json")?,
        row.get("body_json")?,
        row.get("auth_json")?,
    ))
}

fn finish(
    (mut req, params_json, body_json, auth_json): (AdHocRequest, String, String, Option<String>),
) -> Result<AdHocRequest> {
    req.params = serde_json::from_str::<Vec<KeyValue>>(&params_json)
        .map_err(|e| corrupt("params_json", e))?;
    req.body = serde_json::from_str::<Body>(&body_json).map_err(|e| corrupt("body_json", e))?;
    req.auth = auth_json
        .map(|j| serde_json::from_str::<AuthOverride>(&j).map_err(|e| corrupt("auth_json", e)))
        .transpose()?;
    Ok(req)
}

fn get(conn: &Connection, id: &str) -> Result<Option<AdHocRequest>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM adhoc_requests WHERE id = ?1"),
        [id],
        adhoc_from_row,
    )
    .optional()?
    .map(finish)
    .transpose()
}

impl Database {
    /// プロジェクトの AdHocRequest を作った順に返す。
    pub fn adhoc_list(&self, project_id: &str) -> Result<Vec<AdHocRequest>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM adhoc_requests WHERE project_id = ?1 ORDER BY created_at, rowid"
        ))?;
        let rows = stmt
            .query_map([project_id], adhoc_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(finish).collect()
    }

    pub fn adhoc_get(&self, id: &str) -> Result<Option<AdHocRequest>> {
        get(&self.conn(), id)
    }

    /// 保存する（なければ作り、あれば更新する）。`id` が空なら新しい ID をつける。
    /// 作成日時は最初のものを保ち、更新日時はいまの時刻にする。保存した内容を返す。
    pub fn adhoc_save(&self, req: &AdHocRequest) -> Result<AdHocRequest> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(StorageError::InvalidName(
                "リクエスト名を入力してください".into(),
            ));
        }
        if name.chars().count() > MAX_REQUEST_NAME_CHARS {
            return Err(StorageError::InvalidName(format!(
                "リクエスト名は {MAX_REQUEST_NAME_CHARS} 文字以内にしてください"
            )));
        }
        let method = req.method.trim().to_ascii_uppercase();
        if method.is_empty() {
            return Err(StorageError::Invalid("method を指定してください".into()));
        }
        let params_json =
            serde_json::to_string(&req.params).map_err(|e| corrupt("params_json", e))?;
        let body_json = serde_json::to_string(&req.body).map_err(|e| corrupt("body_json", e))?;
        let auth_json = req
            .auth
            .as_ref()
            .map(|a| serde_json::to_string(a).map_err(|e| corrupt("auth_json", e)))
            .transpose()?;

        let conn = self.conn();
        let project_exists = conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                [&req.project_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !project_exists {
            return Err(StorageError::NotFound {
                what: "プロジェクト",
                id: req.project_id.clone(),
            });
        }

        let ts = now();
        let id = if req.id.is_empty() {
            new_id()
        } else {
            req.id.clone()
        };
        conn.execute(
            "INSERT INTO adhoc_requests (id, project_id, name, description, method, url, params_json, body_json, auth_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?10, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(id) DO UPDATE SET
                 project_id = excluded.project_id, name = excluded.name, description = excluded.description,
                 method = excluded.method,
                 url = excluded.url, params_json = excluded.params_json, body_json = excluded.body_json,
                 auth_json = excluded.auth_json, updated_at = excluded.updated_at",
            params![id, req.project_id, name, method, req.url, params_json, body_json, auth_json, ts, req.description],
        )?;
        get(&conn, &id)?.ok_or_else(|| not_found(&id))
    }

    pub fn adhoc_delete(&self, id: &str) -> Result<()> {
        let changed = self
            .conn()
            .execute("DELETE FROM adhoc_requests WHERE id = ?1", [id])?;
        if changed == 0 {
            return Err(not_found(id));
        }
        Ok(())
    }
}
