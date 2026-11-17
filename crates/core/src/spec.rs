//! yaml の入口ファイルから読んだ「文書全体の情報」（タイトル・servers・認証の定義など）と、取り込みの変換オプション。
//! `ApiSource` に持たせ、Operation の既定の認証（`security` が未指定なら全体の `security`）を決めるのに使う。

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::operation::SecurityRequirement;
use crate::request::ApiKeyIn;

/// 入口ファイルの内容のうち、Operation に入らないもの。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecMeta {
    pub title: String,
    /// `info.version`。
    pub version: String,
    pub description: Option<String>,
    pub servers: Vec<ServerInfo>,
    pub security_schemes: Vec<SecuritySchemeInfo>,
    /// ルートの `security`（Operation に `security` がないときの既定）。
    pub root_security: Vec<SecurityRequirement>,
    pub tags: Vec<TagInfo>,
}

/// `servers` の 1 件。URL の `{変数}` は既定値で置き換えてある。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub url: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SecuritySchemeKind {
    ApiKey,
    Http,
    Oauth2,
    OpenIdConnect,
    MutualTls,
    Other,
}

/// `components.securitySchemes` の 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySchemeInfo {
    /// `securitySchemes` のキー（例: `cookieAuth`）。
    pub name: String,
    pub kind: SecuritySchemeKind,
    /// `apiKey` の置き場所（`in`）。
    pub location: Option<ApiKeyIn>,
    /// `apiKey` の名前（`name`。例: `SESSION`）。
    pub key_name: Option<String>,
    /// `http` の方式（`bearer`・`basic` など。小文字）。
    pub scheme: Option<String>,
    pub bearer_format: Option<String>,
    pub description: Option<String>,
}

impl SecuritySchemeInfo {
    /// 画面に出す短い説明（例: `cookieAuth（Cookie: SESSION）`）。
    pub fn label(&self) -> String {
        let detail = match (self.kind, self.location, &self.key_name, &self.scheme) {
            (SecuritySchemeKind::ApiKey, Some(loc), Some(key), _) => {
                let place = match loc {
                    ApiKeyIn::Header => "ヘッダー",
                    ApiKeyIn::Query => "クエリ",
                    ApiKeyIn::Cookie => "Cookie",
                };
                format!("{place}: {key}")
            }
            (SecuritySchemeKind::Http, _, _, Some(s)) => format!("HTTP {s}"),
            (SecuritySchemeKind::Oauth2, ..) => "OAuth 2.0".to_string(),
            (SecuritySchemeKind::OpenIdConnect, ..) => "OpenID Connect".to_string(),
            (SecuritySchemeKind::MutualTls, ..) => "mutual TLS".to_string(),
            _ => String::new(),
        };
        if detail.is_empty() {
            self.name.clone()
        } else {
            format!("{}（{detail}）", self.name)
        }
    }
}

/// 取り込みの変換オプション（画面「OpenAPI 取り込み」の 4 つのチェック）。既定はすべて入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportOptions {
    /// タグごとにフォルダを作る（`OperationListItem.folder` にタグを入れる）。
    pub tag_folders: bool,
    /// example からボディとパラメータの初期値を埋める。
    pub fill_from_examples: bool,
    /// cookieAuth（Cookie の apiKey）を認証設定に反映する。
    pub apply_cookie_auth: bool,
    /// description を設計タブのドキュメントにする。
    pub description_as_doc: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            tag_folders: true,
            fill_from_examples: true,
            apply_cookie_auth: true,
            description_as_doc: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_describes_cookie_auth() {
        let s = SecuritySchemeInfo {
            name: "cookieAuth".into(),
            kind: SecuritySchemeKind::ApiKey,
            location: Some(ApiKeyIn::Cookie),
            key_name: Some("SESSION".into()),
            scheme: None,
            bearer_format: None,
            description: None,
        };
        assert_eq!(s.label(), "cookieAuth（Cookie: SESSION）");
    }

    #[test]
    fn options_default_to_all_on_and_missing_fields_fall_back() {
        let o: ImportOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o, ImportOptions::default());
        assert!(
            o.tag_folders && o.fill_from_examples && o.apply_cookie_auth && o.description_as_doc
        );
        let o: ImportOptions = serde_json::from_str(r#"{"tagFolders":false}"#).unwrap();
        assert!(!o.tag_folders && o.fill_from_examples);
    }
}
