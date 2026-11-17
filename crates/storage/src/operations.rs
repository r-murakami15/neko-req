//! 取り込み元（`api_sources`）と Operation のキャッシュ（`operations`）のリポジトリ関数。
//!
//! - 同期の反映（`operations_apply`）は 1 つのトランザクションで行う。RequestDraft は消さない。
//! - yaml から消えた Operation は、入力値があれば `deleted_at` を付けて残す（「削除された API」）。なければ行を消す。
//! - operationId が変わった Operation は `METHOD path` が同じものとして引き継ぎ、入力値のキーも付け替える。
//! - yaml から消えたパラメータは、入力値の `stale_params_json` に覚える（値は残す）。

use std::collections::HashSet;

use nekoreq_core::{
    match_operations, new_id, ApiSource, OpKey, Operation, ParamIn, ParamSpec, RequestBodySpec,
    ResponseSpec, SecurityRequirement, SourceFile, SourceKind,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StorageError};
use crate::{now, Database};

/// 保存されている Operation と、その状態。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredOperation {
    pub operation: Operation,
    /// 確認画面で選んで取り込んだ（ツリーに出す）。
    pub imported: bool,
    /// yaml から消えた日時（入力値が残っているので「削除された API」として出す）。
    pub deleted_at: Option<String>,
}

/// yaml から消えたパラメータ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleParam {
    pub location: ParamIn,
    pub name: String,
}

/// 新しく見つかった Operation を取り込み済みにするか。
#[derive(Debug, Clone)]
pub enum Selection {
    /// 指定のキーだけ（新規のうち。既存の取り込み済みはそのまま）。
    Only(HashSet<OpKey>),
    /// 新しいものはすべて。
    AllNew,
}

impl Selection {
    fn includes(&self, key: &OpKey) -> bool {
        match self {
            Selection::Only(s) => s.contains(key),
            Selection::AllNew => true,
        }
    }
}

/// 反映の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub added: u32,
    pub changed: u32,
    /// yaml から消えた数。
    pub removed: u32,
    /// そのうち入力値が残っていて「削除された API」に移したもの。
    pub kept_as_deleted: u32,
    /// operationId の付け替えで入力値のキーを引き継いだ数。
    pub renamed: u32,
}

fn corrupt(what: &str, e: impl std::fmt::Display) -> StorageError {
    StorageError::Corrupt(format!("operations.{what}: {e}"))
}

fn kind_str(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Path => "path",
        SourceKind::Url => "url",
        SourceKind::Copy => "copy",
    }
}

/// 保存した JSON を読み戻す。スキーマの入れ子が深くても読めるように、再帰の上限を外し、スタックは足りなければ
/// 伸ばす（保存できたものは必ず読み戻せる）。
fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> serde_json::Result<T> {
    let mut de = serde_json::Deserializer::from_str(s);
    de.disable_recursion_limit();
    let v = T::deserialize(serde_stacker::Deserializer::new(&mut de))?;
    de.end()?;
    Ok(v)
}

fn to_json<T: Serialize>(v: &T, what: &str) -> Result<String> {
    serde_json::to_string(v).map_err(|e| corrupt(what, e))
}

const OP_COLUMNS: &str =
    "id, project_id, op_key, operation_id, method, path, tags_json, summary, description,
    parameters_json, request_body_json, responses_json, security_json, source_file, source_line,
    content_hash, imported, deleted_at";

fn stored_from_row(row: &Row<'_>) -> Result<StoredOperation> {
    let get = |i: usize| -> rusqlite::Result<String> { row.get(i) };
    let op_key_s: String = get(2)?;
    let op_key = OpKey::parse(&op_key_s).map_err(|e| corrupt("op_key", e))?;
    let tags_json: String = get(6)?;
    let parameters_json: String = get(9)?;
    let request_body_json: Option<String> = row.get(10)?;
    let responses_json: String = get(11)?;
    let security_json: Option<String> = row.get(12)?;
    let source_line: Option<i64> = row.get(14)?;
    Ok(StoredOperation {
        operation: Operation {
            id: get(0)?,
            project_id: get(1)?,
            op_key,
            operation_id: row.get(3)?,
            method: get(4)?,
            path: get(5)?,
            tags: serde_json::from_str(&tags_json).map_err(|e| corrupt("tags_json", e))?,
            summary: row.get(7)?,
            description: row.get(8)?,
            parameters: from_json::<Vec<ParamSpec>>(&parameters_json)
                .map_err(|e| corrupt("parameters_json", e))?,
            request_body: request_body_json
                .map(|j| from_json::<RequestBodySpec>(&j))
                .transpose()
                .map_err(|e| corrupt("request_body_json", e))?,
            responses: from_json::<Vec<ResponseSpec>>(&responses_json)
                .map_err(|e| corrupt("responses_json", e))?,
            security: security_json
                .map(|j| serde_json::from_str::<Vec<SecurityRequirement>>(&j))
                .transpose()
                .map_err(|e| corrupt("security_json", e))?,
            source_file: row.get(13)?,
            source_line: source_line.and_then(|l| u32::try_from(l).ok()),
            content_hash: get(15)?,
        },
        imported: row.get(16)?,
        deleted_at: row.get(17)?,
    })
}

fn load_operations(conn: &Connection, project_id: &str) -> Result<Vec<StoredOperation>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {OP_COLUMNS} FROM operations WHERE project_id = ?1 ORDER BY rowid"
    ))?;
    let mut rows = stmt.query([project_id])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(stored_from_row(row)?);
    }
    Ok(out)
}

fn project_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM projects WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn upsert_source(conn: &Connection, project_id: &str, source: &ApiSource) -> Result<()> {
    let id = conn
        .query_row(
            "SELECT id FROM api_sources WHERE project_id = ?1",
            [project_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_else(|| {
            if source.id.is_empty() {
                new_id()
            } else {
                source.id.clone()
            }
        });
    conn.execute(
        "INSERT INTO api_sources
            (id, project_id, kind, entry, files_json, openapi_version, synced_at,
             allowed_hosts_json, has_token, options_json, meta_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(project_id) DO UPDATE SET
            kind = excluded.kind, entry = excluded.entry, files_json = excluded.files_json,
            openapi_version = excluded.openapi_version, synced_at = excluded.synced_at,
            allowed_hosts_json = excluded.allowed_hosts_json, has_token = excluded.has_token,
            options_json = excluded.options_json, meta_json = excluded.meta_json",
        params![
            id,
            project_id,
            kind_str(source.kind),
            source.entry,
            to_json(&source.files, "files_json")?,
            source.openapi_version,
            source.synced_at,
            to_json(&source.allowed_hosts, "allowed_hosts_json")?,
            source.has_token,
            to_json(&source.options, "options_json")?,
            to_json(&source.meta, "meta_json")?,
        ],
    )?;
    Ok(())
}

fn write_operation(
    conn: &Connection,
    id: &str,
    project_id: &str,
    op: &Operation,
    imported: Option<bool>,
    insert: bool,
) -> Result<()> {
    let tags = to_json(&op.tags, "tags_json")?;
    let parameters = to_json(&op.parameters, "parameters_json")?;
    let body = op
        .request_body
        .as_ref()
        .map(|b| to_json(b, "request_body_json"))
        .transpose()?;
    let responses = to_json(&op.responses, "responses_json")?;
    let security = op
        .security
        .as_ref()
        .map(|s| to_json(s, "security_json"))
        .transpose()?;
    let key = op.op_key.to_key_string();
    let line = op.source_line.map(i64::from);
    if insert {
        conn.execute(
            "INSERT INTO operations
                (id, project_id, op_key, operation_id, method, path, tags_json, summary, description,
                 parameters_json, request_body_json, responses_json, security_json,
                 source_file, source_line, content_hash, imported, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, NULL)",
            params![
                id,
                project_id,
                key,
                op.operation_id,
                op.method,
                op.path,
                tags,
                op.summary,
                op.description,
                parameters,
                body,
                responses,
                security,
                op.source_file,
                line,
                op.content_hash,
                imported.unwrap_or(true),
            ],
        )?;
    } else {
        conn.execute(
            "UPDATE operations SET
                op_key = ?2, operation_id = ?3, method = ?4, path = ?5, tags_json = ?6, summary = ?7,
                description = ?8, parameters_json = ?9, request_body_json = ?10, responses_json = ?11,
                security_json = ?12, source_file = ?13, source_line = ?14, content_hash = ?15,
                deleted_at = NULL, imported = coalesce(?16, imported)
             WHERE id = ?1",
            params![
                id,
                key,
                op.operation_id,
                op.method,
                op.path,
                tags,
                op.summary,
                op.description,
                parameters,
                body,
                responses,
                security,
                op.source_file,
                line,
                op.content_hash,
                imported,
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn stale_of(
    conn: &Connection,
    project_id: &str,
    key: &str,
) -> Result<Option<Vec<StaleParam>>> {
    let json = conn
        .query_row(
            "SELECT stale_params_json FROM request_drafts WHERE project_id = ?1 AND op_key = ?2",
            params![project_id, key],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    json.map(|j| serde_json::from_str(&j).map_err(|e| corrupt("stale_params_json", e)))
        .transpose()
}

/// 保存した入力値に残っていないパラメータの印を外す（`draft_save` から呼ぶ）。
pub(crate) fn prune_stale(
    conn: &Connection,
    project_id: &str,
    key: &str,
    keep: &[(ParamIn, String)],
) -> Result<()> {
    let Some(mut stale) = stale_of(conn, project_id, key)? else {
        return Ok(());
    };
    let before = stale.len();
    stale.retain(|s| keep.iter().any(|(l, n)| *l == s.location && *n == s.name));
    if stale.len() != before {
        conn.execute(
            "UPDATE request_drafts SET stale_params_json = ?3 WHERE project_id = ?1 AND op_key = ?2",
            params![project_id, key, to_json(&stale, "stale_params_json")?],
        )?;
    }
    Ok(())
}

/// 入力値があれば、yaml から消えたパラメータの印を更新する（消えたものを足し、戻ってきたものを外す）。
fn update_stale(
    conn: &Connection,
    project_id: &str,
    key: &OpKey,
    old: &Operation,
    new: &Operation,
) -> Result<()> {
    let key_s = key.to_key_string();
    let Some(mut stale) = stale_of(conn, project_id, &key_s)? else {
        return Ok(());
    };
    let in_new = |l: ParamIn, n: &str| {
        new.parameters
            .iter()
            .any(|p| p.location == l && p.name == n)
    };
    for p in &old.parameters {
        if !in_new(p.location, &p.name)
            && !stale
                .iter()
                .any(|s| s.location == p.location && s.name == p.name)
        {
            stale.push(StaleParam {
                location: p.location,
                name: p.name.clone(),
            });
        }
    }
    stale.retain(|s| !in_new(s.location, &s.name));
    conn.execute(
        "UPDATE request_drafts SET stale_params_json = ?3 WHERE project_id = ?1 AND op_key = ?2",
        params![project_id, key_s, to_json(&stale, "stale_params_json")?],
    )?;
    Ok(())
}

impl Database {
    /// 取り込み元だけを保存する（Operation は変えない）。更新日時・ハッシュの更新に使う。
    pub fn source_save(&self, project_id: &str, source: &ApiSource) -> Result<()> {
        let conn = self.conn();
        if !project_exists(&conn, project_id)? {
            return Err(StorageError::NotFound {
                what: "プロジェクト",
                id: project_id.into(),
            });
        }
        upsert_source(&conn, project_id, source)
    }

    /// 取り込み元の `files`（更新日時・ハッシュ）と `synced_at` だけを更新する。
    pub fn source_touch_files(&self, project_id: &str, files: &[SourceFile]) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE api_sources SET files_json = ?2 WHERE project_id = ?1",
            params![project_id, to_json(&files, "files_json")?],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound {
                what: "取り込み元",
                id: project_id.into(),
            });
        }
        Ok(())
    }

    /// 確認済みの別ホスト（origin）の一覧を保存する（同期で確認したホストを覚える）。
    pub fn source_set_allowed_hosts(&self, project_id: &str, hosts: &[String]) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE api_sources SET allowed_hosts_json = ?2 WHERE project_id = ?1",
            params![project_id, to_json(&hosts, "allowed_hosts_json")?],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound {
                what: "取り込み元",
                id: project_id.into(),
            });
        }
        Ok(())
    }

    /// URL 方式のアクセストークンの有無を記録する。
    pub fn source_set_has_token(&self, project_id: &str, has_token: bool) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE api_sources SET has_token = ?2 WHERE project_id = ?1",
            params![project_id, has_token],
        )?;
        if n == 0 {
            return Err(StorageError::NotFound {
                what: "取り込み元",
                id: project_id.into(),
            });
        }
        Ok(())
    }

    /// プロジェクトの Operation（取り込んでいないもの・消えたものを含む。取り込んだ順）。
    pub fn operations_list(&self, project_id: &str) -> Result<Vec<StoredOperation>> {
        load_operations(&self.conn(), project_id)
    }

    pub fn operation_get(&self, project_id: &str, key: &OpKey) -> Result<Option<StoredOperation>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {OP_COLUMNS} FROM operations WHERE project_id = ?1 AND op_key = ?2"
        ))?;
        let mut rows = stmt.query(params![project_id, key.to_key_string()])?;
        match rows.next()? {
            Some(row) => Ok(Some(stored_from_row(row)?)),
            None => Ok(None),
        }
    }

    /// 入力値（RequestDraft）を持つ Operation のキー。
    pub fn draft_keys(&self, project_id: &str) -> Result<HashSet<OpKey>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT op_key FROM request_drafts WHERE project_id = ?1")?;
        let keys = stmt
            .query_map([project_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        keys.into_iter()
            .map(|k| OpKey::parse(&k).map_err(|e| corrupt("request_drafts.op_key", e)))
            .collect()
    }

    /// yaml から消えたパラメータ（入力値がなければ空）。
    pub fn draft_stale_params(&self, project_id: &str, key: &OpKey) -> Result<Vec<StaleParam>> {
        Ok(stale_of(&self.conn(), project_id, &key.to_key_string())?.unwrap_or_default())
    }

    /// 「削除された API」を自分で消す（入力値も消える）。まだ yaml にある API は消せない。
    pub fn operation_forget(&self, project_id: &str, key: &OpKey) -> Result<()> {
        let mut conn = self.conn();
        let key_s = key.to_key_string();
        let deleted = conn
            .query_row(
                "SELECT deleted_at IS NOT NULL FROM operations WHERE project_id = ?1 AND op_key = ?2",
                params![project_id, key_s],
                |r| r.get::<_, bool>(0),
            )
            .optional()?;
        if deleted == Some(false) {
            return Err(StorageError::Invalid(
                "yaml にまだある API は消せません".into(),
            ));
        }
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM operations WHERE project_id = ?1 AND op_key = ?2",
            params![project_id, key_s],
        )?;
        tx.execute(
            "DELETE FROM request_drafts WHERE project_id = ?1 AND op_key = ?2",
            params![project_id, key_s],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 取り込み元と Operation を反映する。RequestDraft は消さない。
    ///
    /// - `new_ops` は yaml から読んだ今回の全 Operation（`id`・`project_id` は無視して付け直す）。
    /// - 同じ `op_key`、または `METHOD path` が同じものを同じ API とみなして更新する。operationId が変わった
    ///   ものは入力値のキーも付け替える。
    /// - 消えた Operation は、入力値があれば `deleted_at` を付けて残し、なければ行を消す。
    /// - 新しく見つかったものを取り込み済みにするかは `selection`。
    pub fn operations_apply(
        &self,
        project_id: &str,
        source: &ApiSource,
        new_ops: &[Operation],
        selection: &Selection,
    ) -> Result<ApplyReport> {
        let mut conn = self.conn();
        if !project_exists(&conn, project_id)? {
            return Err(StorageError::NotFound {
                what: "プロジェクト",
                id: project_id.into(),
            });
        }
        let tx = conn.transaction()?;
        upsert_source(&tx, project_id, source)?;

        let old = load_operations(&tx, project_id)?;
        let old_ops: Vec<Operation> = old.iter().map(|s| s.operation.clone()).collect();
        let m = match_operations(&old_ops, new_ops);
        let mut report = ApplyReport::default();
        let ts = now();

        for &(i, j) in &m.pairs {
            let (o, n) = (&old[i], &new_ops[j]);
            let renamed = o.operation.op_key != n.op_key;
            let changed =
                renamed || o.operation.content_hash != n.content_hash || o.operation.tags != n.tags;
            // 既存の取り込み済みはそのまま。選ばれていた（Only）ものは取り込み済みにする
            let imported = match selection {
                Selection::Only(s) if s.contains(&n.op_key) => Some(true),
                _ => None,
            };
            if renamed {
                // 入力値のキーを付け替える（付け替え先に入力値があるときはそのまま残す）
                let moved = tx.execute(
                    "UPDATE request_drafts SET op_key = ?3 WHERE project_id = ?1 AND op_key = ?2
                       AND NOT EXISTS (SELECT 1 FROM request_drafts d2 WHERE d2.project_id = ?1 AND d2.op_key = ?3)",
                    params![project_id, o.operation.op_key.to_key_string(), n.op_key.to_key_string()],
                )?;
                if moved > 0 {
                    report.renamed += 1;
                }
            }
            write_operation(&tx, &o.operation.id, project_id, n, imported, false)?;
            update_stale(&tx, project_id, &n.op_key, &o.operation, n)?;
            // yaml に戻ってきた API（削除済みだったもの）は、同期の差分と同じく「追加」と数える
            if o.deleted_at.is_some() {
                report.added += 1;
            } else if changed {
                report.changed += 1;
            }
        }

        for &j in &m.added {
            let n = &new_ops[j];
            write_operation(
                &tx,
                &new_id(),
                project_id,
                n,
                Some(selection.includes(&n.op_key)),
                true,
            )?;
            report.added += 1;
        }

        for &i in &m.removed {
            let o = &old[i];
            if o.deleted_at.is_some() {
                continue;
            }
            report.removed += 1;
            let key = o.operation.op_key.to_key_string();
            let has_draft = stale_of(&tx, project_id, &key)?.is_some();
            if has_draft {
                tx.execute(
                    "UPDATE operations SET deleted_at = ?2 WHERE id = ?1",
                    params![o.operation.id, ts],
                )?;
                report.kept_as_deleted += 1;
            } else {
                tx.execute("DELETE FROM operations WHERE id = ?1", [&o.operation.id])?;
            }
        }
        tx.commit()?;
        Ok(report)
    }
}
