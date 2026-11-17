//! アプリの状態。Tauri から切り離してあるので、テストでは Tauri を起動せずに使える。
//!
//! - 保存（SQLite）は `Database`、シークレットは `SecretStore`（本番はキーチェーン、テストはメモリ）。
//! - 送信まわり（変数の組み立て・送信・中止・ボディの保持）は `send.rs`。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use bytes::Bytes;
use nekoreq_core::{
    AdHocRequest, Environment, OpKey, Project, ProjectSettings, RequestDraft, VarValue,
};
use nekoreq_http::CancellationToken;
use nekoreq_secrets::{KeyringSecretStore, SecretKey, SecretStore};
use nekoreq_storage::{Database, LATEST_VERSION};

use crate::commands::AppInfo;
use crate::error::AppError;

/// DB のファイル名。置き場所は `<app_data_dir>/data.sqlite`。
pub const DB_FILE_NAME: &str = "data.sqlite";

/// 「ファイルに保存」のために全体を持っておく直近のレスポンスの数。
pub const KEPT_BODIES: usize = 8;

/// アプリの状態。複製しても同じ中身を指す（送信の非同期タスクに渡すため）。
#[derive(Clone)]
pub struct AppState {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) db: Database,
    pub(crate) data_dir: PathBuf,
    pub(crate) secrets: Arc<dyn SecretStore>,
    /// 送信中のリクエスト（送信 ID → 中止の合図）。
    pub(crate) in_flight: Mutex<HashMap<String, CancellationToken>>,
    /// 直近のレスポンスのボディ全体（送信 ID → ボディ）。古いものから捨てる。
    pub(crate) bodies: Mutex<VecDeque<(String, Bytes)>>,
    /// 自動更新（環境の変数の作成とシークレットの書き込み）を直列にする。
    pub(crate) auto_lock: Mutex<()>,
}

impl Inner {
    pub(crate) fn in_flight(&self) -> MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.in_flight.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn auto_lock(&self) -> MutexGuard<'_, ()> {
        self.auto_lock.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn bodies(&self) -> MutexGuard<'_, VecDeque<(String, Bytes)>> {
        self.bodies.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl AppState {
    /// データフォルダの DB を開く（なければ作る）。シークレットは OS のキーチェーンに置く。
    pub fn open(data_dir: &Path) -> Result<Self, AppError> {
        Self::open_with_secrets(data_dir, Arc::new(KeyringSecretStore::new()))
    }

    /// シークレットの保存先を指定して開く（テストではメモリ実装を渡す）。
    pub fn open_with_secrets(
        data_dir: &Path,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Self, AppError> {
        let db = Database::open(data_dir.join(DB_FILE_NAME))?;
        let state = Self {
            inner: Arc::new(Inner {
                db,
                data_dir: data_dir.to_path_buf(),
                secrets,
                in_flight: Mutex::new(HashMap::new()),
                bodies: Mutex::new(VecDeque::new()),
                auto_lock: Mutex::new(()),
            }),
        };
        // 前回の異常終了で残ったアップロードの一時フォルダを消す
        state.clean_temp_dirs();
        Ok(state)
    }

    pub(crate) fn db(&self) -> &Database {
        &self.inner.db
    }

    pub(crate) fn secrets(&self) -> &dyn SecretStore {
        self.inner.secrets.as_ref()
    }

    pub fn app_info(&self, version: &str, identifier: &str) -> Result<AppInfo, AppError> {
        Ok(AppInfo {
            name: "NekoReq".to_string(),
            version: version.to_string(),
            identifier: identifier.to_string(),
            data_dir: self.inner.data_dir.display().to_string(),
            db_path: self.inner.data_dir.join(DB_FILE_NAME).display().to_string(),
            schema_version: self.db().schema_version()?,
        })
    }

    // ---- プロジェクト ----

    pub fn project_create(&self, name: &str) -> Result<Project, AppError> {
        Ok(self.db().project_create(name)?)
    }

    pub fn project_list(&self) -> Result<Vec<Project>, AppError> {
        Ok(self.db().project_list()?)
    }

    pub fn project_rename(&self, id: &str, name: &str) -> Result<Project, AppError> {
        Ok(self.db().project_rename(id, name)?)
    }

    /// プロジェクトの設定（cookieAuth・CSRF の自動化のオン・オフ）を保存する。更新後のプロジェクトを返す。
    pub fn project_settings_set(
        &self,
        id: &str,
        settings: &ProjectSettings,
    ) -> Result<Project, AppError> {
        Ok(self.db().project_settings_set(id, settings)?)
    }

    /// プロジェクトを消す。その環境のシークレットもキーチェーンから消す（消せなくてもプロジェクトは消える）。
    pub fn project_delete(&self, id: &str) -> Result<(), AppError> {
        let envs = self.db().env_list(id).unwrap_or_default();
        self.db().project_delete(id)?;
        for env in &envs {
            self.delete_env_secrets(id, env);
        }
        // 取り込み元のアクセストークンと、アップロードで置いた yaml 一式
        let _ = self
            .secrets()
            .delete(&SecretKey::new(id, "source", "token"));
        let _ = std::fs::remove_dir_all(self.project_sources_dir(id));
        Ok(())
    }

    // ---- 環境と変数 ----

    /// 環境の一覧。「グローバル」が先頭。選んでいる環境は `Project.defaultEnvironmentId`。
    pub fn env_list(&self, project_id: &str) -> Result<Vec<Environment>, AppError> {
        Ok(self.db().env_list(project_id)?)
    }

    /// 環境の名前と変数（通常の値・シークレットの印・有効）を保存する。
    /// シークレットでなくなった・消えた変数の値は、キーチェーンからも消す。
    pub fn env_save(&self, env: &Environment) -> Result<Environment, AppError> {
        let before = self
            .db()
            .env_get(&env.id)?
            .ok_or_else(|| AppError::NotFound(format!("環境が見つかりません: {}", env.id)))?;
        let saved = self.db().env_save(env)?;
        let still_secret = |name: &str| {
            saved
                .vars
                .iter()
                .any(|v| v.name == name && v.value == VarValue::Secret)
        };
        for v in before
            .vars
            .iter()
            .filter(|v| v.value == VarValue::Secret && !still_secret(&v.name))
        {
            let _ = self
                .secrets()
                .delete(&SecretKey::new(&saved.project_id, &saved.id, &v.name));
        }
        Ok(saved)
    }

    pub fn env_create(&self, project_id: &str, name: &str) -> Result<Environment, AppError> {
        Ok(self.db().env_create(project_id, name)?)
    }

    /// 環境を複製する。シークレットの値もキーチェーンから写す（写せなければ複製を取り消す）。
    pub fn env_duplicate(&self, id: &str) -> Result<Environment, AppError> {
        let src = self
            .db()
            .env_get(id)?
            .ok_or_else(|| AppError::NotFound(format!("環境が見つかりません: {id}")))?;
        let copy = self.db().env_duplicate(id)?;
        let copied = (|| -> Result<(), AppError> {
            for v in src.vars.iter().filter(|v| v.value == VarValue::Secret) {
                let from = SecretKey::new(&src.project_id, &src.id, &v.name);
                if let Some(value) = self.secrets().get(&from)? {
                    self.secrets()
                        .set(&SecretKey::new(&copy.project_id, &copy.id, &v.name), &value)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = copied {
            self.delete_env_secrets(&copy.project_id, &copy);
            let _ = self.db().env_delete(&copy.id);
            return Err(e);
        }
        Ok(copy)
    }

    /// 環境を消す（シークレットの値もキーチェーンから消す）。「グローバル」は消せない。
    pub fn env_delete(&self, id: &str) -> Result<(), AppError> {
        let env = self
            .db()
            .env_get(id)?
            .ok_or_else(|| AppError::NotFound(format!("環境が見つかりません: {id}")))?;
        self.db().env_delete(id)?;
        self.delete_env_secrets(&env.project_id, &env);
        Ok(())
    }

    /// 「選んでいる環境」を決める（`None` で未選択）。更新後のプロジェクトを返す。
    pub fn env_select(
        &self,
        project_id: &str,
        environment_id: Option<&str>,
    ) -> Result<Project, AppError> {
        Ok(self.db().env_select(project_id, environment_id)?)
    }

    fn delete_env_secrets(&self, project_id: &str, env: &Environment) {
        for v in env.vars.iter().filter(|v| v.value == VarValue::Secret) {
            let _ = self
                .secrets()
                .delete(&SecretKey::new(project_id, &env.id, &v.name));
        }
    }

    // ---- シークレット ----

    /// キーと、その環境（宣言の確認用）。
    fn secret_target(
        &self,
        project_id: &str,
        environment_id: &str,
        name: &str,
    ) -> Result<(SecretKey, Environment), AppError> {
        let env = self
            .db()
            .env_get(environment_id)?
            .filter(|e| e.project_id == project_id)
            .ok_or_else(|| AppError::NotFound(format!("環境が見つかりません: {environment_id}")))?;
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Validation("変数名を指定してください".into()));
        }
        Ok((SecretKey::new(project_id, &env.id, name), env))
    }

    fn secret_key(
        &self,
        project_id: &str,
        environment_id: &str,
        name: &str,
    ) -> Result<SecretKey, AppError> {
        Ok(self.secret_target(project_id, environment_id, name)?.0)
    }

    /// シークレットの値をキーチェーンに保存する（SQLite には入れない）。
    /// その環境で「シークレットの変数」として宣言されている名前だけ書ける（宣言のない名前の値は、
    /// 環境を消してもキーチェーンに残ってしまうため）。先に `env_save` で変数を保存しておくこと。
    pub fn secret_set(
        &self,
        project_id: &str,
        environment_id: &str,
        name: &str,
        value: &str,
    ) -> Result<(), AppError> {
        let (key, env) = self.secret_target(project_id, environment_id, name)?;
        if value.is_empty() {
            return Err(AppError::Validation(
                "値を入力してください（消すときは削除します）".into(),
            ));
        }
        let declared = env
            .vars
            .iter()
            .any(|v| v.name == key.name && v.value == VarValue::Secret);
        if !declared {
            return Err(AppError::Validation(format!(
                "「{}」はこの環境のシークレットの変数ではありません。先に変数をシークレットとして保存してください",
                key.name
            )));
        }
        Ok(self.secrets().set(&key, value)?)
    }

    pub fn secret_delete(
        &self,
        project_id: &str,
        environment_id: &str,
        name: &str,
    ) -> Result<(), AppError> {
        let key = self.secret_key(project_id, environment_id, name)?;
        Ok(self.secrets().delete(&key)?)
    }

    /// 目のアイコンで一時的に見せる用。値がなければ `None`。
    pub fn secret_reveal(
        &self,
        project_id: &str,
        environment_id: &str,
        name: &str,
    ) -> Result<Option<String>, AppError> {
        let key = self.secret_key(project_id, environment_id, name)?;
        Ok(self.secrets().get(&key)?)
    }

    // ---- 手で作ったリクエストと入力値 ----

    pub fn adhoc_list(&self, project_id: &str) -> Result<Vec<AdHocRequest>, AppError> {
        Ok(self.db().adhoc_list(project_id)?)
    }

    /// 保存する（`id` が空なら新しく作る）。
    pub fn adhoc_save(&self, request: &AdHocRequest) -> Result<AdHocRequest, AppError> {
        Ok(self.db().adhoc_save(request)?)
    }

    pub fn adhoc_delete(&self, id: &str) -> Result<(), AppError> {
        Ok(self.db().adhoc_delete(id)?)
    }

    pub fn draft_save(&self, draft: &RequestDraft) -> Result<RequestDraft, AppError> {
        Ok(self.db().draft_save(draft)?)
    }

    pub fn draft_get(
        &self,
        project_id: &str,
        op_key: &OpKey,
    ) -> Result<Option<RequestDraft>, AppError> {
        Ok(self.db().draft_get(project_id, op_key)?)
    }
}

/// このアプリが対応するスキーマの版（画面の「情報」表示用）。
pub const fn latest_schema_version() -> u32 {
    LATEST_VERSION
}

#[cfg(test)]
mod tests;
