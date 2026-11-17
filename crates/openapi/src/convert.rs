//! 読んだ文書（`DocSet`）から、メタ情報（タイトル・servers・認証の定義）と Operation を作る。
//!
//! - paths の PathItem が `$ref` で別のファイルを指していても辿る。各 Operation には、実際に書かれた
//!   ファイル（入口からの相対パス）と行番号を付ける。
//! - parameters は PathItem と Operation をマージする（同じ `name` + `in` は Operation が勝つ）。
//! - スキーマは `Inliner` で `$ref` を展開して持つ（`x-nekoreq-ref` に名前を残す）。
//! - `operationId` がない Operation のキーは `METHOD path`。operationId が重複するものは、安定するよう
//!   すべて `METHOD path` をキーにする。

use std::collections::HashMap;

use nekoreq_core::{
    ApiKeyIn, MediaTypeSpec, OpKey, Operation, ParamIn, ParamSpec, RequestBodySpec, ResponseSpec,
    SecurityRef, SecurityRequirement, SecuritySchemeInfo, SecuritySchemeKind, ServerInfo, SpecMeta,
    TagInfo,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::doc::DocSet;
use crate::model::{DuplicateOperationId, Inspection};
use crate::pointer;
use crate::schema::Inliner;
use crate::text;

/// OpenAPI が PathItem に置ける method（3.2 の `query` を含む）。
const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace", "query",
];

/// 変換の結果。
#[derive(Debug, Clone)]
pub struct Converted {
    pub meta: SpecMeta,
    pub openapi_version: String,
    pub operations: Vec<Operation>,
    pub inspection: Inspection,
}

fn text_of(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_security(v: Option<&Value>) -> Vec<SecurityRequirement> {
    let Some(Value::Array(items)) = v else {
        return vec![];
    };
    items
        .iter()
        .filter_map(Value::as_object)
        .map(|req| SecurityRequirement {
            schemes: req
                .iter()
                .map(|(name, scopes)| SecurityRef {
                    name: name.clone(),
                    scopes: scopes
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect(),
        })
        .collect()
}

struct Ctx<'a> {
    docs: &'a DocSet,
    inl: Inliner<'a>,
}

impl<'a> Ctx<'a> {
    /// `$ref` なら辿り、解決できなければ記録して `None`。
    fn deref(
        &mut self,
        file: usize,
        ptr: &str,
        node: &'a Value,
    ) -> Option<(usize, String, &'a Value)> {
        match node.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let docs = self.docs;
                match docs.resolve_chain(file, r) {
                    Ok(t) => Some((t.file, t.pointer, t.value)),
                    Err(f) => {
                        self.inl.record_unresolved(file, ptr, r, f.reason());
                        None
                    }
                }
            }
            None => Some((file, ptr.to_string(), node)),
        }
    }

    /// `example`、なければ `examples` の最初の値。
    fn first_example(&mut self, file: usize, ptr: &str, node: &'a Value) -> Option<Value> {
        if let Some(e) = node.get("example") {
            return Some(e.clone());
        }
        match node.get("examples")? {
            Value::Object(m) => {
                let (k, first) = m.iter().next()?;
                let eptr = pointer::child(&pointer::child(ptr, "examples"), k);
                let (_, _, ex) = self.deref(file, &eptr, first)?;
                ex.get("value").cloned()
            }
            Value::Array(a) => a.first().cloned(),
            _ => None,
        }
    }

    fn schema_of(&mut self, file: usize, ptr: &str, holder: &'a Value) -> Option<Value> {
        let schema = holder.get("schema")?;
        Some(
            self.inl
                .inline(file, schema, &pointer::child(ptr, "schema")),
        )
    }

    fn parameter(&mut self, file: usize, ptr: &str, node: &'a Value) -> Option<ParamSpec> {
        let (file, ptr, node) = self.deref(file, ptr, node)?;
        let name = text_of(node, "name")?;
        let location = match node.get("in").and_then(Value::as_str)? {
            "query" => ParamIn::Query,
            "path" => ParamIn::Path,
            "header" => ParamIn::Header,
            "cookie" => ParamIn::Cookie,
            _ => return None,
        };
        let mut schema = self.schema_of(file, &ptr, node);
        if schema.is_none() {
            // `schema` の代わりに `content` を使う書き方
            if let Some(Value::Object(content)) = node.get("content") {
                if let Some((ct, media)) = content.iter().next() {
                    let mptr = pointer::child(&pointer::child(&ptr, "content"), ct);
                    schema = self.schema_of(file, &mptr, media);
                }
            }
        }
        let example = self
            .first_example(file, &ptr, node)
            .or_else(|| schema.as_ref().and_then(|s| s.get("example").cloned()));
        let default = schema.as_ref().and_then(|s| s.get("default").cloned());
        Some(ParamSpec {
            name,
            location,
            required: location == ParamIn::Path
                || node.get("required").and_then(Value::as_bool) == Some(true),
            description: text_of(node, "description"),
            example,
            default,
            schema,
        })
    }

    fn content(&mut self, file: usize, ptr: &str, node: &'a Value) -> Vec<MediaTypeSpec> {
        let Some(Value::Object(content)) = node.get("content") else {
            return vec![];
        };
        let cptr = pointer::child(ptr, "content");
        content
            .iter()
            .map(|(ct, media)| {
                let mptr = pointer::child(&cptr, ct);
                // メディア型が `$ref` のことは（3.2 の mediaTypes）、辿れるときだけ辿る
                let (mfile, mptr, media) = self
                    .deref(file, &mptr, media)
                    .unwrap_or((file, mptr, media));
                MediaTypeSpec {
                    content_type: ct.clone(),
                    schema: self.schema_of(mfile, &mptr, media),
                    example: self.first_example(mfile, &mptr, media),
                }
            })
            .collect()
    }

    fn request_body(&mut self, file: usize, ptr: &str, node: &'a Value) -> Option<RequestBodySpec> {
        let (file, ptr, rb) = self.deref(file, ptr, node)?;
        Some(RequestBodySpec {
            required: rb.get("required").and_then(Value::as_bool) == Some(true),
            description: text_of(rb, "description"),
            content: self.content(file, &ptr, rb),
        })
    }

    fn responses(&mut self, file: usize, ptr: &str, node: &'a Value) -> Vec<ResponseSpec> {
        let Some(Value::Object(map)) = node.get("responses") else {
            return vec![];
        };
        let rptr = pointer::child(ptr, "responses");
        let mut out = Vec::new();
        for (status, resp) in map {
            let sptr = pointer::child(&rptr, status);
            let Some((rfile, sptr, r)) = self.deref(file, &sptr, resp) else {
                continue;
            };
            out.push(ResponseSpec {
                status: status.clone(),
                description: text_of(r, "description"),
                content: self.content(rfile, &sptr, r),
            });
        }
        out
    }

    /// PathItem / Operation の `parameters` を読む。
    fn parameters(&mut self, file: usize, ptr: &str, holder: &'a Value) -> Vec<ParamSpec> {
        let Some(Value::Array(items)) = holder.get("parameters") else {
            return vec![];
        };
        let pptr = pointer::child(ptr, "parameters");
        items
            .iter()
            .enumerate()
            .filter_map(|(i, p)| self.parameter(file, &pointer::child_index(&pptr, i), p))
            .collect()
    }
}

fn read_meta(docs: &DocSet) -> SpecMeta {
    let root = &docs.entry().value;
    let info = root.get("info").cloned().unwrap_or(Value::Null);
    let servers = root
        .get("servers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let mut url = text_of(s, "url")?;
                    if let Some(Value::Object(vars)) = s.get("variables") {
                        for (name, v) in vars {
                            if let Some(d) = text_of(v, "default") {
                                url = url.replace(&format!("{{{name}}}"), &d);
                            }
                        }
                    }
                    Some(ServerInfo {
                        url,
                        description: text_of(s, "description"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let tags = root
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some(TagInfo {
                        name: text_of(t, "name")?,
                        description: text_of(t, "description"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let mut schemes = Vec::new();
    if let Some(Value::Object(m)) = root
        .get("components")
        .and_then(|c| c.get("securitySchemes"))
    {
        for (name, node) in m {
            let node = match node.get("$ref").and_then(Value::as_str) {
                Some(r) => match docs.resolve_chain(0, r) {
                    Ok(t) => t.value,
                    Err(_) => continue,
                },
                None => node,
            };
            let kind = match text_of(node, "type").as_deref() {
                Some("apiKey") => SecuritySchemeKind::ApiKey,
                Some("http") => SecuritySchemeKind::Http,
                Some("oauth2") => SecuritySchemeKind::Oauth2,
                Some("openIdConnect") => SecuritySchemeKind::OpenIdConnect,
                Some("mutualTLS") => SecuritySchemeKind::MutualTls,
                _ => SecuritySchemeKind::Other,
            };
            let location = match text_of(node, "in").as_deref() {
                Some("header") => Some(ApiKeyIn::Header),
                Some("query") => Some(ApiKeyIn::Query),
                Some("cookie") => Some(ApiKeyIn::Cookie),
                _ => None,
            };
            schemes.push(SecuritySchemeInfo {
                name: name.clone(),
                kind,
                location,
                key_name: text_of(node, "name"),
                scheme: text_of(node, "scheme").map(|s| s.to_ascii_lowercase()),
                bearer_format: text_of(node, "bearerFormat"),
                description: text_of(node, "description"),
            });
        }
    }

    SpecMeta {
        title: text_of(&info, "title").unwrap_or_default(),
        version: text_of(&info, "version").unwrap_or_default(),
        description: text_of(&info, "description"),
        servers,
        security_schemes: schemes,
        root_security: parse_security(root.get("security")),
        tags,
    }
}

fn hash_operation(op: &Operation) -> String {
    // 書かれた場所（ファイル・行）は含めない（行がずれただけで「変更」にならないように）
    let v = json!({
        "operationId": op.operation_id,
        "method": op.method,
        "path": op.path,
        "tags": op.tags,
        "summary": op.summary,
        "description": op.description,
        "parameters": op.parameters,
        "requestBody": op.request_body,
        "responses": op.responses,
        "security": op.security,
    });
    let bytes = serde_json::to_vec(&v).unwrap_or_default();
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 文書全体を変換する。
pub fn convert(docs: &DocSet) -> Converted {
    let meta = read_meta(docs);
    let root = &docs.entry().value;
    let openapi_version = text_of(root, "openapi").unwrap_or_default();
    let mut cx = Ctx {
        docs,
        inl: Inliner::new(docs),
    };
    let mut ops: Vec<Operation> = Vec::new();

    if let Some(Value::Object(paths)) = root.get("paths") {
        for (path, item) in paths {
            let item_ptr = pointer::child("/paths", path);
            let Some((ifile, iptr, ival)) = cx.deref(0, &item_ptr, item) else {
                continue;
            };
            let path_params = cx.parameters(ifile, &iptr, ival);

            let mut methods: Vec<(String, String, &Value)> = Vec::new();
            for m in METHODS {
                if let Some(op) = ival.get(*m).filter(|v| v.is_object()) {
                    methods.push((m.to_ascii_uppercase(), pointer::child(&iptr, m), op));
                }
            }
            // 3.2 の additionalOperations（任意の method 名）
            if let Some(Value::Object(extra)) = ival.get("additionalOperations") {
                let base = pointer::child(&iptr, "additionalOperations");
                for (m, op) in extra.iter().filter(|(_, v)| v.is_object()) {
                    methods.push((m.to_ascii_uppercase(), pointer::child(&base, m), op));
                }
            }

            for (method, optr, op) in methods {
                let mut params = path_params.clone();
                for p in cx.parameters(ifile, &optr, op) {
                    match params
                        .iter_mut()
                        .find(|e| e.name == p.name && e.location == p.location)
                    {
                        Some(slot) => *slot = p,
                        None => params.push(p),
                    }
                }
                let request_body = op.get("requestBody").and_then(|rb| {
                    cx.request_body(ifile, &pointer::child(&optr, "requestBody"), rb)
                });
                let responses = cx.responses(ifile, &optr, op);
                let operation_id = text_of(op, "operationId").filter(|s| !s.is_empty());
                let tags = op
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                let doc = &docs.files[ifile];
                let mut operation = Operation {
                    id: String::new(),
                    project_id: String::new(),
                    op_key: match &operation_id {
                        Some(id) => OpKey::Id(id.clone()),
                        None => OpKey::method_path(&method, path),
                    },
                    operation_id,
                    method,
                    path: path.clone(),
                    tags,
                    summary: text_of(op, "summary"),
                    description: text_of(op, "description"),
                    parameters: params,
                    request_body,
                    responses,
                    security: op.get("security").map(|s| parse_security(Some(s))),
                    source_file: Some(doc.display.clone()),
                    source_line: doc.line_of(&optr),
                    content_hash: String::new(),
                };
                operation.content_hash = hash_operation(&operation);
                ops.push(operation);
            }
        }
    }

    // operationId の重複: すべて METHOD path をキーにする
    let mut by_id: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, o) in ops.iter().enumerate() {
        if let Some(id) = &o.operation_id {
            by_id.entry(id.clone()).or_default().push(i);
        }
    }
    let mut duplicates: Vec<DuplicateOperationId> = by_id
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(id, idx)| {
            for &i in &idx {
                ops[i].op_key = OpKey::method_path(&ops[i].method, &ops[i].path);
            }
            DuplicateOperationId {
                operation_id: id,
                operations: idx
                    .iter()
                    .map(|&i| format!("{} {}", ops[i].method, ops[i].path))
                    .collect(),
            }
        })
        .collect();
    duplicates.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));

    let inspection = Inspection {
        unresolved_refs: std::mem::take(&mut cx.inl.issues),
        duplicate_operation_ids: duplicates,
        no_security_count: ops
            .iter()
            .filter(|o| matches!(&o.security, Some(s) if s.is_empty()))
            .count() as u32,
        markdown_table_count: ops
            .iter()
            .filter(|o| {
                o.description
                    .as_deref()
                    .is_some_and(text::has_markdown_table)
            })
            .count() as u32,
    };

    Converted {
        meta,
        openapi_version,
        operations: ops,
        inspection,
    }
}
