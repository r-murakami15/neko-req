//! スキーマの取り扱い。
//!
//! 1. `Inliner`: `$ref` を解決しながらスキーマをその場に展開する（`$ref` の名前は `x-nekoreq-ref` に残す。
//!    循環は 2 回目で止めて `x-nekoreq-circular: true` を付ける。解決できないものは `x-nekoreq-unresolved`。
//!    1 Operation あたりの展開ノード数・入れ子の深さに上限があり、超えたらそこで止めて
//!    `x-nekoreq-truncated: true` を付ける）。
//! 2. `schema_table`: 展開したスキーマから、深さつきの行のリスト（名前・型・必須・制約・説明）を作る。
//!    allOf はまとめる。
//! 3. `mock`: スキーマから例を作る（スキーマの example・default・enum があればそれを使う）。

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::doc::{DocSet, Target};
use crate::model::{RefSite, SchemaRow, UnresolvedRef};
use crate::pointer;

pub const REF_KEY: &str = "x-nekoreq-ref";
pub const CIRCULAR_KEY: &str = "x-nekoreq-circular";
pub const UNRESOLVED_KEY: &str = "x-nekoreq-unresolved";
/// 大きすぎて展開を打ち切った印。
pub const TRUNCATED_KEY: &str = "x-nekoreq-truncated";

/// スキーマ表の行の説明に入れる文言（展開を打ち切ったとき）。
pub const TRUNCATED_NOTE: &str = "（大きすぎるため省略）";

/// 1 Operation あたりに展開するスキーマのノード数の上限。扇状に `$ref` する定義が指数的に膨れるのを止める。
pub const MAX_NODES: usize = 20_000;
/// 1 Operation あたりに複製するデータ（example・enum など）のノード数の上限。
const MAX_DATA_NODES: usize = 400_000;
/// 展開するスキーマの入れ子の深さの上限（保存・表示で再帰が深くなりすぎないように）。
const MAX_INLINE_DEPTH: usize = 100;

/// 展開の深さの上限（循環以外の暴走防止）。
const MAX_DEPTH: usize = 40;

/// スキーマを値に持つキーワード。
const SCHEMA_KEYS: &[&str] = &[
    "items",
    "additionalProperties",
    "additionalItems",
    "not",
    "if",
    "then",
    "else",
    "contains",
    "propertyNames",
    "unevaluatedProperties",
    "unevaluatedItems",
    "contentSchema",
];
const SCHEMA_LIST_KEYS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
const SCHEMA_MAP_KEYS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

/// `$ref` の名前（ポインターの最後の区間。なければファイル名）。
pub fn ref_name(docs: &DocSet, t: &Target<'_>) -> String {
    pointer::last_segment(&t.pointer).unwrap_or_else(|| {
        let d = &docs.files[t.file].display;
        let base = d.rsplit('/').next().unwrap_or(d);
        base.split('.').next().unwrap_or(base).to_string()
    })
}

/// `$ref` を展開する。
pub struct Inliner<'a> {
    docs: &'a DocSet,
    stack: Vec<(usize, String)>,
    pub issues: Vec<UnresolvedRef>,
    seen: HashSet<(String, usize, String)>,
    /// 残りのスキーマのノード数・データのノード数・今の入れ子の深さ。
    nodes_left: usize,
    data_left: usize,
    depth: usize,
}

/// 打ち切ったところに置く値。
fn truncated_value() -> Value {
    let mut o = Map::new();
    o.insert(TRUNCATED_KEY.into(), Value::Bool(true));
    Value::Object(o)
}

/// `v` の中のノード数（`cap` で数えるのをやめる）。
fn count_nodes(v: &Value, cap: usize) -> usize {
    fn go(v: &Value, n: &mut usize, cap: usize) {
        *n += 1;
        if *n >= cap {
            return;
        }
        match v {
            Value::Array(a) => {
                for c in a {
                    go(c, n, cap);
                    if *n >= cap {
                        return;
                    }
                }
            }
            Value::Object(m) => {
                for c in m.values() {
                    go(c, n, cap);
                    if *n >= cap {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
    let mut n = 0;
    go(v, &mut n, cap);
    n
}

impl<'a> Inliner<'a> {
    pub fn new(docs: &'a DocSet) -> Self {
        Self {
            docs,
            stack: Vec::new(),
            issues: Vec::new(),
            seen: HashSet::new(),
            nodes_left: MAX_NODES,
            data_left: MAX_DATA_NODES,
            depth: 0,
        }
    }

    /// 1 つの Operation（PathItem の共通パラメータを含む）の変換を始める。展開の上限を数え直す。
    pub fn begin_operation(&mut self) {
        self.nodes_left = MAX_NODES;
        self.data_left = MAX_DATA_NODES;
        self.depth = 0;
        self.stack.clear();
    }

    /// 値をそのまま複製する（example・enum など）。複製した大きさを上限から引く。
    fn clone_data(&mut self, v: &Value) -> Value {
        if v.is_array() || v.is_object() {
            self.data_left = self
                .data_left
                .saturating_sub(count_nodes(v, MAX_DATA_NODES + 1));
        }
        v.clone()
    }

    /// `file` に書かれたスキーマ `node`（ポインター `ptr`）を展開する。
    pub fn inline(&mut self, file: usize, node: &Value, ptr: &str) -> Value {
        let Value::Object(m) = node else {
            return node.clone();
        };
        // 上限を超えたら、そこで止める（以降のノードも同じ）
        if self.nodes_left == 0 || self.data_left == 0 || self.depth >= MAX_INLINE_DEPTH {
            return truncated_value();
        }
        self.nodes_left -= 1;
        self.depth += 1;
        let v = if let Some(Value::String(r)) = m.get("$ref") {
            self.reference(file, m, r, ptr)
        } else {
            let mut out = Map::with_capacity(m.len());
            for (k, child) in m {
                let cptr = pointer::child(ptr, k);
                out.insert(k.clone(), self.keyword(file, k, child, &cptr));
            }
            Value::Object(out)
        };
        self.depth -= 1;
        v
    }

    /// 解決できなかった `$ref` を記録する（同じ場所は 1 回だけ）。
    pub fn record_unresolved(&mut self, file: usize, ptr: &str, reference: &str, reason: String) {
        let key = (reference.to_string(), file, ptr.to_string());
        if !self.seen.insert(key) {
            return;
        }
        let doc = &self.docs.files[file];
        self.issues.push(UnresolvedRef {
            reference: reference.to_string(),
            from: RefSite {
                file: doc.display.clone(),
                line: doc.line_of(&format!("{ptr}/$ref")),
            },
            reason,
        });
    }

    fn keyword(&mut self, file: usize, key: &str, child: &Value, ptr: &str) -> Value {
        if SCHEMA_KEYS.contains(&key) {
            match child {
                Value::Array(items) => Value::Array(
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, v)| self.inline(file, v, &pointer::child_index(ptr, i)))
                        .collect(),
                ),
                other => self.inline(file, other, ptr),
            }
        } else if SCHEMA_LIST_KEYS.contains(&key) {
            match child {
                Value::Array(items) => Value::Array(
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, v)| self.inline(file, v, &pointer::child_index(ptr, i)))
                        .collect(),
                ),
                other => self.clone_data(other),
            }
        } else if SCHEMA_MAP_KEYS.contains(&key) {
            match child {
                Value::Object(m) => {
                    let mut out = Map::with_capacity(m.len());
                    for (k, v) in m {
                        out.insert(k.clone(), self.inline(file, v, &pointer::child(ptr, k)));
                    }
                    Value::Object(out)
                }
                other => self.clone_data(other),
            }
        } else {
            self.clone_data(child)
        }
    }

    fn reference(&mut self, file: usize, m: &Map<String, Value>, r: &str, ptr: &str) -> Value {
        let docs = self.docs;
        let target = match docs.resolve_chain(file, r) {
            Ok(t) => t,
            Err(f) => {
                self.record_unresolved(file, ptr, r, f.reason());
                return json!({ UNRESOLVED_KEY: r });
            }
        };
        let name = ref_name(docs, &target);
        let key = (target.file, target.pointer.clone());
        let base = if self.stack.contains(&key) || self.stack.len() >= MAX_DEPTH {
            let mut o = Map::new();
            o.insert(CIRCULAR_KEY.into(), Value::Bool(true));
            Value::Object(o)
        } else {
            self.stack.push(key);
            let v = self.inline(target.file, target.value, &target.pointer);
            self.stack.pop();
            v
        };
        let mut obj = match base {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        obj.insert(REF_KEY.into(), Value::String(name));
        // `$ref` と並んで書かれたキー（3.1 の description など）は上書きする
        for (k, child) in m {
            if k == "$ref" {
                continue;
            }
            let cptr = pointer::child(ptr, k);
            obj.insert(k.clone(), self.keyword(file, k, child, &cptr));
        }
        Value::Object(obj)
    }
}

// ---- allOf のまとめ・型の判定 ----

fn str_of<'a>(m: &'a Value, key: &str) -> Option<&'a str> {
    m.get(key).and_then(Value::as_str)
}

/// allOf を 1 つのスキーマにまとめる（properties・required は和集合、そのほかは自分の値を優先）。
pub fn merge_all_of(s: &Value) -> Value {
    let Value::Object(m) = s else {
        return s.clone();
    };
    let Some(Value::Array(parts)) = m.get("allOf") else {
        return s.clone();
    };
    let mut out = Map::new();
    for (k, v) in m {
        if k != "allOf" {
            out.insert(k.clone(), v.clone());
        }
    }
    for part in parts {
        merge_into(&mut out, &merge_all_of(part));
    }
    Value::Object(out)
}

fn merge_into(out: &mut Map<String, Value>, part: &Value) {
    let Value::Object(pm) = part else { return };
    for (k, v) in pm {
        match k.as_str() {
            "properties" => {
                if let Value::Object(props) = v {
                    let entry = out
                        .entry("properties")
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Value::Object(dst) = entry {
                        for (pk, pv) in props {
                            if !dst.contains_key(pk) {
                                dst.insert(pk.clone(), pv.clone());
                            }
                        }
                    }
                }
            }
            "required" => {
                if let Value::Array(req) = v {
                    let entry = out
                        .entry("required")
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Value::Array(dst) = entry {
                        for r in req {
                            if !dst.contains(r) {
                                dst.push(r.clone());
                            }
                        }
                    }
                }
            }
            REF_KEY => {}
            CIRCULAR_KEY | TRUNCATED_KEY => {
                if !out.contains_key("properties") && !out.contains_key("type") {
                    out.insert(k.clone(), v.clone());
                }
            }
            _ => {
                if !out.contains_key(k) {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

/// `type` の最初の null でないもの。なければ形から推測する。
pub fn base_type(m: &Value) -> String {
    match m.get("type") {
        Some(Value::String(t)) => return t.clone(),
        Some(Value::Array(ts)) => {
            if let Some(t) = ts.iter().filter_map(Value::as_str).find(|t| *t != "null") {
                return t.to_string();
            }
        }
        _ => {}
    }
    if m.get("properties").is_some() {
        "object".into()
    } else if m.get("items").is_some() {
        "array".into()
    } else if m.get("oneOf").is_some() || m.get("anyOf").is_some() {
        "oneOf".into()
    } else if m.get("enum").is_some() {
        "string".into()
    } else if m.get(CIRCULAR_KEY).is_some() {
        "object".into()
    } else {
        "any".into()
    }
}

pub fn is_nullable(m: &Value) -> bool {
    if m.get("nullable").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    matches!(m.get("type"), Some(Value::Array(ts)) if ts.iter().any(|t| t.as_str() == Some("null")))
}

pub fn is_circular(m: &Value) -> bool {
    m.get(CIRCULAR_KEY).and_then(Value::as_bool) == Some(true)
}

/// 大きすぎて展開を打ち切ったスキーマか。
pub fn is_truncated(m: &Value) -> bool {
    m.get(TRUNCATED_KEY).and_then(Value::as_bool) == Some(true)
}

/// 画面に出す型。`$ref` の名前があればそれ。
pub fn type_label(m: &Value) -> String {
    if let Some(name) = str_of(m, REF_KEY) {
        return name.to_string();
    }
    match base_type(m).as_str() {
        "array" => {
            let inner = match m.get("items") {
                Some(items @ Value::Object(_)) => type_label(&merge_all_of(items)),
                _ => "any".to_string(),
            };
            format!("{inner}[]")
        }
        "oneOf" => {
            let alts = m
                .get("oneOf")
                .or_else(|| m.get("anyOf"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| type_label(&merge_all_of(x)))
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default();
            if alts.is_empty() {
                "oneOf".into()
            } else {
                alts
            }
        }
        "object" => match m.get("additionalProperties") {
            Some(v @ Value::Object(_)) if m.get("properties").is_none() => {
                format!("map<{}>", type_label(&merge_all_of(v)))
            }
            _ => "object".into(),
        },
        t => t.to_string(),
    }
}

fn fmt_value(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() > 80 {
        let cut: String = s.chars().take(80).collect();
        format!("{cut}…")
    } else {
        s
    }
}

/// 制約の表示（`nullable`・`enum`・`format`・最小最大・長さ・`pattern`・`default` など）。
pub fn constraints(m: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if is_nullable(m) {
        out.push("nullable".to_string());
    }
    if let Some(Value::Array(e)) = m.get("enum") {
        let shown: Vec<String> = e.iter().take(12).map(fmt_value).collect();
        let more = if e.len() > 12 { ", …" } else { "" };
        out.push(format!("enum: {}{more}", shown.join(", ")));
    }
    if let Some(c) = m.get("const") {
        out.push(format!("const: {}", fmt_value(c)));
    }
    if let Some(f) = str_of(m, "format") {
        out.push(format!("format: {f}"));
    }
    // 3.0 の exclusiveMinimum / exclusiveMaximum は真偽値（minimum / maximum を含まない）。「> 0」「< 10」と出す
    let exclusive = |key: &str| m.get(key).and_then(Value::as_bool) == Some(true);
    for (key, label) in [
        ("minimum", "min"),
        ("maximum", "max"),
        ("exclusiveMinimum", "exclusiveMin"),
        ("exclusiveMaximum", "exclusiveMax"),
        ("multipleOf", "multipleOf"),
        ("minLength", "minLength"),
        ("maxLength", "maxLength"),
        ("minItems", "minItems"),
        ("maxItems", "maxItems"),
        ("minProperties", "minProperties"),
        ("maxProperties", "maxProperties"),
    ] {
        if let Some(v) = m.get(key) {
            if v.is_boolean() {
                continue;
            }
            if key == "minimum" && exclusive("exclusiveMinimum") {
                out.push(format!("> {}", fmt_value(v)));
            } else if key == "maximum" && exclusive("exclusiveMaximum") {
                out.push(format!("< {}", fmt_value(v)));
            } else {
                out.push(format!("{label}: {}", fmt_value(v)));
            }
        }
    }
    if let Some(p) = str_of(m, "pattern") {
        out.push(format!("pattern: {p}"));
    }
    if m.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
        out.push("uniqueItems".into());
    }
    if let Some(d) = m.get("default") {
        out.push(format!("default: {}", fmt_value(d)));
    }
    if m.get("readOnly").and_then(Value::as_bool) == Some(true) {
        out.push("readOnly".into());
    }
    if m.get("writeOnly").and_then(Value::as_bool) == Some(true) {
        out.push("writeOnly".into());
    }
    if m.get("deprecated").and_then(Value::as_bool) == Some(true) {
        out.push("deprecated".into());
    }
    if is_circular(m) {
        out.push("循環".into());
    }
    if is_truncated(m) {
        out.push("省略".into());
    }
    out
}

// ---- スキーマ表 ----

/// スキーマ表（ルートの型の表示と、深さつきの行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaTable {
    pub label: String,
    pub rows: Vec<SchemaRow>,
}

#[derive(PartialEq, Eq)]
enum Kind {
    Object,
    Array,
    Other,
}

fn kind(m: &Value) -> Kind {
    match base_type(m).as_str() {
        "object" => Kind::Object,
        "array" => Kind::Array,
        _ => Kind::Other,
    }
}

fn depth_u32(d: usize) -> u32 {
    u32::try_from(d).unwrap_or(u32::MAX)
}

fn example_text(m: &Value) -> Option<String> {
    let v = m.get("example").or_else(|| {
        m.get("examples")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
    })?;
    Some(match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn row_for(name: &str, m: &Value, required: bool, depth: usize) -> SchemaRow {
    let description = match (str_of(m, "description"), is_truncated(m)) {
        (Some(d), true) => Some(format!("{TRUNCATED_NOTE} {d}")),
        (None, true) => Some(TRUNCATED_NOTE.to_string()),
        (d, false) => d.map(str::to_string),
    };
    SchemaRow {
        depth: depth_u32(depth),
        name: name.to_string(),
        type_label: type_label(m),
        base_type: base_type(m),
        required,
        nullable: is_nullable(m),
        constraints: constraints(m),
        description,
        example: example_text(m),
        ref_name: str_of(m, REF_KEY).map(str::to_string),
        circular: is_circular(m),
    }
}

fn push_children(m: &Value, depth: usize, rows: &mut Vec<SchemaRow>) {
    if is_circular(m) || is_truncated(m) || depth > 60 {
        return;
    }
    match kind(m) {
        Kind::Object => {
            let required: HashSet<&str> = m
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if let Some(Value::Object(props)) = m.get("properties") {
                for (name, ps) in props {
                    let pm = merge_all_of(ps);
                    rows.push(row_for(name, &pm, required.contains(name.as_str()), depth));
                    push_children(&pm, depth + 1, rows);
                }
            }
            if let Some(ap @ Value::Object(_)) = m.get("additionalProperties") {
                let pm = merge_all_of(ap);
                rows.push(row_for("*", &pm, false, depth));
                push_children(&pm, depth + 1, rows);
            }
        }
        Kind::Array => {
            if let Some(items @ Value::Object(_)) = m.get("items") {
                let im = merge_all_of(items);
                if is_circular(&im) {
                    rows.push(row_for("[]", &im, false, depth));
                } else {
                    push_children(&im, depth, rows);
                }
            }
        }
        Kind::Other => {}
    }
}

/// スキーマ（`Inliner` で展開したもの）から表を作る。ルートが object なら、その properties が深さ 0 の行になる。
/// ルートが配列なら、要素の properties が深さ 0。スカラーなら行は空。
pub fn schema_table(schema: &Value) -> SchemaTable {
    let m = merge_all_of(schema);
    let mut rows = Vec::new();
    push_children(&m, 0, &mut rows);
    SchemaTable {
        label: type_label(&m),
        rows,
    }
}

// ---- モック ----

/// スキーマから作った例。
#[derive(Debug, Clone, PartialEq)]
pub struct Mock {
    pub value: Value,
    /// スキーマの example・default・enum を 1 つでも使った。
    pub used_schema_example: bool,
}

/// スキーマから例を作る。`for_request` が true のときは readOnly の項目を除き、false のときは writeOnly を除く。
pub fn mock(schema: &Value, for_request: bool) -> Mock {
    let mut used = false;
    let mut budget = MOCK_NODES;
    let value = gen(schema, 0, for_request, &mut used, &mut budget);
    Mock {
        value,
        used_schema_example: used,
    }
}

/// モックで作る値のノード数の上限（minItems を重ねて膨れないように）。
const MOCK_NODES: usize = 5_000;
/// モックの配列の要素数の上限。
const MOCK_ITEMS: u64 = 10;

fn gen(s: &Value, depth: usize, req: bool, used: &mut bool, budget: &mut usize) -> Value {
    if depth > 14 || *budget == 0 {
        return Value::Null;
    }
    *budget -= 1;
    let m = merge_all_of(s);
    if is_circular(&m) || is_truncated(&m) || m.get(UNRESOLVED_KEY).is_some() {
        return Value::Null;
    }
    if let Some(e) = m.get("example").or_else(|| {
        m.get("examples")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
    }) {
        *used = true;
        return e.clone();
    }
    if let Some(c) = m.get("const") {
        *used = true;
        return c.clone();
    }
    if let Some(d) = m.get("default") {
        *used = true;
        return d.clone();
    }
    if let Some(e) = m
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        *used = true;
        return e.clone();
    }
    if let Some(alts) = m
        .get("oneOf")
        .or_else(|| m.get("anyOf"))
        .and_then(Value::as_array)
    {
        let pick = alts
            .iter()
            .find(|a| a.get("type").and_then(Value::as_str) != Some("null"))
            .or_else(|| alts.first());
        return pick.map_or(Value::Null, |a| gen(a, depth + 1, req, used, budget));
    }
    match base_type(&m).as_str() {
        "object" => {
            let mut out = Map::new();
            if let Some(Value::Object(props)) = m.get("properties") {
                for (name, ps) in props {
                    let pm = merge_all_of(ps);
                    let ro = pm.get("readOnly").and_then(Value::as_bool) == Some(true);
                    let wo = pm.get("writeOnly").and_then(Value::as_bool) == Some(true);
                    if (req && ro) || (!req && wo) {
                        continue;
                    }
                    out.insert(name.clone(), gen(&pm, depth + 1, req, used, budget));
                }
            }
            Value::Object(out)
        }
        "array" => {
            let min = m.get("minItems").and_then(Value::as_u64).unwrap_or(0);
            let max = m.get("maxItems").and_then(Value::as_u64);
            // 既定は 1 要素。minItems があればその数、maxItems を超えない
            let mut n = min.clamp(1, MOCK_ITEMS);
            if let Some(max) = max {
                n = n.min(max);
            }
            match m.get("items") {
                Some(items) => Value::Array(
                    (0..n)
                        .map(|_| gen(items, depth + 1, req, used, budget))
                        .collect(),
                ),
                None => Value::Array(vec![]),
            }
        }
        "string" => Value::String(string_example(&m)),
        "integer" => json!(number_floor(&m)),
        "number" => json!(number_floor(&m)),
        "boolean" => Value::Bool(false),
        "null" => Value::Null,
        _ => Value::Null,
    }
}

/// 数値の例。下限があれば下限、なければ 0。上限（`maximum`・`exclusiveMaximum`）を超えないようにする。
/// 3.0 の真偽値の `exclusiveMinimum` / `exclusiveMaximum` も見る。
fn number_floor(m: &Value) -> i64 {
    let num = |k: &str| m.get(k).and_then(Value::as_f64);
    let flag = |k: &str| m.get(k).and_then(Value::as_bool) == Some(true);
    let mut lo: Option<i64> = None;
    if let Some(v) = num("minimum") {
        lo = Some(if flag("exclusiveMinimum") {
            v.floor() as i64 + 1
        } else {
            v.ceil() as i64
        });
    }
    if let Some(v) = num("exclusiveMinimum") {
        let e = v.floor() as i64 + 1;
        lo = Some(lo.map_or(e, |l| l.max(e)));
    }
    let mut hi: Option<i64> = None;
    if let Some(v) = num("maximum") {
        hi = Some(if flag("exclusiveMaximum") {
            v.ceil() as i64 - 1
        } else {
            v.floor() as i64
        });
    }
    if let Some(v) = num("exclusiveMaximum") {
        let e = v.ceil() as i64 - 1;
        hi = Some(hi.map_or(e, |h| h.min(e)));
    }
    match (lo, hi) {
        (Some(l), _) => l,
        (None, Some(h)) if h < 0 => h,
        _ => 0,
    }
}

fn string_example(m: &Value) -> String {
    let mut s = match str_of(m, "format") {
        Some("date-time") => "2026-01-01T00:00:00+09:00".to_string(),
        Some("date") => "2026-01-01".to_string(),
        Some("time") => "00:00:00".to_string(),
        Some("email") => "user@example.com".to_string(),
        Some("uuid") => "3fa85f64-5717-4562-b3fc-2c963f66afa6".to_string(),
        Some("uri" | "url") => "https://example.com".to_string(),
        Some("hostname") => "example.com".to_string(),
        Some("ipv4") => "192.0.2.1".to_string(),
        Some("ipv6") => "2001:db8::1".to_string(),
        Some("password") => "********".to_string(),
        Some("byte") => "c3RyaW5n".to_string(),
        Some("binary") => "(binary)".to_string(),
        _ => "string".to_string(),
    };
    if let Some(min) = m.get("minLength").and_then(Value::as_u64) {
        let min = usize::try_from(min).unwrap_or(0).min(10_000);
        while s.chars().count() < min {
            s.push('x');
        }
    }
    if let Some(max) = m.get("maxLength").and_then(Value::as_u64) {
        let max = usize::try_from(max).unwrap_or(usize::MAX);
        if s.chars().count() > max {
            s = s.chars().take(max).collect();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_of_is_merged_with_union_of_properties_and_required() {
        let s = json!({
            "allOf": [
                {"type": "object", "required": ["a"], "properties": {"a": {"type": "string"}}},
                {"required": ["b"], "properties": {"b": {"type": "integer"}}}
            ],
            "description": "d"
        });
        let m = merge_all_of(&s);
        assert_eq!(m["required"], json!(["a", "b"]));
        assert_eq!(m["properties"].as_object().unwrap().len(), 2);
        assert_eq!(m["description"], "d");
    }

    #[test]
    fn table_has_depth_ref_names_and_constraints() {
        let s = json!({
            REF_KEY: "ClientList",
            "type": "object",
            "required": ["items"],
            "properties": {
                "items": {"type": "array", "items": {
                    REF_KEY: "ClientListItem", "type": "object", "required": ["id"],
                    "properties": {
                        "id": {"type": "integer", "format": "int64", "minimum": 1, "description": "ID", "example": 7},
                        "status": {REF_KEY: "ClientStatus", "type": "string", "enum": ["ACTIVE", "INACTIVE"]},
                        "memo": {"type": "string", "nullable": true, "maxLength": 20}
                    }}},
                "total": {"type": "integer"}
            }
        });
        let t = schema_table(&s);
        assert_eq!(t.label, "ClientList");
        let rows: Vec<_> = t
            .rows
            .iter()
            .map(|r| (r.depth, r.name.as_str(), r.type_label.as_str(), r.required))
            .collect();
        assert_eq!(
            rows,
            vec![
                (0, "items", "ClientListItem[]", true),
                (1, "id", "integer", true),
                (1, "status", "ClientStatus", false),
                (1, "memo", "string", false),
                (0, "total", "integer", false),
            ]
        );
        let id = &t.rows[1];
        assert_eq!(id.constraints, vec!["format: int64", "min: 1"]);
        assert_eq!(id.example.as_deref(), Some("7"));
        assert_eq!(t.rows[2].constraints, vec!["enum: ACTIVE, INACTIVE"]);
        assert_eq!(t.rows[2].ref_name.as_deref(), Some("ClientStatus"));
        assert!(t.rows[3].nullable);
        assert_eq!(t.rows[3].constraints, vec!["nullable", "maxLength: 20"]);
    }

    #[test]
    fn circular_rows_stop_and_are_marked() {
        let s = json!({
            REF_KEY: "Node", "type": "object",
            "properties": {"child": {REF_KEY: "Node", CIRCULAR_KEY: true}}
        });
        let t = schema_table(&s);
        assert_eq!(t.rows.len(), 1);
        assert!(t.rows[0].circular);
        assert!(t.rows[0].constraints.contains(&"循環".to_string()));
    }

    #[test]
    fn mock_prefers_examples_then_generates() {
        let s = json!({
            "type": "object",
            "properties": {
                "email": {"type": "string", "format": "email"},
                "n": {"type": "integer", "example": 5},
                "tags": {"type": "array", "items": {"type": "string"}},
                "state": {"type": "string", "enum": ["A", "B"]},
                "id": {"type": "integer", "readOnly": true}
            }
        });
        let m = mock(&s, true);
        assert!(m.used_schema_example);
        assert_eq!(
            m.value,
            json!({"email": "user@example.com", "n": 5, "tags": ["string"], "state": "A"})
        );
        let plain = mock(
            &json!({"type": "object", "properties": {"a": {"type": "string"}}}),
            true,
        );
        assert!(!plain.used_schema_example);
        assert_eq!(plain.value, json!({"a": "string"}));
    }
}
