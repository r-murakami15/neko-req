//! nekoreq-secrets: シークレット（session・xsrfToken・password など）の保存先。
//!
//! 本番は OS のキーチェーン（macOS キーチェーン、Windows 資格情報マネージャー）を使い、
//! SQLite・履歴・ログにシークレットの値を残さない。
//!
//! - サービス名 `NekoReq/<プロジェクト ID>`、アカウント名 `<環境 ID>/<変数名>`（`SecretKey`）。
//! - `SecretStore` トレイトの実装は 2 つ。本番用の `KeyringSecretStore`（keyring クレート）と、
//!   テスト・コンテナ用の `MemorySecretStore`。
//! - Linux では keyring の `linux-native`（kernel keyring）を入れてあり、コンテナでもビルドは通る。
//!   Linux はリリース対象外で、実行時に使えなくてよい。

use std::collections::HashMap;
use std::sync::Mutex;

use thiserror::Error;

/// シークレット 1 件の置き場所。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretKey {
    pub project_id: String,
    pub environment_id: String,
    pub name: String,
}

impl SecretKey {
    pub fn new(
        project_id: impl Into<String>,
        environment_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            environment_id: environment_id.into(),
            name: name.into(),
        }
    }

    /// キーチェーンのサービス名。
    pub fn service(&self) -> String {
        format!("NekoReq/{}", self.project_id)
    }

    /// キーチェーンのアカウント名。
    pub fn account(&self) -> String {
        format!("{}/{}", self.environment_id, self.name)
    }
}

/// シークレットの読み書きの失敗。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecretError {
    /// キーチェーンのエラー（ロックされている・アクセスを拒否された・使えない環境など）。
    #[error("キーチェーンを使えませんでした: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, SecretError>;

/// シークレットの読み書き。値はログに出さないこと。
pub trait SecretStore: Send + Sync {
    /// 値を読む。なければ `None`。
    fn get(&self, key: &SecretKey) -> Result<Option<String>>;
    /// 値を書く（あれば置き換える）。
    fn set(&self, key: &SecretKey, value: &str) -> Result<()>;
    /// 値を消す。なくてもエラーにしない。
    fn delete(&self, key: &SecretKey) -> Result<()>;
}

/// OS のキーチェーン。
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(key: &SecretKey) -> Result<keyring::Entry> {
        keyring::Entry::new(&key.service(), &key.account()).map_err(backend)
    }
}

fn backend(e: keyring::Error) -> SecretError {
    SecretError::Backend(e.to_string())
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, key: &SecretKey) -> Result<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    fn set(&self, key: &SecretKey, value: &str) -> Result<()> {
        Self::entry(key)?.set_password(value).map_err(backend)
    }

    fn delete(&self, key: &SecretKey) -> Result<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(backend(e)),
        }
    }
}

/// メモリ上の実装（テスト・コンテナ用）。プロセスが終わると消える。
/// `Debug` は値を出さない（件数だけ）。
#[derive(Default)]
pub struct MemorySecretStore {
    map: Mutex<HashMap<(String, String), String>>,
}

impl std::fmt::Debug for MemorySecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemorySecretStore")
            .field("entries", &self.len())
            .finish_non_exhaustive()
    }
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入っている件数（テスト用）。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), String>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &SecretKey) -> Result<Option<String>> {
        Ok(self.lock().get(&(key.service(), key.account())).cloned())
    }

    fn set(&self, key: &SecretKey, value: &str) -> Result<()> {
        self.lock()
            .insert((key.service(), key.account()), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<()> {
        self.lock().remove(&(key.service(), key.account()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_and_account_names_follow_the_design() {
        let k = SecretKey::new("proj-1", "env-1", "session");
        assert_eq!(k.service(), "NekoReq/proj-1");
        assert_eq!(k.account(), "env-1/session");
    }

    #[test]
    fn memory_store_get_set_delete() {
        let s = MemorySecretStore::new();
        let k = SecretKey::new("p", "e", "password");
        assert_eq!(s.get(&k).unwrap(), None);
        s.set(&k, "one").unwrap();
        assert_eq!(s.get(&k).unwrap().as_deref(), Some("one"));
        s.set(&k, "two").unwrap();
        assert_eq!(s.get(&k).unwrap().as_deref(), Some("two"), "上書きできる");
        assert_eq!(s.len(), 1);
        s.delete(&k).unwrap();
        assert_eq!(s.get(&k).unwrap(), None);
        s.delete(&k).unwrap(); // なくてもエラーにしない
        assert!(s.is_empty());
    }

    #[test]
    fn memory_store_keeps_projects_environments_and_names_apart() {
        let s = MemorySecretStore::new();
        s.set(&SecretKey::new("p1", "e1", "a"), "1").unwrap();
        s.set(&SecretKey::new("p2", "e1", "a"), "2").unwrap();
        s.set(&SecretKey::new("p1", "e2", "a"), "3").unwrap();
        s.set(&SecretKey::new("p1", "e1", "b"), "4").unwrap();
        assert_eq!(
            s.get(&SecretKey::new("p1", "e1", "a")).unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(
            s.get(&SecretKey::new("p2", "e1", "a")).unwrap().as_deref(),
            Some("2")
        );
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn store_is_usable_as_a_trait_object_across_threads() {
        let s: std::sync::Arc<dyn SecretStore> = std::sync::Arc::new(MemorySecretStore::new());
        let s2 = s.clone();
        std::thread::spawn(move || {
            s2.set(&SecretKey::new("p", "e", "x"), "v").unwrap();
        })
        .join()
        .unwrap();
        assert_eq!(
            s.get(&SecretKey::new("p", "e", "x")).unwrap().as_deref(),
            Some("v")
        );
    }
}
