//! 環境（`environments`）と環境変数（`env_vars`）のリポジトリ関数。
//!
//! - プロジェクトごとに「グローバル」環境（`is_global`）が 1 つある。選択・削除・改名はできない。
//! - 「選んでいる環境」は `projects.default_environment_id`（`Project.default_environment_id`）に保存する。
//! - シークレットの変数は名前・説明・有効だけを持ち、`value` は常に NULL（実体はキーチェーン）。

use nekoreq_core::{new_id, EnvVar, Environment, Project, VarValue, GLOBAL_ENVIRONMENT_NAME};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::error::{Result, StorageError};
use crate::projects::get_project;
use crate::{now, Database};

/// 環境名の最大文字数。
pub const MAX_ENV_NAME_CHARS: usize = 100;
/// 変数名の最大文字数。
pub const MAX_VAR_NAME_CHARS: usize = 100;

fn env_not_found(id: &str) -> StorageError {
    StorageError::NotFound {
        what: "環境",
        id: id.to_string(),
    }
}

fn project_not_found(id: &str) -> StorageError {
    StorageError::NotFound {
        what: "プロジェクト",
        id: id.to_string(),
    }
}

/// 「グローバル」でない環境の名前を整える。「グローバル」は特別な環境の名前なので付けられない。
fn normalize_env_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name == GLOBAL_ENVIRONMENT_NAME {
        return Err(StorageError::InvalidName(format!(
            "「{GLOBAL_ENVIRONMENT_NAME}」は特別な環境の名前なので付けられません"
        )));
    }
    if name.is_empty() {
        return Err(StorageError::InvalidName("環境名を入力してください".into()));
    }
    if name.chars().count() > MAX_ENV_NAME_CHARS {
        return Err(StorageError::InvalidName(format!(
            "環境名は {MAX_ENV_NAME_CHARS} 文字以内にしてください"
        )));
    }
    Ok(name.to_string())
}

/// 変数名を整える。空・空白や波括弧を含む・`$` から始まる（動的変数の名前）はエラー。
fn normalize_var_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StorageError::Invalid("変数名を入力してください".into()));
    }
    if name.chars().count() > MAX_VAR_NAME_CHARS {
        return Err(StorageError::Invalid(format!(
            "変数名は {MAX_VAR_NAME_CHARS} 文字以内にしてください: {name}"
        )));
    }
    if name.starts_with('$') {
        return Err(StorageError::Invalid(format!(
            "変数名は $ から始められません（動的変数の名前です）: {name}"
        )));
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c == '{' || c == '}')
    {
        return Err(StorageError::Invalid(format!(
            "変数名に空白と波括弧は使えません: {name}"
        )));
    }
    Ok(name.to_string())
}

/// プロジェクトの「グローバル」環境を作る（`project_create` と同じトランザクションで呼ぶ）。
pub(crate) fn insert_global(conn: &Connection, project_id: &str, ts: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at, is_global)
         VALUES (?1, ?2, ?3, 0, ?4, ?4, 1)",
        params![new_id(), project_id, GLOBAL_ENVIRONMENT_NAME, ts],
    )?;
    Ok(())
}

fn load_vars(conn: &Connection, environment_id: &str) -> Result<Vec<EnvVar>> {
    let mut stmt = conn.prepare(
        "SELECT name, kind, value, description, enabled
         FROM env_vars WHERE environment_id = ?1 ORDER BY sort_order, id",
    )?;
    let rows = stmt
        .query_map([environment_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(name, kind, value, description, enabled)| {
            let value = match kind.as_str() {
                "plain" => VarValue::Plain(value.unwrap_or_default()),
                "secret" => VarValue::Secret,
                other => return Err(StorageError::Corrupt(format!("env_vars.kind: {other}"))),
            };
            Ok(EnvVar {
                name,
                value,
                description,
                enabled,
            })
        })
        .collect()
}

struct EnvRow {
    id: String,
    project_id: String,
    name: String,
    is_global: bool,
    created_at: String,
    updated_at: String,
}

fn env_row(row: &Row<'_>) -> rusqlite::Result<EnvRow> {
    Ok(EnvRow {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        name: row.get("name")?,
        is_global: row.get("is_global")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

const ENV_COLUMNS: &str = "id, project_id, name, is_global, created_at, updated_at";

fn with_vars(conn: &Connection, row: EnvRow) -> Result<Environment> {
    let vars = load_vars(conn, &row.id)?;
    Ok(Environment {
        id: row.id,
        project_id: row.project_id,
        name: row.name,
        is_global: row.is_global,
        vars,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

pub(crate) fn get_env(conn: &Connection, id: &str) -> Result<Option<Environment>> {
    let row = conn
        .query_row(
            &format!("SELECT {ENV_COLUMNS} FROM environments WHERE id = ?1"),
            [id],
            env_row,
        )
        .optional()?;
    row.map(|r| with_vars(conn, r)).transpose()
}

fn project_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM projects WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn next_sort_order(conn: &Connection, project_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT coalesce(max(sort_order), 0) + 1 FROM environments WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )?)
}

fn insert_vars(conn: &Connection, environment_id: &str, vars: &[EnvVar]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO env_vars (environment_id, name, kind, value, description, sort_order, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (i, v) in vars.iter().enumerate() {
        let (kind, value): (&str, Option<&str>) = match &v.value {
            VarValue::Plain(s) => ("plain", Some(s.as_str())),
            // シークレットの値は SQLite に入れない
            VarValue::Secret => ("secret", None),
        };
        stmt.execute(params![
            environment_id,
            v.name,
            kind,
            value,
            v.description,
            i64::try_from(i).unwrap_or(i64::MAX),
            v.enabled
        ])?;
    }
    Ok(())
}

impl Database {
    /// プロジェクトの環境を返す。「グローバル」が先頭、続いて作った順。
    pub fn env_list(&self, project_id: &str) -> Result<Vec<Environment>> {
        let conn = self.conn();
        if !project_exists(&conn, project_id)? {
            return Err(project_not_found(project_id));
        }
        let mut stmt = conn.prepare(&format!(
            "SELECT {ENV_COLUMNS} FROM environments WHERE project_id = ?1
             ORDER BY is_global DESC, sort_order, rowid"
        ))?;
        let rows = stmt
            .query_map([project_id], env_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|r| with_vars(&conn, r)).collect()
    }

    /// ID で環境を 1 つ返す。
    pub fn env_get(&self, id: &str) -> Result<Option<Environment>> {
        get_env(&self.conn(), id)
    }

    /// プロジェクトの「グローバル」環境。
    pub fn env_global(&self, project_id: &str) -> Result<Environment> {
        let conn = self.conn();
        let row = conn
            .query_row(
                &format!(
                    "SELECT {ENV_COLUMNS} FROM environments WHERE project_id = ?1 AND is_global = 1"
                ),
                [project_id],
                env_row,
            )
            .optional()?;
        match row {
            Some(r) => with_vars(&conn, r),
            None if project_exists(&conn, project_id)? => Err(StorageError::Corrupt(format!(
                "グローバル環境がありません: {project_id}"
            ))),
            None => Err(project_not_found(project_id)),
        }
    }

    /// 環境を作る（変数はなし）。プロジェクトに「選んでいる環境」がまだなければ、これを選ぶ。
    pub fn env_create(&self, project_id: &str, name: &str) -> Result<Environment> {
        let name = normalize_env_name(name)?;
        let mut conn = self.conn();
        if !project_exists(&conn, project_id)? {
            return Err(project_not_found(project_id));
        }
        let id = new_id();
        let ts = now();
        let tx = conn.transaction()?;
        let order = next_sort_order(&tx, project_id)?;
        tx.execute(
            "INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at, is_global)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0)",
            params![id, project_id, name, order, ts],
        )?;
        tx.execute(
            "UPDATE projects SET default_environment_id = ?2, updated_at = ?3
             WHERE id = ?1 AND default_environment_id IS NULL",
            params![project_id, id, ts],
        )?;
        tx.commit()?;
        get_env(&conn, &id)?.ok_or_else(|| env_not_found(&id))
    }

    /// yaml の `servers` から環境を作る（`baseUrl` 変数に URL を入れる）。環境名は servers の説明（なければ URL）。
    /// 同じ名前の環境がすでにあれば作らない。作った環境を返す。
    /// プロジェクトに「選んでいる環境」がまだなければ、最初に作ったものを選ぶ。
    pub fn env_create_from_servers(
        &self,
        project_id: &str,
        servers: &[nekoreq_core::ServerInfo],
    ) -> Result<Vec<Environment>> {
        let mut conn = self.conn();
        if !project_exists(&conn, project_id)? {
            return Err(project_not_found(project_id));
        }
        let tx = conn.transaction()?;
        let mut existing: Vec<String> = {
            let mut stmt = tx.prepare("SELECT name FROM environments WHERE project_id = ?1")?;
            let rows = stmt.query_map([project_id], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut created: Vec<String> = Vec::new();
        for s in servers {
            let url = s.url.trim();
            if url.is_empty() {
                continue;
            }
            let wanted = s
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .unwrap_or(url);
            let name: String = wanted.chars().take(MAX_ENV_NAME_CHARS).collect();
            // 「グローバル」は特別な名前なので使えない（URL の名前にする）
            let name = if name == GLOBAL_ENVIRONMENT_NAME {
                url.chars().take(MAX_ENV_NAME_CHARS).collect()
            } else {
                name
            };
            if existing.contains(&name) {
                continue;
            }
            let id = new_id();
            let ts = now();
            let order = next_sort_order(&tx, project_id)?;
            tx.execute(
                "INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at, is_global)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0)",
                params![id, project_id, name, order, ts],
            )?;
            insert_vars(
                &tx,
                &id,
                &[EnvVar {
                    name: "baseUrl".into(),
                    value: VarValue::Plain(url.to_string()),
                    description: "yaml の servers から作成".into(),
                    enabled: true,
                }],
            )?;
            tx.execute(
                "UPDATE projects SET default_environment_id = ?2, updated_at = ?3
                 WHERE id = ?1 AND default_environment_id IS NULL",
                params![project_id, id, ts],
            )?;
            existing.push(name);
            created.push(id);
        }
        tx.commit()?;
        created
            .iter()
            .map(|id| get_env(&conn, id)?.ok_or_else(|| env_not_found(id)))
            .collect()
    }

    /// 環境を複製する（名前は「<元の名前> のコピー」。変数は有効・無効も含めて写す。
    /// シークレットの値はキーチェーンにあるので、呼び出し側で写す）。
    pub fn env_duplicate(&self, id: &str) -> Result<Environment> {
        let mut conn = self.conn();
        let src = get_env(&conn, id)?.ok_or_else(|| env_not_found(id))?;
        if src.is_global {
            return Err(StorageError::Invalid("グローバルは複製できません".into()));
        }
        let suffix = " のコピー";
        let max_base = MAX_ENV_NAME_CHARS - suffix.chars().count();
        let base: String = src.name.chars().take(max_base).collect();
        let name = format!("{base}{suffix}");
        let new = new_id();
        let ts = now();
        let tx = conn.transaction()?;
        let order = next_sort_order(&tx, &src.project_id)?;
        tx.execute(
            "INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at, is_global)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0)",
            params![new, src.project_id, name, order, ts],
        )?;
        insert_vars(&tx, &new, &src.vars)?;
        tx.commit()?;
        get_env(&conn, &new)?.ok_or_else(|| env_not_found(&new))
    }

    /// 環境を消す（変数も消える）。「グローバル」は消せない。
    /// 選んでいた環境を消したときは、残りの最初の環境を選び直す（なければ未選択）。
    pub fn env_delete(&self, id: &str) -> Result<()> {
        let mut conn = self.conn();
        let env = get_env(&conn, id)?.ok_or_else(|| env_not_found(id))?;
        if env.is_global {
            return Err(StorageError::Invalid("グローバルは削除できません".into()));
        }
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE projects SET updated_at = ?3, default_environment_id = (
                 SELECT e.id FROM environments e
                 WHERE e.project_id = ?1 AND e.is_global = 0 AND e.id <> ?2
                 ORDER BY e.sort_order, e.rowid LIMIT 1)
             WHERE id = ?1 AND default_environment_id = ?2",
            params![env.project_id, id, now()],
        )?;
        tx.execute("DELETE FROM environments WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// 環境の名前と変数の一覧を保存する（変数は丸ごと置き換える）。
    /// 変数名は前後の空白を除き、空・重複・`$` 始まり・空白や波括弧を含むものはエラー。
    /// シークレットの値はここでは受け取らない（キーチェーンへは `secret_set`）。
    /// 「グローバル」の名前は変えられない。
    pub fn env_save(&self, env: &Environment) -> Result<Environment> {
        let mut conn = self.conn();
        let current = get_env(&conn, &env.id)?.ok_or_else(|| env_not_found(&env.id))?;
        let name = if current.is_global {
            if env.name.trim() != current.name {
                return Err(StorageError::Invalid(
                    "グローバルの名前は変えられません".into(),
                ));
            }
            current.name.clone()
        } else {
            normalize_env_name(&env.name)?
        };

        let mut vars = Vec::with_capacity(env.vars.len());
        for v in &env.vars {
            let n = normalize_var_name(&v.name)?;
            if vars.iter().any(|x: &EnvVar| x.name == n) {
                return Err(StorageError::Invalid(format!(
                    "変数名が重複しています: {n}"
                )));
            }
            vars.push(EnvVar {
                name: n,
                value: v.value.clone(),
                description: v.description.clone(),
                enabled: v.enabled,
            });
        }

        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE environments SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![env.id, name, now()],
        )?;
        tx.execute("DELETE FROM env_vars WHERE environment_id = ?1", [&env.id])?;
        insert_vars(&tx, &env.id, &vars)?;
        tx.commit()?;
        Database::truncate_wal(&conn);
        get_env(&conn, &env.id)?.ok_or_else(|| env_not_found(&env.id))
    }

    /// 環境に、シークレットの変数 `name` があるようにする（自動更新用。環境の他の内容は触らない）。
    ///
    /// - なければ末尾に足す（有効・説明は `description`）。
    /// - 通常の変数なら、シークレットに変える（元の値は SQLite に残さない）。
    /// - すでにシークレットなら何もしない（有効・無効もそのまま）。
    ///
    /// 変更したら `true`。
    pub fn env_var_ensure_secret(
        &self,
        environment_id: &str,
        name: &str,
        description: &str,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let env = get_env(&conn, environment_id)?.ok_or_else(|| env_not_found(environment_id))?;
        let tx = conn.transaction()?;
        let changed = match env.vars.iter().find(|v| v.name == name) {
            Some(v) if v.value == VarValue::Secret => false,
            Some(_) => {
                tx.execute(
                    "UPDATE env_vars SET kind = 'secret', value = NULL
                     WHERE environment_id = ?1 AND name = ?2",
                    params![environment_id, name],
                )?;
                true
            }
            None => {
                let next: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM env_vars WHERE environment_id = ?1",
                    [environment_id],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "INSERT INTO env_vars (environment_id, name, kind, value, description, sort_order, enabled)
                     VALUES (?1, ?2, 'secret', NULL, ?3, ?4, 1)",
                    params![environment_id, name, description, next],
                )?;
                true
            }
        };
        if changed {
            tx.execute(
                "UPDATE environments SET updated_at = ?2 WHERE id = ?1",
                params![environment_id, now()],
            )?;
        }
        tx.commit()?;
        if changed {
            Database::truncate_wal(&conn);
        }
        Ok(changed)
    }

    /// プロジェクトの「選んでいる環境」を決める（`None` で未選択）。「グローバル」は選べない。
    /// 更新後のプロジェクトを返す。
    pub fn env_select(&self, project_id: &str, environment_id: Option<&str>) -> Result<Project> {
        let conn = self.conn();
        if !project_exists(&conn, project_id)? {
            return Err(project_not_found(project_id));
        }
        if let Some(eid) = environment_id {
            let env = get_env(&conn, eid)?.ok_or_else(|| env_not_found(eid))?;
            if env.project_id != project_id {
                return Err(env_not_found(eid));
            }
            if env.is_global {
                return Err(StorageError::Invalid(
                    "グローバルは選べません（どの環境を選んでいても使われます）".into(),
                ));
            }
        }
        conn.execute(
            "UPDATE projects SET default_environment_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![project_id, environment_id, now()],
        )?;
        get_project(&conn, project_id)?.ok_or_else(|| project_not_found(project_id))
    }
}
