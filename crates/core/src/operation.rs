use std::fmt;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::request::ParamIn;

/// Operation と RequestDraft を結びつけるキー。operationId があればそれ、なければ `METHOD path`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum OpKey {
    Id(String),
    MethodPath { method: String, path: String },
}

/// `OpKey::parse` の失敗。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("OpKey として読めません: {0}")]
pub struct OpKeyParseError(pub String);

impl OpKey {
    /// method は大文字にそろえる。
    pub fn method_path(method: &str, path: &str) -> Self {
        OpKey::MethodPath {
            method: method.to_ascii_uppercase(),
            path: path.to_string(),
        }
    }

    /// 保存用の 1 本の文字列にする。`id:<operationId>` または `mp:<METHOD> <path>`。
    pub fn to_key_string(&self) -> String {
        match self {
            OpKey::Id(id) => format!("id:{id}"),
            OpKey::MethodPath { method, path } => format!("mp:{method} {path}"),
        }
    }

    /// `to_key_string` の逆。
    pub fn parse(s: &str) -> Result<Self, OpKeyParseError> {
        if let Some(id) = s.strip_prefix("id:") {
            return Ok(OpKey::Id(id.to_string()));
        }
        if let Some(rest) = s.strip_prefix("mp:") {
            if let Some((method, path)) = rest.split_once(' ') {
                if !method.is_empty() && !path.is_empty() {
                    return Ok(OpKey::MethodPath {
                        method: method.to_string(),
                        path: path.to_string(),
                    });
                }
            }
        }
        Err(OpKeyParseError(s.to_string()))
    }
}

impl fmt::Display for OpKey {
    /// 画面に出す形。`operationId` または `METHOD path`。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpKey::Id(id) => f.write_str(id),
            OpKey::MethodPath { method, path } => write!(f, "{method} {path}"),
        }
    }
}

/// yaml から作る API の定義（読み取り専用。同期のたびに作り直す）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub project_id: String,
    pub op_key: OpKey,
    pub operation_id: Option<String>,
    /// 大文字（GET / POST ...）。
    pub method: String,
    pub path: String,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    /// Markdown。
    pub description: Option<String>,
    pub parameters: Vec<ParamSpec>,
    pub request_body: Option<RequestBodySpec>,
    pub responses: Vec<ResponseSpec>,
    /// `None` = 全体の security に従う、`Some(vec![])` = 認証なし（`security: []`）。
    pub security: Option<Vec<SecurityRequirement>>,
    /// 実際に書かれたファイル（入口からの相対パスまたは URL）と行番号。
    pub source_file: Option<String>,
    pub source_line: Option<u32>,
    /// Operation の内容のハッシュ。同期時に「追加・変更・削除」を出すのに使う。
    pub content_hash: String,
}

impl Operation {
    /// この Operation に実際に適用される security。`security` が未指定なら全体（`root`）に従う。
    /// 空なら認証なし（`security: []` か、全体にも security がない）。
    pub fn effective_security<'a>(
        &'a self,
        root: &'a [SecurityRequirement],
    ) -> &'a [SecurityRequirement] {
        match &self.security {
            Some(own) => own,
            None => root,
        }
    }
}

/// 前回の Operation の一覧と今回の一覧の突き合わせ結果（添字）。同期の差分と、保存の反映の両方で使う。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationMatching {
    /// `(前回の添字, 今回の添字)`。operationId が変わっても `METHOD path` が同じなら組にする。
    pub pairs: Vec<(usize, usize)>,
    /// 今回にだけある。
    pub added: Vec<usize>,
    /// 前回にだけある。
    pub removed: Vec<usize>,
}

/// 前回と今回を突き合わせる。まず `op_key` が同じもの、残りのうち `METHOD path` が同じものを同じ API とみなす
/// （operationId の付け替えで入力値が失われないように）。
pub fn match_operations(old: &[Operation], new: &[Operation]) -> OperationMatching {
    use std::collections::HashMap;
    let mut out = OperationMatching::default();
    let by_key: HashMap<&OpKey, usize> = old
        .iter()
        .enumerate()
        .map(|(i, o)| (&o.op_key, i))
        .collect();
    let mut old_used = vec![false; old.len()];
    let mut unmatched_new: Vec<usize> = Vec::new();
    for (j, n) in new.iter().enumerate() {
        match by_key.get(&n.op_key) {
            Some(&i) if !old_used[i] => {
                old_used[i] = true;
                out.pairs.push((i, j));
            }
            _ => unmatched_new.push(j),
        }
    }
    let mut by_mp: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, o) in old.iter().enumerate().filter(|(i, _)| !old_used[*i]) {
        by_mp
            .entry((o.method.as_str(), o.path.as_str()))
            .or_insert(i);
    }
    for j in unmatched_new {
        let n = &new[j];
        match by_mp.remove(&(n.method.as_str(), n.path.as_str())) {
            Some(i) => {
                old_used[i] = true;
                out.pairs.push((i, j));
            }
            None => out.added.push(j),
        }
    }
    out.removed = (0..old.len()).filter(|i| !old_used[*i]).collect();
    out.pairs.sort_by_key(|&(_, j)| j);
    out
}

/// yaml の parameter 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    pub name: String,
    #[serde(rename = "in")]
    pub location: ParamIn,
    pub required: bool,
    pub description: Option<String>,
    pub example: Option<serde_json::Value>,
    pub default: Option<serde_json::Value>,
    /// JSON Schema（`$ref` を展開済み。参照の名前は `x-nekoreq-ref`、循環で止めた所は `x-nekoreq-circular: true`、
    /// 解決できなかった所は `x-nekoreq-unresolved`）。
    pub schema: Option<serde_json::Value>,
}

/// yaml の requestBody。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RequestBodySpec {
    pub required: bool,
    pub description: Option<String>,
    pub content: Vec<MediaTypeSpec>,
}

/// Content-Type ごとのスキーマと例。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MediaTypeSpec {
    pub content_type: String,
    /// `ParamSpec.schema` と同じ形。
    pub schema: Option<serde_json::Value>,
    /// yaml のメディア型に書かれた `example`（なければ `examples` の最初の値）。スキーマから作った例は含まない。
    pub example: Option<serde_json::Value>,
}

/// yaml の responses の 1 ステータス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSpec {
    /// `"200"`、`"4XX"`、`"default"` など。
    pub status: String,
    pub description: Option<String>,
    pub content: Vec<MediaTypeSpec>,
}

/// security の 1 要件（AND でつながる scheme の組。要件どうしは OR）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SecurityRequirement {
    pub schemes: Vec<SecurityRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SecurityRef {
    /// `securitySchemes` のキー（例: `cookieAuth`）。
    pub name: String,
    pub scopes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(id: Option<&str>, method: &str, path: &str) -> Operation {
        Operation {
            id: String::new(),
            project_id: String::new(),
            op_key: match id {
                Some(i) => OpKey::Id(i.into()),
                None => OpKey::method_path(method, path),
            },
            operation_id: id.map(String::from),
            method: method.into(),
            path: path.into(),
            tags: vec![],
            summary: None,
            description: None,
            parameters: vec![],
            request_body: None,
            responses: vec![],
            security: None,
            source_file: None,
            source_line: None,
            content_hash: String::new(),
        }
    }

    #[test]
    fn matching_pairs_same_key_then_same_method_path() {
        let old = vec![
            op(Some("a"), "GET", "/a"),
            op(Some("b"), "GET", "/b"),
            op(Some("gone"), "GET", "/gone"),
        ];
        let new = vec![
            op(Some("a"), "GET", "/a"),
            op(Some("b2"), "GET", "/b"),
            op(Some("fresh"), "POST", "/fresh"),
        ];
        let m = match_operations(&old, &new);
        assert_eq!(m.pairs, vec![(0, 0), (1, 1)]);
        assert_eq!(m.added, vec![2]);
        assert_eq!(m.removed, vec![2]);
    }

    #[test]
    fn effective_security_follows_root_when_unspecified() {
        let root = vec![SecurityRequirement {
            schemes: vec![SecurityRef {
                name: "cookieAuth".into(),
                scopes: vec![],
            }],
        }];
        let mut o = op(Some("a"), "GET", "/a");
        assert_eq!(o.effective_security(&root).len(), 1);
        o.security = Some(vec![]);
        assert!(o.effective_security(&root).is_empty());
    }

    #[test]
    fn op_key_round_trips_through_key_string() {
        let keys = [
            OpKey::Id("getClientList".into()),
            OpKey::method_path("get", "/api/v1/video/clients/{clientId}"),
        ];
        for key in keys {
            assert_eq!(OpKey::parse(&key.to_key_string()).unwrap(), key);
        }
        assert!(OpKey::parse("xx").is_err());
        assert!(OpKey::parse("mp:GET").is_err());
    }

    #[test]
    fn op_key_serializes_as_adjacently_tagged() {
        let json = serde_json::to_value(OpKey::Id("a".into())).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "id", "value": "a"}));
        let json = serde_json::to_value(OpKey::method_path("get", "/x")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "methodPath", "value": {"method": "GET", "path": "/x"}})
        );
    }
}
