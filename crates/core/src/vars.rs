//! 変数展開。`{{name}}` を「リクエスト内の上書き › 選んでいる環境 › グローバル」の順で解決する。
//!
//! - 動的変数: `{{$uuid}}`・`{{$timestamp}}`（UNIX 秒）・`{{$isoTimestamp}}`・`{{$randomInt}}`（0〜1000）。
//!   書かれた箇所ごとに新しく作る。上書きや環境の変数では差し替えられない。
//! - 展開は 1 回だけ。変数の値に `{{x}}` が入っていても、それをさらに展開はしない。
//! - 展開できない変数は `{{name}}` のまま残し、名前を返す。
//! - 値は書かれたまま差し込む（JSON の文字列に入れるときのエスケープはしない）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::redact::{redact_text, MASK};
use crate::request::ParamIn;

/// 層に置く変数 1 件。`Debug` ではシークレットの値を `••••` にする。
#[derive(Clone, PartialEq, Eq)]
pub struct ScopedValue {
    pub value: String,
    /// シークレット（キーチェーンから読んだ値）。履歴や画面のプレビューでは伏せ字にする。
    pub secret: bool,
}

impl std::fmt::Debug for ScopedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = if self.secret { MASK } else { &self.value };
        f.debug_struct("ScopedValue")
            .field("value", &value)
            .field("secret", &self.secret)
            .finish()
    }
}

/// 同じ優先度の変数の集まり（上書き 1 組、環境 1 つ、グローバル 1 つ）。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct VarLayer {
    entries: HashMap<String, ScopedValue>,
}

impl VarLayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 変数を足す（同じ名前があれば置き換える）。
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>, secret: bool) {
        self.entries.insert(
            name.into(),
            ScopedValue {
                value: value.into(),
                secret,
            },
        );
    }

    pub fn get(&self, name: &str) -> Option<&ScopedValue> {
        self.entries.get(name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl std::fmt::Debug for VarLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 並びを安定させる（HashMap の順番に依らない）
        let mut names: Vec<&String> = self.entries.keys().collect();
        names.sort();
        let mut m = f.debug_map();
        for name in names {
            m.entry(name, &self.entries[name]);
        }
        m.finish()
    }
}

/// 優先度の高い層から順に並べた変数の集まり。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct VarScope {
    layers: Vec<VarLayer>,
}

impl VarScope {
    pub fn new() -> Self {
        Self::default()
    }

    /// 今ある層より低い優先度の層を足す。最初に足したものがいちばん優先される。
    pub fn push_layer(&mut self, layer: VarLayer) {
        self.layers.push(layer);
    }

    /// 「上書き › 環境 › グローバル」の順に組み立てる。
    pub fn from_layers(overrides: VarLayer, environment: VarLayer, global: VarLayer) -> Self {
        Self {
            layers: vec![overrides, environment, global],
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&ScopedValue> {
        self.layers.iter().find_map(|layer| layer.get(name))
    }
}

impl std::fmt::Debug for VarScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VarScope")
            .field("layers", &self.layers)
            .finish()
    }
}

/// 動的変数の値の出どころ。テストでは固定値に差し替える。
pub trait DynamicSource {
    /// `name` は `$` から始まる名前（例: `$uuid`）。知らない名前は `None`。
    fn eval(&self, name: &str) -> Option<String>;
}

/// 本番用。UUID v4・現在時刻・乱数。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDynamics;

impl DynamicSource for SystemDynamics {
    fn eval(&self, name: &str) -> Option<String> {
        match name {
            "$uuid" => Some(uuid::Uuid::new_v4().to_string()),
            "$timestamp" => Some(chrono::Utc::now().timestamp().to_string()),
            "$isoTimestamp" => {
                Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            }
            "$randomInt" => {
                // 0〜1000（両端を含む）。偏りは 1001 に対して 2^128 なので無視できる
                let n = uuid::Uuid::new_v4().as_u128() % 1001;
                Some(n.to_string())
            }
            _ => None,
        }
    }
}

/// 変数の展開結果。`Debug` ではシークレットの値を伏せる。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Expanded {
    pub text: String,
    /// 展開できなかった変数の名前（出てきた順、重複なし）。
    pub unresolved: Vec<String>,
    /// 差し込んだシークレットの値（出てきた順、重複なし）。
    pub secrets: Vec<String>,
}

impl std::fmt::Debug for Expanded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Expanded")
            .field("text", &redact_text(&self.text, &self.secrets))
            .field("unresolved", &self.unresolved)
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// `template` の `{{name}}` を展開する。
pub fn expand(template: &str, scope: &VarScope, dynamics: &dyn DynamicSource) -> Expanded {
    let mut out = Expanded::default();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.text.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // 閉じていない `{{` はそのまま
            out.text.push_str(&rest[start..]);
            return out;
        };
        let raw_name = &after[..end];
        let name = raw_name.trim();
        if !is_variable_name(name) {
            // `{{` を文字として残し、1 文字ぶん進めて探し直す（`{{{x}}` のような並びのため）
            out.text.push('{');
            rest = &rest[start + 1..];
            continue;
        }
        match resolve(name, scope, dynamics) {
            Some(found) => {
                if found.secret && !out.secrets.contains(&found.value) {
                    out.secrets.push(found.value.clone());
                }
                out.text.push_str(&found.value);
            }
            None => {
                if !out.unresolved.iter().any(|n| n == name) {
                    out.unresolved.push(name.to_string());
                }
                out.text.push_str("{{");
                out.text.push_str(raw_name);
                out.text.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.text.push_str(rest);
    out
}

/// 変数名として読めるか（空・改行・波括弧を含むものは変数ではない）。
pub fn is_variable_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['{', '}', '\n', '\r'])
}

/// `template` に出てくる変数名（`{{ }}` の中身。出てきた順、重複なし）。
pub fn variable_names(template: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let name = after[..end].trim();
        if is_variable_name(name) {
            if !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            rest = &after[end + 2..];
        } else {
            rest = &rest[start + 1..];
        }
    }
    names
}

fn resolve(name: &str, scope: &VarScope, dynamics: &dyn DynamicSource) -> Option<ScopedValue> {
    if name.starts_with('$') {
        return dynamics.eval(name).map(|value| ScopedValue {
            value,
            secret: false,
        });
    }
    scope.lookup(name).cloned()
}

/// 行の中の「名前」と「値」のどちらか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RowPart {
    Name,
    Value,
}

/// 展開できなかった変数が書かれていた場所。行番号は 1 始まりで、画面に並ぶ表の行
/// （無効の行も数える）に合わせる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum VarLocation {
    /// URL 欄。
    Url,
    /// パラメータの表（クエリ・パス・ヘッダー・Cookie）。`row` はその種類の中での行番号。
    Param {
        location: ParamIn,
        row: u32,
        part: RowPart,
    },
    /// ボディ（JSON・テキスト・バイナリのファイルパス）。
    Body,
    /// フォーム（form-urlencoded）の行。
    BodyForm { row: u32, part: RowPart },
    /// multipart の行。
    BodyMultipart { row: u32, part: RowPart },
    /// 認証。`field` は `token` / `username` / `password` / `name` / `value`。
    Auth { field: String },
}

impl VarLocation {
    /// 画面や送信エラーに出す日本語の場所の名前。
    pub fn describe(&self) -> String {
        fn part(p: &RowPart) -> &'static str {
            match p {
                RowPart::Name => "キー",
                RowPart::Value => "値",
            }
        }
        match self {
            VarLocation::Url => "URL".to_string(),
            VarLocation::Param {
                location,
                row,
                part: p,
            } => {
                let table = match location {
                    ParamIn::Query => "クエリパラメータ",
                    ParamIn::Path => "パスパラメータ",
                    ParamIn::Header => "ヘッダー",
                    ParamIn::Cookie => "Cookie",
                };
                format!("{table} {row} 行目の{}", part(p))
            }
            VarLocation::Body => "ボディ".to_string(),
            VarLocation::BodyForm { row, part: p } => {
                format!("ボディ（フォーム）{row} 行目の{}", part(p))
            }
            VarLocation::BodyMultipart { row, part: p } => {
                format!("ボディ（multipart）{row} 行目の{}", part(p))
            }
            VarLocation::Auth { field } => format!("認証（{field}）"),
        }
    }
}

/// 展開できなかった変数 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedVar {
    pub name: String,
    pub location: VarLocation,
}

impl UnresolvedVar {
    /// 例: `{{baseUrl}}（URL）`。
    pub fn describe(&self) -> String {
        format!("{{{{{}}}}}（{}）", self.name, self.location.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定値の動的変数。
    struct Fixed;
    impl DynamicSource for Fixed {
        fn eval(&self, name: &str) -> Option<String> {
            match name {
                "$uuid" => Some("00000000-0000-4000-8000-000000000000".into()),
                "$timestamp" => Some("1700000000".into()),
                _ => None,
            }
        }
    }

    fn layer(pairs: &[(&str, &str, bool)]) -> VarLayer {
        let mut l = VarLayer::new();
        for (n, v, s) in pairs {
            l.insert(*n, *v, *s);
        }
        l
    }

    fn scope() -> VarScope {
        VarScope::from_layers(
            layer(&[("who", "override", false)]),
            layer(&[("who", "env", false), ("baseUrl", "http://env", false)]),
            layer(&[
                ("who", "global", false),
                ("baseUrl", "http://global", false),
                ("onlyGlobal", "g", false),
                ("password", "s3cret", true),
            ]),
        )
    }

    #[test]
    fn precedence_is_override_then_environment_then_global() {
        let s = scope();
        assert_eq!(expand("{{who}}", &s, &Fixed).text, "override");
        assert_eq!(expand("{{baseUrl}}", &s, &Fixed).text, "http://env");
        assert_eq!(expand("{{onlyGlobal}}", &s, &Fixed).text, "g");
    }

    #[test]
    fn expands_many_variables_and_trims_inner_spaces() {
        let s = scope();
        let e = expand("a {{ who }} b {{baseUrl}}/x {{who}}", &s, &Fixed);
        assert_eq!(e.text, "a override b http://env/x override");
        assert!(e.unresolved.is_empty());
    }

    #[test]
    fn dynamic_variables_come_from_the_source() {
        let s = VarScope::new();
        let e = expand("{{$uuid}}/{{$timestamp}}", &s, &Fixed);
        assert_eq!(e.text, "00000000-0000-4000-8000-000000000000/1700000000");
    }

    #[test]
    fn system_dynamics_have_the_expected_shapes() {
        let d = SystemDynamics;
        let uuid = d.eval("$uuid").unwrap();
        assert!(uuid::Uuid::parse_str(&uuid).is_ok());
        let ts: i64 = d.eval("$timestamp").unwrap().parse().unwrap();
        assert!(ts > 1_700_000_000);
        let iso = d.eval("$isoTimestamp").unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&iso).is_ok(), "{iso}");
        for _ in 0..200 {
            let n: u32 = d.eval("$randomInt").unwrap().parse().unwrap();
            assert!(n <= 1000);
        }
        assert_eq!(d.eval("$nope"), None);
        // 同じ変数でも毎回別の値になる
        assert_ne!(d.eval("$uuid"), d.eval("$uuid"));
    }

    #[test]
    fn dynamic_variables_cannot_be_overridden() {
        let mut s = VarScope::new();
        s.push_layer(layer(&[("$uuid", "fixed", false)]));
        let e = expand("{{$uuid}}", &s, &Fixed);
        assert_eq!(e.text, "00000000-0000-4000-8000-000000000000");
    }

    #[test]
    fn unresolved_are_reported_once_and_left_as_written() {
        let s = scope();
        let e = expand("{{nope}} {{ nope }} {{$unknown}} {{who}}", &s, &Fixed);
        assert_eq!(e.text, "{{nope}} {{ nope }} {{$unknown}} override");
        assert_eq!(e.unresolved, ["nope", "$unknown"]);
    }

    #[test]
    fn secrets_are_collected_but_values_are_expanded() {
        let s = scope();
        let e = expand("pw={{password}}&pw2={{password}}", &s, &Fixed);
        assert_eq!(e.text, "pw=s3cret&pw2=s3cret");
        assert_eq!(e.secrets, ["s3cret"]);
    }

    #[test]
    fn values_are_not_expanded_again() {
        let mut env = VarLayer::new();
        env.insert("a", "{{b}}", false);
        env.insert("b", "B", false);
        let mut s = VarScope::new();
        s.push_layer(env);
        assert_eq!(expand("{{a}}", &s, &Fixed).text, "{{b}}");
    }

    #[test]
    fn odd_braces_are_kept_as_text() {
        let s = scope();
        assert_eq!(expand("{{", &s, &Fixed).text, "{{");
        assert_eq!(expand("a {{ b", &s, &Fixed).text, "a {{ b");
        assert_eq!(expand("{{}} {{  }}", &s, &Fixed).text, "{{}} {{  }}");
        assert_eq!(expand("{{{who}}}", &s, &Fixed).text, "{override}");
        assert_eq!(expand("{id}", &s, &Fixed).text, "{id}");
        assert_eq!(
            expand("日本語{{who}}です", &s, &Fixed).text,
            "日本語overrideです"
        );
    }

    #[test]
    fn variable_names_lists_in_order_without_duplicates() {
        assert_eq!(
            variable_names("{{a}}/{{ b }}/{{a}}/{{$uuid}}/{x}"),
            ["a", "b", "$uuid"]
        );
    }

    #[test]
    fn location_descriptions_are_in_japanese() {
        let v = UnresolvedVar {
            name: "q".into(),
            location: VarLocation::Param {
                location: ParamIn::Query,
                row: 2,
                part: RowPart::Value,
            },
        };
        assert_eq!(v.describe(), "{{q}}（クエリパラメータ 2 行目の値）");
        assert_eq!(VarLocation::Url.describe(), "URL");
        assert_eq!(
            VarLocation::Auth {
                field: "token".into()
            }
            .describe(),
            "認証（token）"
        );
    }

    #[test]
    fn location_wire_shape() {
        let v = serde_json::to_value(VarLocation::Param {
            location: ParamIn::Header,
            row: 1,
            part: RowPart::Name,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "param", "location": "header", "row": 1, "part": "name"})
        );
    }
}
