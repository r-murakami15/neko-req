use serde::{Deserialize, Serialize};
use specta::Type;

use crate::operation::OpKey;

/// パラメータの置き場所。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ParamIn {
    Query,
    Path,
    Header,
    Cookie,
}

/// 名前と値の組（有効・無効つき）。パラメータとヘッダーは `location` で区別する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KeyValue {
    pub location: ParamIn,
    pub name: String,
    /// `{{変数名}}` を含められる。
    pub value: String,
    pub enabled: bool,
}

/// リクエストボディ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Body {
    None,
    Json(String),
    Form(Vec<KeyValue>),
    Multipart(Vec<MultipartPart>),
    Text(String),
    /// 送るファイルのパス。
    Binary(String),
}

/// multipart の 1 パート。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MultipartPart {
    pub name: String,
    pub value: MultipartValue,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum MultipartValue {
    Text(String),
    /// ファイルのパス。
    File(String),
}

/// API キーの置き場所。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyIn {
    Header,
    Query,
    Cookie,
}

/// 認証の上書き。`RequestDraft.auth = None` のときは Operation の security に従う。
///
/// 値には `{{変数名}}` を書く。シークレットは環境変数（キーチェーン）経由で参照し、
/// ここに平文のシークレットを保存しない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum AuthOverride {
    /// 認証なしで送る。
    None,
    ApiKey {
        location: ApiKeyIn,
        name: String,
        value: String,
    },
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
}

/// Operation に対して自分が入れた値。`(project_id, op_key)` ごとに 1 つ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestDraft {
    pub id: String,
    pub project_id: String,
    pub op_key: OpKey,
    pub params: Vec<KeyValue>,
    pub body: Body,
    pub auth: Option<AuthOverride>,
    pub updated_at: String,
}

/// yaml にないリクエスト（cURL 取り込みなど）。「その他」フォルダに入る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AdHocRequest {
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// 説明（任意。保存ダイアログの「説明（任意）」）。なければ空。
    #[serde(default)]
    pub description: String,
    pub method: String,
    /// `{{baseUrl}}/...` のような URL。
    pub url: String,
    pub params: Vec<KeyValue>,
    pub body: Body,
    pub auth: Option<AuthOverride>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_json_shape() {
        let v = serde_json::to_value(Body::Json("{}".into())).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "json", "value": "{}"}));
        let v = serde_json::to_value(Body::None).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "none"}));
        let back: Body = serde_json::from_value(serde_json::json!({"kind": "none"})).unwrap();
        assert_eq!(back, Body::None);
    }

    #[test]
    fn auth_override_shape() {
        let v = serde_json::to_value(AuthOverride::Bearer {
            token: "{{token}}".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type": "bearer", "token": "{{token}}"})
        );
    }
}
