//! 画面へ返すエラー。TypeScript にも型（`AppError`）として書き出される。

use nekoreq_secrets::SecretError;
use nekoreq_storage::StorageError;
use serde::Serialize;
use specta::Type;
use thiserror::Error;

/// コマンドのエラー。`{ kind, message }` の形で画面に届く。`message` は画面にそのまま出せる日本語。
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind", content = "message")]
pub enum AppError {
    /// 入力が正しくない（名前が空など）。
    #[error("{0}")]
    Validation(String),
    /// 指定の ID が見つからない。
    #[error("{0}")]
    NotFound(String),
    /// 保存先（SQLite）のエラー。
    #[error("{0}")]
    Storage(String),
    /// キーチェーン（シークレットの保存先）のエラー。
    #[error("{0}")]
    Secret(String),
    /// ファイルの書き込みなどのエラー。
    #[error("{0}")]
    Io(String),
    /// 更新の確認・入れ替えのエラー（ネットワーク・署名の検証など）。
    #[error("{0}")]
    Update(String),
}

impl From<SecretError> for AppError {
    fn from(e: SecretError) -> Self {
        AppError::Secret(e.to_string())
    }
}

impl From<StorageError> for AppError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::InvalidName(_) | StorageError::Invalid(_) => {
                AppError::Validation(e.to_string())
            }
            StorageError::NotFound { .. } => AppError::NotFound(e.to_string()),
            other => AppError::Storage(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_errors_are_mapped_by_kind() {
        let e: AppError = StorageError::InvalidName("空です".into()).into();
        assert_eq!(e, AppError::Validation("空です".into()));
        let e: AppError = StorageError::NotFound {
            what: "プロジェクト",
            id: "x".into(),
        }
        .into();
        assert!(matches!(e, AppError::NotFound(_)));
        let e: AppError = StorageError::Corrupt("x".into()).into();
        assert!(matches!(e, AppError::Storage(_)));
        let e: AppError = StorageError::Invalid("グローバルは消せません".into()).into();
        assert_eq!(e, AppError::Validation("グローバルは消せません".into()));
        let e: AppError = SecretError::Backend("x".into()).into();
        assert!(matches!(e, AppError::Secret(_)));
    }

    #[test]
    fn serializes_as_kind_and_message() {
        let v = serde_json::to_value(AppError::Validation("だめ".into())).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "validation", "message": "だめ"})
        );
    }
}
