//! プロジェクトのリポジトリ関数。

use nekoreq_core::{new_id, ApiSource, Project, ProjectSettings, SourceFile, SourceKind};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::error::{Result, StorageError};
use crate::{now, Database};

/// プロジェクト名の最大文字数。
pub const MAX_PROJECT_NAME_CHARS: usize = 100;

fn normalize_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StorageError::InvalidName(
            "プロジェクト名を入力してください".into(),
        ));
    }
    if name.chars().count() > MAX_PROJECT_NAME_CHARS {
        return Err(StorageError::InvalidName(format!(
            "プロジェクト名は {MAX_PROJECT_NAME_CHARS} 文字以内にしてください"
        )));
    }
    Ok(name.to_string())
}

fn not_found(id: &str) -> StorageError {
    StorageError::NotFound {
        what: "プロジェクト",
        id: id.to_string(),
    }
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    let settings_json: String = row.get("settings_json")?;
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        source: None,
        default_environment_id: row.get("default_environment_id")?,
        // 読めない設定は既定（どちらもオン）で扱う
        settings: serde_json::from_str(&settings_json).unwrap_or_default(),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn source_kind_from_str(s: &str) -> Result<SourceKind> {
    match s {
        "path" => Ok(SourceKind::Path),
        "url" => Ok(SourceKind::Url),
        "copy" => Ok(SourceKind::Copy),
        other => Err(StorageError::Corrupt(format!("api_sources.kind: {other}"))),
    }
}

fn load_source(conn: &Connection, project_id: &str) -> Result<Option<ApiSource>> {
    let row = conn
        .query_row(
            "SELECT id, kind, entry, files_json, openapi_version, synced_at,
                    allowed_hosts_json, has_token, options_json, meta_json
             FROM api_sources WHERE project_id = ?1",
            [project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id,
        kind,
        entry,
        files_json,
        openapi_version,
        synced_at,
        hosts_json,
        has_token,
        options_json,
        meta_json,
    )) = row
    else {
        return Ok(None);
    };
    let corrupt =
        |col: &str, e: serde_json::Error| StorageError::Corrupt(format!("api_sources.{col}: {e}"));
    Ok(Some(ApiSource {
        id,
        kind: source_kind_from_str(&kind)?,
        entry,
        files: serde_json::from_str::<Vec<SourceFile>>(&files_json)
            .map_err(|e| corrupt("files_json", e))?,
        openapi_version,
        synced_at,
        has_token,
        allowed_hosts: serde_json::from_str(&hosts_json)
            .map_err(|e| corrupt("allowed_hosts_json", e))?,
        options: serde_json::from_str(&options_json).map_err(|e| corrupt("options_json", e))?,
        meta: serde_json::from_str(&meta_json).map_err(|e| corrupt("meta_json", e))?,
    }))
}

pub(crate) fn get_project(conn: &Connection, id: &str) -> Result<Option<Project>> {
    let project = conn
        .query_row(
            "SELECT id, name, default_environment_id, settings_json, created_at, updated_at
             FROM projects WHERE id = ?1",
            [id],
            project_from_row,
        )
        .optional()?;
    match project {
        Some(mut p) => {
            p.source = load_source(conn, &p.id)?;
            Ok(Some(p))
        }
        None => Ok(None),
    }
}

impl Database {
    /// プロジェクトを作る（「グローバル」環境も 1 つ作る）。名前は前後の空白を除き、空や 100 文字超はエラー。
    pub fn project_create(&self, name: &str) -> Result<Project> {
        let name = normalize_name(name)?;
        let id = new_id();
        let ts = now();
        let mut conn = self.conn();
        // プロジェクトと「グローバル」環境は 1 組で作る
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO projects (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, name, ts],
        )?;
        crate::environments::insert_global(&tx, &id, &ts)?;
        tx.commit()?;
        Ok(Project {
            id,
            name,
            source: None,
            default_environment_id: None,
            settings: ProjectSettings::default(),
            created_at: ts.clone(),
            updated_at: ts,
        })
    }

    /// 作った順（古いものが先）に全プロジェクトを返す。
    pub fn project_list(&self) -> Result<Vec<Project>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, default_environment_id, settings_json, created_at, updated_at
             FROM projects ORDER BY created_at, rowid",
        )?;
        let mut projects = stmt
            .query_map([], project_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for p in &mut projects {
            p.source = load_source(&conn, &p.id)?;
        }
        Ok(projects)
    }

    /// ID でプロジェクトを 1 つ返す。なければ `None`。
    pub fn project_get(&self, id: &str) -> Result<Option<Project>> {
        get_project(&self.conn(), id)
    }

    /// 名前を変える。ID がなければ `NotFound`。
    pub fn project_rename(&self, id: &str, name: &str) -> Result<Project> {
        let name = normalize_name(name)?;
        let conn = self.conn();
        let changed = conn.execute(
            "UPDATE projects SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, now()],
        )?;
        if changed == 0 {
            return Err(not_found(id));
        }
        get_project(&conn, id)?.ok_or_else(|| not_found(id))
    }

    /// プロジェクトの設定（cookieAuth・CSRF の自動化のオン・オフ）を保存する。更新後のプロジェクトを返す。
    pub fn project_settings_set(&self, id: &str, settings: &ProjectSettings) -> Result<Project> {
        let conn = self.conn();
        let json = serde_json::to_string(settings)
            .map_err(|e| StorageError::Corrupt(format!("settings_json: {e}")))?;
        let changed = conn.execute(
            "UPDATE projects SET settings_json = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, json, now()],
        )?;
        if changed == 0 {
            return Err(not_found(id));
        }
        get_project(&conn, id)?.ok_or_else(|| not_found(id))
    }

    /// プロジェクトを消す。紐づく取り込み元・Operation・入力値・環境・履歴も一緒に消える。
    pub fn project_delete(&self, id: &str) -> Result<()> {
        let conn = self.conn();
        let changed = conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        if changed == 0 {
            return Err(not_found(id));
        }
        // 消した履歴・環境の内容を WAL に残さない
        Database::truncate_wal(&conn);
        Ok(())
    }
}
