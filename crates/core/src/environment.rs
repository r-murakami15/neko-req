use serde::{Deserialize, Serialize};
use specta::Type;

/// グローバル環境の名前。
pub const GLOBAL_ENVIRONMENT_NAME: &str = "グローバル";

/// 環境（接続先ごとの変数の集まり）。servers から作成し、以後はアプリで編集する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// プロジェクトに 1 つだけある「グローバル」環境（どの環境を選んでいても使われる共通の変数）。
    /// 選択・削除・名前変更はできない。
    pub is_global: bool,
    pub vars: Vec<EnvVar>,
    pub created_at: String,
    pub updated_at: String,
}

/// 環境変数 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    pub name: String,
    pub value: VarValue,
    pub description: String,
    /// 無効の変数は展開に使わない（画面の「有効」チェック）。
    pub enabled: bool,
}

/// 変数の値。シークレットの実体は OS のキーチェーンにあり、SQLite にも画面へ渡す型にも載せない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum VarValue {
    Plain(String),
    Secret,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_has_no_value_on_the_wire() {
        let v = serde_json::to_value(VarValue::Secret).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "secret"}));
        let v = serde_json::to_value(VarValue::Plain("x".into())).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "plain", "value": "x"}));
    }
}
