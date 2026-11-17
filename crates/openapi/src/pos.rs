//! 位置つきの YAML の読み込み。YAML（JSON も読める）を `serde_json::Value` にし、
//! JSON Pointer → 行番号の表を同時に作る。
//!
//! - 行番号は 1 始まり。マッピングの値は「キーの書かれた行」、配列の要素は要素の始まりの行。
//! - 数値などの型の判定は YAML 1.2 のコアスキーマ（saphyr）。キーが文字列でないときは文字列にする。
//! - Value の順序は書かれた順（`serde_json` の `preserve_order`）。
//! - `info.version` だけは、数値に見える書き方（`1.10`・`001`）でも書かれた文字列のまま持つ
//!   （OpenAPI では string なので、`1.10` が `1.1` にならないように）。それ以外の値は YAML の仕様どおり。
//! - merge key（`<<: *base`・`<<: [*a, *b]`）は展開する。同じキーは、そのマッピングに直接書いたものが勝ち、
//!   `<<` のリストでは前のものが勝つ。
//! - パースの前に入れ子の深さを見積もり（`MAX_NESTING`）、超えたら読まずに止める。パースと変換は大きなスタック
//!   （`BIG_STACK`）のスレッドで行う。

use std::collections::HashMap;
use std::sync::Mutex;

use saphyr::{AnnotatedMapping, LoadableYamlNode, MarkedYaml, Scalar, YamlData};
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

/// ブロック形式の入れ子の深さの上限（見積もり）。
pub const MAX_NESTING: usize = 200;

/// パース・変換を走らせるスレッドのスタック（再帰で落ちないように）。
const BIG_STACK: usize = 64 * 1024 * 1024;

/// `f` を大きなスタックのスレッドで走らせる（スレッドを作れなければその場で走らせる）。
pub fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    let cell = Mutex::new(Some(f));
    let run = || -> Option<T> { cell.lock().ok()?.take().map(|f| f()) };
    let spawned = std::thread::scope(|s| {
        match std::thread::Builder::new()
            .stack_size(BIG_STACK)
            .spawn_scoped(s, run)
        {
            Ok(h) => Some(h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))),
            Err(_) => None,
        }
    });
    match spawned {
        Some(Some(v)) => v,
        _ => {
            // スレッドを作れなかった（まだ取り出していない）
            let f = cell
                .lock()
                .ok()
                .and_then(|mut c| c.take())
                .expect("closure is still there when the thread could not be spawned");
            f()
        }
    }
}

/// ブロック形式の入れ子の深さを、字下げと `- ` から見積もる。深すぎたらエラー（パースしない）。
/// JSON・フロー形式（先頭が `{` `[`）は、パーサー自身の再帰の上限に任せる。
fn check_nesting(text: &str) -> Result<(), YamlError> {
    if text.trim_start().starts_with(['{', '[']) {
        return Ok(());
    }
    let too_deep = |line: usize| YamlError {
        line: Some(to_u32(line + 1)),
        column: None,
        reason: format!("入れ子が深すぎます（{MAX_NESTING} 段まで）"),
    };
    let mut stack: Vec<usize> = Vec::new();
    // `|` `>` のブロックスカラーの中（この字下げより深い行）は構造ではない
    let mut scalar_parent: Option<usize> = None;
    for (n, raw) in text.split(['\n', '\r']).enumerate() {
        let indent = raw.bytes().take_while(|b| *b == b' ').count();
        let rest = raw[indent..].trim_end();
        if rest.is_empty() || rest.starts_with('#') {
            continue;
        }
        if let Some(parent) = scalar_parent {
            if indent > parent {
                continue;
            }
            scalar_parent = None;
        }
        if indent == 0 && (rest == "---" || rest.starts_with("--- ") || rest == "...") {
            stack.clear();
            continue;
        }
        let mut col = indent;
        let mut last_col = indent;
        let mut s = rest;
        // `- - - x` のような入れ子の配列
        while s == "-" || s.starts_with("- ") {
            while stack.last().is_some_and(|c| *c >= col) {
                stack.pop();
            }
            stack.push(col);
            last_col = col;
            let after = &s[1..];
            let after_trim = after.trim_start();
            col += 1 + (after.len() - after_trim.len());
            s = after_trim;
        }
        if !s.is_empty() {
            while stack.last().is_some_and(|c| *c >= col) {
                stack.pop();
            }
            stack.push(col);
            last_col = col;
        }
        if stack.len() > MAX_NESTING {
            return Err(too_deep(n));
        }
        if opens_block_scalar(s) {
            scalar_parent = Some(last_col);
        }
    }
    Ok(())
}

/// 行の終わりが `|` `>`（`|-` `>+` `|2` などを含む）のブロックスカラーの頭か。
fn opens_block_scalar(line: &str) -> bool {
    let code = match line.find(" #") {
        Some(i) => &line[..i],
        None => line,
    };
    let Some(last) = code.split_whitespace().last() else {
        return false;
    };
    let mut chars = last.chars();
    matches!(chars.next(), Some('|' | '>')) && chars.all(|c| matches!(c, '-' | '+' | '0'..='9'))
}

/// テキストを読む。
pub fn parse(text: &str) -> Result<Parsed, YamlError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    check_nesting(text)?;
    on_big_stack(|| parse_text(text))
}

fn parse_text(text: &str) -> Result<Parsed, YamlError> {
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
    let value = convert(root, text, &mut ptr, &mut lines)?;
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

/// ソースの `[start, end)`（文字の位置）の文字列。
fn source_slice(src: &str, start: usize, end: usize) -> String {
    src.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect::<String>()
        .trim()
        .to_string()
}

/// マッピングの 1 つ分の項目を `out` に入れる。`merging` のときは merge key から来た項目で、
/// すでにあるキーは上書きしない（直接書いたもの・前の merge が勝つ）。
fn fill_mapping(
    map: &AnnotatedMapping<'_, MarkedYaml<'_>>,
    src: &str,
    ptr: &mut String,
    lines: &mut HashMap<String, u32>,
    out: &mut Map<String, Value>,
    merge_line: Option<u32>,
) -> Result<(), YamlError> {
    for (k, v) in map.iter() {
        if is_merge_key(k) {
            if let Some(sources) = merge_sources(v) {
                let line = merge_line.unwrap_or_else(|| to_u32(k.span.start.line()));
                for m in sources {
                    fill_mapping(m, src, ptr, lines, out, Some(line))?;
                }
                continue;
            }
        }
        let key = key_string(k)?;
        if merge_line.is_some() && out.contains_key(&key) {
            continue;
        }
        let len = ptr.len();
        ptr.push('/');
        ptr.push_str(&pointer::escape(&key));
        // キーの行を、その値のポインターの行にする（merge で来たものは `<<` の行）
        let key_line = merge_line.unwrap_or_else(|| to_u32(k.span.start.line()));
        lines.insert(ptr.clone(), key_line);
        out.insert(key, convert(v, src, ptr, lines)?);
        // 値の側が書き換えたぶんを、キーの行に戻す
        lines.insert(ptr.clone(), key_line);
        ptr.truncate(len);
    }
    Ok(())
}

fn is_merge_key(k: &MarkedYaml<'_>) -> bool {
    matches!(&k.data, YamlData::Value(Scalar::String(s)) if s == "<<")
}

/// `<<` の値が、マッピング 1 つか、マッピングだけのリストなら、そのマッピング（前のものが優先）。
fn merge_sources<'a, 'b>(
    v: &'b MarkedYaml<'a>,
) -> Option<Vec<&'b AnnotatedMapping<'a, MarkedYaml<'a>>>> {
    match &v.data {
        YamlData::Mapping(m) => Some(vec![m]),
        YamlData::Sequence(items) => items
            .iter()
            .map(|i| match &i.data {
                YamlData::Mapping(m) => Some(m),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn convert(
    node: &MarkedYaml<'_>,
    src: &str,
    ptr: &mut String,
    lines: &mut HashMap<String, u32>,
) -> Result<Value, YamlError> {
    lines
        .entry(ptr.clone())
        .or_insert_with(|| to_u32(node.span.start.line()));
    Ok(match &node.data {
        YamlData::Value(s) => match s {
            Scalar::Null => Value::Null,
            // info.version は書かれた文字列のまま（`1.10` を 1.1 にしない）
            _ if ptr == "/info/version" && !matches!(s, Scalar::String(_)) => {
                let raw = source_slice(src, node.span.start.index(), node.span.end.index());
                if raw.is_empty() {
                    Value::Null
                } else {
                    Value::String(raw)
                }
            }
            Scalar::Boolean(b) => Value::Bool(*b),
            Scalar::Integer(i) => Value::Number((*i).into()),
            Scalar::FloatingPoint(f) => Number::from_f64(f.0)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(f.0.to_string())),
            Scalar::String(s) => Value::String(s.to_string()),
        },
        YamlData::Representation(s, _, _) => Value::String(s.to_string()),
        YamlData::Tagged(_, inner) => convert(inner, src, ptr, lines)?,
        YamlData::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let len = ptr.len();
                ptr.push('/');
                ptr.push_str(&i.to_string());
                out.push(convert(item, src, ptr, lines)?);
                ptr.truncate(len);
            }
            Value::Array(out)
        }
        YamlData::Mapping(map) => {
            let mut out = Map::new();
            fill_mapping(map, src, ptr, lines, &mut out, None)?;
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
        // info.version は書かれた文字列のまま
        assert_eq!(p.value["info"]["version"], json!("1"));
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
    fn deeply_nested_blocks_are_refused_before_parsing() {
        let mut y = String::from("a:\n");
        for i in 0..300 {
            y.push_str(&" ".repeat(i + 1));
            y.push_str("k:\n");
        }
        let e = parse(&y).unwrap_err();
        assert!(e.reason.contains("入れ子"), "{e:?}");
        assert!(e.line.is_some());
        // 入れ子の配列（`- - - x`）も
        let e = parse(&format!("{}x\n", "- ".repeat(300))).unwrap_err();
        assert!(e.reason.contains("入れ子"), "{e:?}");
        // 普通の深さ・ブロックスカラーの中の字下げは数えない
        let mut text = String::from("d: |\n");
        for i in 0..300 {
            text.push_str(&" ".repeat(i + 2));
            text.push_str("x\n");
        }
        assert!(parse(&text).is_ok());
        assert!(parse("a:\n  b:\n    - c:\n        d: 1\n").is_ok());
    }

    #[test]
    fn merge_keys_are_expanded_with_yaml_precedence() {
        let y = "base: &b\n  x: 1\n  y: 1\nother: &o\n  y: 2\n  z: 2\nm:\n  <<: [*b, *o]\n  x: 9\nn:\n  w: 0\n  <<: *b\n";
        let p = parse(y).unwrap();
        assert_eq!(p.value["m"], json!({"x": 9, "y": 1, "z": 2}));
        assert_eq!(p.value["n"], json!({"w": 0, "x": 1, "y": 1}));
        // `<<` が配列やマッピングでないときは、ふつうのキー
        let p = parse("m:\n  <<: 5\n").unwrap();
        assert_eq!(p.value["m"], json!({"<<": 5}));
    }

    #[test]
    fn info_version_is_a_string_but_other_numbers_are_not() {
        let p = parse("info:\n  version: 1.10\nx: 0.10\ny: 001\n").unwrap();
        assert_eq!(p.value["info"]["version"], json!("1.10"));
        assert_eq!(p.value["x"], json!(0.1));
        assert_eq!(p.value["y"], json!(1));
    }

    #[test]
    fn json_is_accepted() {
        let p = parse("{\"openapi\": \"3.1.0\", \"paths\": {}}").unwrap();
        assert_eq!(p.value["openapi"], json!("3.1.0"));
    }
}
