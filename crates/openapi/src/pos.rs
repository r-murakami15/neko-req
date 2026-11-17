//! 位置つきの YAML の読み込み。YAML（JSON も読める）を `serde_json::Value` にし、
//! JSON Pointer → 行番号の表を同時に作る。
//!
//! - 行番号は 1 始まり。マッピングの値は「キーの書かれた行」、配列の要素は要素の始まりの行。
//! - 数値などの型の判定は YAML 1.2 のコアスキーマ（saphyr）。キーが文字列でないときは文字列にする。
//! - Value の順序は書かれた順（`serde_json` の `preserve_order`）。

use std::collections::HashMap;

use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};
use serde_json::{Map, Number, Value};

use crate::pointer;

/// 読み込んだ結果。
#[derive(Debug, Clone)]
pub struct Parsed {
    pub value: Value,
    /// JSON Pointer → 行番号（1 始まり）。
    pub lines: HashMap<String, u32>,
}

/// 読めなかったときの理由（行・列は分かるときだけ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub reason: String,
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// テキストを読む。
pub fn parse(text: &str) -> Result<Parsed, YamlError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let docs = match MarkedYaml::load_from_str(text) {
        Ok(d) => d,
        Err(e) => {
            // タブでインデントした JSON など、YAML としては読めない JSON は serde_json で読む（行番号なし）
            if text.trim_start().starts_with(['{', '[']) {
                if let Ok(value) = serde_json::from_str::<Value>(text) {
                    return Ok(Parsed {
                        value,
                        lines: HashMap::new(),
                    });
                }
            }
            let m = e.marker();
            return Err(YamlError {
                line: Some(to_u32(m.line())),
                column: Some(to_u32(m.col() + 1)),
                reason: e.info().to_string(),
            });
        }
    };
    let Some(root) = docs.first() else {
        return Err(YamlError {
            line: None,
            column: None,
            reason: "ファイルが空です".into(),
        });
    };
    let mut lines = HashMap::new();
    let mut ptr = String::new();
    let value = convert(root, &mut ptr, &mut lines)?;
    Ok(Parsed { value, lines })
}

fn key_string(k: &MarkedYaml<'_>) -> Result<String, YamlError> {
    let err = |what: &str| YamlError {
        line: Some(to_u32(k.span.start.line())),
        column: Some(to_u32(k.span.start.col() + 1)),
        reason: format!("マッピングのキーに {what} は使えません"),
    };
    match &k.data {
        YamlData::Value(Scalar::String(s)) => Ok(s.to_string()),
        YamlData::Value(Scalar::Integer(i)) => Ok(i.to_string()),
        YamlData::Value(Scalar::Boolean(b)) => Ok(b.to_string()),
        YamlData::Value(Scalar::FloatingPoint(f)) => Ok(f.0.to_string()),
        YamlData::Value(Scalar::Null) => Ok("null".to_string()),
        YamlData::Representation(s, _, _) => Ok(s.to_string()),
        YamlData::Tagged(_, inner) => key_string(inner),
        _ => Err(err("配列やマッピング")),
    }
}

fn convert(
    node: &MarkedYaml<'_>,
    ptr: &mut String,
    lines: &mut HashMap<String, u32>,
) -> Result<Value, YamlError> {
    lines
        .entry(ptr.clone())
        .or_insert_with(|| to_u32(node.span.start.line()));
    Ok(match &node.data {
        YamlData::Value(s) => match s {
            Scalar::Null => Value::Null,
            Scalar::Boolean(b) => Value::Bool(*b),
            Scalar::Integer(i) => Value::Number((*i).into()),
            Scalar::FloatingPoint(f) => Number::from_f64(f.0)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(f.0.to_string())),
            Scalar::String(s) => Value::String(s.to_string()),
        },
        YamlData::Representation(s, _, _) => Value::String(s.to_string()),
        YamlData::Tagged(_, inner) => convert(inner, ptr, lines)?,
        YamlData::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let len = ptr.len();
                ptr.push('/');
                ptr.push_str(&i.to_string());
                out.push(convert(item, ptr, lines)?);
                ptr.truncate(len);
            }
            Value::Array(out)
        }
        YamlData::Mapping(map) => {
            let mut out = Map::new();
            for (k, v) in map.iter() {
                let key = key_string(k)?;
                let len = ptr.len();
                ptr.push('/');
                ptr.push_str(&pointer::escape(&key));
                // キーの行を、その値のポインターの行にする
                lines.insert(ptr.clone(), to_u32(k.span.start.line()));
                out.insert(key, convert(v, ptr, lines)?);
                // 値の側が書き換えたぶんを、キーの行に戻す
                lines.insert(ptr.clone(), to_u32(k.span.start.line()));
                ptr.truncate(len);
            }
            Value::Object(out)
        }
        YamlData::Alias(_) | YamlData::BadValue => {
            return Err(YamlError {
                line: Some(to_u32(node.span.start.line())),
                column: Some(to_u32(node.span.start.col() + 1)),
                reason: "読めない値（未定義のエイリアスなど）です".into(),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_and_lines() {
        let text = "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1\npaths:\n  /a/b:\n    get:\n      summary: s\n      tags: [x, y]\n";
        let p = parse(text).unwrap();
        assert_eq!(p.value["openapi"], json!("3.0.3"));
        assert_eq!(p.value["info"]["version"], json!(1));
        assert_eq!(p.value["paths"]["/a/b"]["get"]["tags"], json!(["x", "y"]));
        assert_eq!(p.lines["/openapi"], 1);
        assert_eq!(p.lines["/info/title"], 3);
        assert_eq!(p.lines["/paths/~1a~1b/get"], 7);
        assert_eq!(p.lines["/paths/~1a~1b/get/tags/1"], 9);
    }

    #[test]
    fn order_is_kept_and_nested_lines_are_not_overwritten() {
        let p = parse("z: 1\na:\n  c: 2\n  b: 3\n").unwrap();
        let keys: Vec<_> = p.value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["z", "a"]);
        assert_eq!(p.lines["/a"], 2);
        assert_eq!(p.lines["/a/b"], 4);
    }

    #[test]
    fn numeric_status_keys_become_strings() {
        let p = parse("responses:\n  200:\n    description: ok\n  \"404\":\n    description: no\n")
            .unwrap();
        assert!(p.value["responses"].get("200").is_some());
        assert!(p.value["responses"].get("404").is_some());
    }

    #[test]
    fn broken_yaml_reports_line() {
        let e = parse("a: 1\nb: [1, 2\nc: 3\n").unwrap_err();
        assert!(e.line.is_some(), "{e:?}");
        assert!(!e.reason.is_empty());
    }

    #[test]
    fn anchors_and_aliases_are_expanded() {
        let p = parse("base: &b\n  type: string\nuse: *b\n").unwrap();
        assert_eq!(p.value["use"], json!({"type": "string"}));
    }

    #[test]
    fn json_is_accepted() {
        let p = parse("{\"openapi\": \"3.1.0\", \"paths\": {}}").unwrap();
        assert_eq!(p.value["openapi"], json!("3.1.0"));
    }
}
