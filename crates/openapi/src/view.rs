//! 画面向けの組み立て。API ツリー・設計タブ・送信タブを開くときの初期状態・既定の認証。
//! 保存先（SQLite）は知らない。保存されている Operation と、入力値（RequestDraft）などを受け取って作る。

use nekoreq_core::{
    ApiKeyIn, AuthOverride, Body, ImportOptions, KeyValue, MediaTypeSpec, MultipartPart,
    MultipartValue, Operation, ParamIn, RequestBodySpec, RequestDraft, RequestSpec,
    SecuritySchemeInfo, SecuritySchemeKind, SpecMeta,
};
use serde_json::Value;

use crate::model::{
    DefaultAuth, ErrorCodeEntry, ExampleSource, MediaDoc, OperationDetail, OperationListItem,
    OperationOpen, ParamDoc, ParamRef, RequestBodyDoc, ResponseDoc, SecurityBadge, SecurityDoc,
};
use crate::schema::{self, mock, schema_table};
use crate::text;

/// 値を 1 つの文字列にする（文字列はそのまま、ほかは JSON）。
pub fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ---- 認証 ----

pub fn security_badge(op: &Operation) -> SecurityBadge {
    match &op.security {
        None => SecurityBadge::Default,
        Some(s) if s.is_empty() => SecurityBadge::None,
        Some(_) => SecurityBadge::Custom,
    }
}

/// 認証が要るか（適用される security がすべて、何かの scheme を要求している）。
pub fn requires_auth(op: &Operation, meta: &SpecMeta) -> bool {
    let eff = op.effective_security(&meta.root_security);
    !eff.is_empty() && eff.iter().all(|r| !r.schemes.is_empty())
}

pub fn security_doc(op: &Operation, meta: &SpecMeta) -> SecurityDoc {
    let eff = op.effective_security(&meta.root_security);
    let labels = eff
        .iter()
        .flat_map(|r| r.schemes.iter())
        .map(|s| {
            meta.security_schemes
                .iter()
                .find(|i| i.name == s.name)
                .map(SecuritySchemeInfo::label)
                .unwrap_or_else(|| s.name.clone())
        })
        .collect();
    SecurityDoc {
        badge: security_badge(op),
        required: requires_auth(op, meta),
        labels,
        requirements: eff.to_vec(),
    }
}

fn auth_for_scheme(info: &SecuritySchemeInfo, options: &ImportOptions) -> Option<DefaultAuth> {
    match info.kind {
        SecuritySchemeKind::ApiKey => {
            let location = info.location?;
            let name = info.key_name.clone()?;
            if location == ApiKeyIn::Cookie && !options.apply_cookie_auth {
                return None;
            }
            // cookieAuth（Cookie の SESSION）は環境変数 `session`、それ以外は scheme 名の変数
            let variable = if location == ApiKeyIn::Cookie {
                name.to_lowercase()
            } else {
                info.name.clone()
            };
            Some(DefaultAuth::ApiKey {
                scheme: info.name.clone(),
                location,
                name,
                variable,
            })
        }
        SecuritySchemeKind::Http => match info.scheme.as_deref() {
            Some("bearer") => Some(DefaultAuth::Bearer {
                scheme: info.name.clone(),
                variable: "token".into(),
            }),
            Some("basic") => Some(DefaultAuth::Basic {
                scheme: info.name.clone(),
                username_variable: "username".into(),
                password_variable: "password".into(),
            }),
            _ => None,
        },
        _ => None,
    }
}

/// yaml の security に従った既定の認証。`security: []` や、対応していない方式（OAuth 2.0 など）だけなら `None`。
/// 要件が複数あるときは、最初に対応できるもの。
pub fn default_auth(
    op: &Operation,
    meta: &SpecMeta,
    options: &ImportOptions,
) -> Option<DefaultAuth> {
    for req in op.effective_security(&meta.root_security) {
        for r in &req.schemes {
            if let Some(info) = meta.security_schemes.iter().find(|i| i.name == r.name) {
                if let Some(a) = auth_for_scheme(info, options) {
                    return Some(a);
                }
            }
        }
    }
    None
}

impl DefaultAuth {
    /// 送信に使う認証の設定（値は `{{変数}}`）。
    pub fn to_override(&self) -> AuthOverride {
        match self {
            DefaultAuth::ApiKey {
                location,
                name,
                variable,
                ..
            } => AuthOverride::ApiKey {
                location: *location,
                name: name.clone(),
                value: format!("{{{{{variable}}}}}"),
            },
            DefaultAuth::Bearer { variable, .. } => AuthOverride::Bearer {
                token: format!("{{{{{variable}}}}}"),
            },
            DefaultAuth::Basic {
                username_variable,
                password_variable,
                ..
            } => AuthOverride::Basic {
                username: format!("{{{{{username_variable}}}}}"),
                password: format!("{{{{{password_variable}}}}}"),
            },
        }
    }

    /// 使う変数の名前。
    pub fn variables(&self) -> Vec<&str> {
        match self {
            DefaultAuth::ApiKey { variable, .. } | DefaultAuth::Bearer { variable, .. } => {
                vec![variable.as_str()]
            }
            DefaultAuth::Basic {
                username_variable,
                password_variable,
                ..
            } => vec![username_variable.as_str(), password_variable.as_str()],
        }
    }
}

// ---- API ツリー ----

pub fn list_item(
    op: &Operation,
    meta: &SpecMeta,
    options: &ImportOptions,
    deleted: bool,
    has_draft: bool,
) -> OperationListItem {
    OperationListItem {
        op_key: op.op_key.clone(),
        operation_id: op.operation_id.clone(),
        method: op.method.clone(),
        path: op.path.clone(),
        summary: op.summary.clone(),
        folder: if options.tag_folders {
            op.tags.first().cloned()
        } else {
            None
        },
        tags: op.tags.clone(),
        requires_auth: requires_auth(op, meta),
        deleted,
        has_draft,
    }
}

// ---- 設計タブ ----

fn media_doc(m: &MediaTypeSpec, for_request: bool) -> MediaDoc {
    let (label, rows, generated) = match &m.schema {
        Some(s) => {
            let table = schema_table(s);
            let mk = mock(s, for_request);
            (Some(table.label), table.rows, Some(mk))
        }
        None => (None, vec![], None),
    };
    let (example, source) = match (&m.example, &generated) {
        (Some(e), _) => (Some(e.clone()), Some(ExampleSource::Yaml)),
        (None, Some(g)) => (
            Some(g.value.clone()),
            Some(if g.used_schema_example {
                ExampleSource::SchemaExample
            } else {
                ExampleSource::Generated
            }),
        ),
        (None, None) => (None, None),
    };
    MediaDoc {
        content_type: m.content_type.clone(),
        schema_label: label,
        rows,
        example,
        example_source: source,
        generated_example: generated.map(|g| g.value),
    }
}

fn param_doc(p: &nekoreq_core::ParamSpec) -> ParamDoc {
    let m = p
        .schema
        .as_ref()
        .map(schema::merge_all_of)
        .unwrap_or(Value::Null);
    ParamDoc {
        name: p.name.clone(),
        location: p.location,
        required: p.required,
        type_label: if m.is_null() {
            "any".into()
        } else {
            schema::type_label(&m)
        },
        constraints: if m.is_null() {
            vec![]
        } else {
            schema::constraints(&m)
        },
        description: p.description.clone(),
        example: p.example.as_ref().map(value_text),
        default: p.default.as_ref().map(value_text),
    }
}

/// 設計タブの内容。`description` は「description を設計タブのドキュメントにする」がオフなら出さない。
pub fn describe_operation(
    op: &Operation,
    meta: &SpecMeta,
    options: &ImportOptions,
    imported_at: Option<String>,
    deleted: bool,
) -> OperationDetail {
    let all_codes: Vec<ErrorCodeEntry> = op
        .description
        .as_deref()
        .map(text::error_codes)
        .unwrap_or_default();
    OperationDetail {
        op_key: op.op_key.clone(),
        operation_id: op.operation_id.clone(),
        method: op.method.clone(),
        path: op.path.clone(),
        tags: op.tags.clone(),
        summary: op.summary.clone(),
        description: if options.description_as_doc {
            op.description.clone()
        } else {
            None
        },
        security: security_doc(op, meta),
        source_file: op.source_file.clone(),
        source_line: op.source_line,
        imported_at,
        parameters: op.parameters.iter().map(param_doc).collect(),
        request_body: op.request_body.as_ref().map(|rb| RequestBodyDoc {
            required: rb.required,
            description: rb.description.clone(),
            contents: rb.content.iter().map(|m| media_doc(m, true)).collect(),
        }),
        responses: op
            .responses
            .iter()
            .map(|r| ResponseDoc {
                status: r.status.clone(),
                description: r.description.clone(),
                contents: r.content.iter().map(|m| media_doc(m, false)).collect(),
                error_codes: all_codes
                    .iter()
                    .filter(|c| c.status == r.status)
                    .cloned()
                    .collect(),
            })
            .collect(),
        error_codes: all_codes,
        deleted,
    }
}

// ---- 送信タブを開く ----

fn is_json(ct: &str) -> bool {
    let ct = ct.to_ascii_lowercase();
    ct.contains("json")
}

fn pick_media(rb: &RequestBodySpec) -> Option<&MediaTypeSpec> {
    rb.content
        .iter()
        .find(|m| m.content_type.eq_ignore_ascii_case("application/json"))
        .or_else(|| rb.content.iter().find(|m| is_json(&m.content_type)))
        .or_else(|| rb.content.first())
}

/// ボディの例（yaml の example があればそれ、なければスキーマから作ったモック）。
fn body_example(m: &MediaTypeSpec) -> Option<Value> {
    if let Some(e) = &m.example {
        return Some(e.clone());
    }
    m.schema.as_ref().map(|s| mock(s, true).value)
}

fn is_binary_schema(s: &Value) -> bool {
    let m = schema::merge_all_of(s);
    m.get("format").and_then(Value::as_str) == Some("binary")
}

/// ボディの初期値と、使った Content-Type。`fill` が false なら中身は空（枠だけ）。
pub fn initial_body(rb: &RequestBodySpec, fill: bool) -> (Body, Option<String>) {
    let Some(m) = pick_media(rb) else {
        return (Body::None, None);
    };
    let ct = m.content_type.clone();
    let lower = ct.to_ascii_lowercase();
    let example = if fill { body_example(m) } else { None };
    let object_fields = || -> Vec<(String, Value, bool)> {
        // (名前, 値, ファイルか)
        let from_schema: Vec<(String, bool)> = m
            .schema
            .as_ref()
            .map(|s| {
                let merged = schema::merge_all_of(s);
                merged
                    .get("properties")
                    .and_then(Value::as_object)
                    .map(|p| {
                        p.iter()
                            .map(|(k, v)| (k.clone(), is_binary_schema(v)))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let mut out: Vec<(String, Value, bool)> = from_schema
            .into_iter()
            .map(|(k, bin)| {
                let v = example
                    .as_ref()
                    .and_then(|e| e.get(&k))
                    .cloned()
                    .unwrap_or(Value::Null);
                (k, v, bin)
            })
            .collect();
        if out.is_empty() {
            if let Some(Value::Object(o)) = &example {
                out = o
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone(), false))
                    .collect();
            }
        }
        out
    };
    let body = if is_json(&lower) {
        Body::Json(match &example {
            Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
            None => String::new(),
        })
    } else if lower.starts_with("application/x-www-form-urlencoded") {
        Body::Form(
            object_fields()
                .into_iter()
                .map(|(name, v, _)| KeyValue {
                    location: ParamIn::Query,
                    name,
                    value: value_text(&v),
                    enabled: true,
                })
                .collect(),
        )
    } else if lower.starts_with("multipart/") {
        Body::Multipart(
            object_fields()
                .into_iter()
                .map(|(name, v, is_file)| MultipartPart {
                    name,
                    value: if is_file {
                        MultipartValue::File(String::new())
                    } else {
                        MultipartValue::Text(value_text(&v))
                    },
                    // ファイルはパスを選ぶまで送らない
                    enabled: !is_file,
                })
                .collect(),
        )
    } else if lower.starts_with("text/") {
        Body::Text(example.as_ref().map(value_text).unwrap_or_default())
    } else {
        Body::Binary(String::new())
    };
    (body, Some(ct))
}

/// yaml のパラメータから作る初期の行。path パラメータは OpenAPI で常に必須なので有効、
/// それ以外（query・header・cookie）は無効の状態で並べる（設計書の対応表）。
pub fn initial_params(op: &Operation, fill: bool) -> Vec<KeyValue> {
    op.parameters
        .iter()
        .map(|p| KeyValue {
            location: p.location,
            name: p.name.clone(),
            value: if fill {
                p.example
                    .as_ref()
                    .or(p.default.as_ref())
                    .map(value_text)
                    .unwrap_or_default()
            } else {
                String::new()
            },
            enabled: p.location == ParamIn::Path,
        })
        .collect()
}

/// 送信タブを開くための初期状態。入力値（draft）があればそれを、なければ example・default から作る。
/// `stale` は yaml から消えたパラメータ（値は無効にして残す）。
pub fn open_operation(
    op: &Operation,
    meta: &SpecMeta,
    options: &ImportOptions,
    draft: Option<&RequestDraft>,
    stale: &[ParamRef],
    deleted: bool,
) -> OperationOpen {
    let is_stale = |p: &KeyValue| {
        stale
            .iter()
            .any(|s| s.location == p.location && s.name == p.name)
    };
    let (params, body, auth, body_ct) = match draft {
        Some(d) => {
            let mut params: Vec<KeyValue> = d
                .params
                .iter()
                .map(|p| {
                    let mut p = p.clone();
                    if is_stale(&p) {
                        p.enabled = false;
                    }
                    p
                })
                .collect();
            // yaml に新しく増えたパラメータは、無効の状態で足す
            for init in initial_params(op, options.fill_from_examples) {
                if !params
                    .iter()
                    .any(|p| p.location == init.location && p.name == init.name)
                {
                    params.push(init);
                }
            }
            let ct = op
                .request_body
                .as_ref()
                .and_then(pick_media)
                .map(|m| m.content_type.clone());
            (params, d.body.clone(), d.auth.clone(), ct)
        }
        None => {
            let (body, ct) = match &op.request_body {
                Some(rb) => initial_body(rb, options.fill_from_examples),
                None => (Body::None, None),
            };
            (
                initial_params(op, options.fill_from_examples),
                body,
                None,
                ct,
            )
        }
    };
    OperationOpen {
        op_key: op.op_key.clone(),
        method: op.method.clone(),
        path: op.path.clone(),
        summary: op.summary.clone(),
        request: RequestSpec {
            method: op.method.clone(),
            url: format!("{{{{baseUrl}}}}{}", op.path),
            params,
            body,
            auth,
        },
        has_draft: draft.is_some(),
        body_content_type: body_ct,
        missing_in_yaml: stale.to_vec(),
        required_params: op
            .parameters
            .iter()
            .filter(|p| p.required)
            .map(|p| ParamRef {
                location: p.location,
                name: p.name.clone(),
            })
            .collect(),
        deleted,
        default_auth: default_auth(op, meta, options),
    }
}
