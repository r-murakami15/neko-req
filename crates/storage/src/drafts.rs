//! 入力値（RequestDraft）のリポジトリ関数。Operation ごと（`project_id` + `op_key`）に 1 つ。
//! 画面での使い方は M3（OpenAPI 取り込み）以降。
//!
//! Operation が yaml から消えても残す（`operations` への外部キーは張っていない）。

use nekoreq_core::{new_id, AuthOverride, Body, KeyValue, OpKey, RequestDraft};
use rusqlite::{params, OptionalExtension};

use crate::error::{Result, StorageError};
use crate::{now, Database};

fn corrupt(what: &str, e: impl std::fmt::Display) -> StorageError {
    StorageError::Corrupt(format!("request_drafts.{what}: {e}"))
}

impl Database {
    /// 保存する（`project_id` + `op_key` が同じものがあれば置き換える。ID は最初のものを保つ）。
    /// 更新日時はいまの時刻にする。保存した内容を返す。
    pub fn draft_save(&self, draft: &RequestDraft) -> Result<RequestDraft> {
        let params_json =
            serde_json::to_string(&draft.params).map_err(|e| corrupt("params_json", e))?;
        let body_json = serde_json::to_string(&draft.body).map_err(|e| corrupt("body_json", e))?;
        let auth_json = draft
            .auth
            .as_ref()
            .map(|a| serde_json::to_string(a).map_err(|e| corrupt("auth_json", e)))
            .transpose()?;
        let key = draft.op_key.to_key_string();

        let conn = self.conn();
        let project_exists = conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                [&draft.project_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !project_exists {
            return Err(StorageError::NotFound {
                what: "プロジェクト",
                id: draft.project_id.clone(),
            });
        }
        let id = if draft.id.is_empty() {
            new_id()
        } else {
            draft.id.clone()
        };
        conn.execute(
            "INSERT INTO request_drafts (id, project_id, op_key, params_json, body_json, auth_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(project_id, op_key) DO UPDATE SET
                 params_json = excluded.params_json, body_json = excluded.body_json,
                 auth_json = excluded.auth_json, updated_at = excluded.updated_at",
            params![id, draft.project_id, key, params_json, body_json, auth_json, now()],
        )?;
        // 入力値から外した（消した）パラメータは「yaml にない」の印も外す
        let keep: Vec<_> = draft
            .params
            .iter()
            .map(|p| (p.location, p.name.clone()))
            .collect();
        crate::operations::prune_stale(&conn, &draft.project_id, &key, &keep)?;
        drop(conn);
        self.draft_get(&draft.project_id, &draft.op_key)?
            .ok_or_else(|| StorageError::NotFound {
                what: "入力値",
                id: key,
            })
    }

    /// Operation の入力値を返す。なければ `None`。
    pub fn draft_get(&self, project_id: &str, op_key: &OpKey) -> Result<Option<RequestDraft>> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT id, params_json, body_json, auth_json, updated_at
                 FROM request_drafts WHERE project_id = ?1 AND op_key = ?2",
                params![project_id, op_key.to_key_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, params_json, body_json, auth_json, updated_at)) = row else {
            return Ok(None);
        };
        Ok(Some(RequestDraft {
            id,
            project_id: project_id.to_string(),
            op_key: op_key.clone(),
            params: serde_json::from_str::<Vec<KeyValue>>(&params_json)
                .map_err(|e| corrupt("params_json", e))?,
            body: serde_json::from_str::<Body>(&body_json).map_err(|e| corrupt("body_json", e))?,
            auth: auth_json
                .map(|j| {
                    serde_json::from_str::<AuthOverride>(&j).map_err(|e| corrupt("auth_json", e))
                })
                .transpose()?,
            updated_at,
        }))
    }
}
