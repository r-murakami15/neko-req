//! 変数展開（`expand`）を公開 API だけで確かめる結合テスト。
//! 既存の単体テスト（crates/core/src/vars.rs）と重ならない観点を中心にする。

use nekoreq_core::{
    expand, DynamicSource, ParamIn, RowPart, SystemDynamics, UnresolvedVar, VarLayer, VarLocation,
    VarScope,
};

struct NoDynamics;
impl DynamicSource for NoDynamics {
    fn eval(&self, _name: &str) -> Option<String> {
        None
    }
}

fn layer(pairs: &[(&str, &str, bool)]) -> VarLayer {
    let mut l = VarLayer::new();
    for (n, v, s) in pairs {
        l.insert(*n, *v, *s);
    }
    l
}

fn scope1(pairs: &[(&str, &str, bool)]) -> VarScope {
    let mut s = VarScope::new();
    s.push_layer(layer(pairs));
    s
}

// ---- 優先順位 ----

#[test]
fn an_empty_value_in_a_higher_layer_still_wins_over_a_lower_one() {
    // 空文字の上書きは「値がある」扱い（次の層へは落ちない）。無効・値なしの扱いは app 側の責務
    let s = VarScope::from_layers(
        layer(&[("x", "", false)]),
        layer(&[("x", "env", false)]),
        layer(&[("x", "global", false)]),
    );
    let e = expand("[{{x}}]", &s, &NoDynamics);
    assert_eq!(e.text, "[]");
    assert!(e.unresolved.is_empty());
}

#[test]
fn name_lookup_is_case_sensitive_and_exact() {
    let s = scope1(&[("Token", "T", false)]);
    let e = expand("{{token}}{{Token}}{{TOKEN}}", &s, &NoDynamics);
    assert_eq!(e.text, "{{token}}T{{TOKEN}}");
    assert_eq!(e.unresolved, ["token", "TOKEN"]);
}

#[test]
fn a_secret_in_a_lower_layer_is_used_when_a_higher_layer_lacks_the_name() {
    let s = VarScope::from_layers(
        layer(&[]),
        layer(&[("other", "o", false)]),
        layer(&[("pw", "g-secret", true)]),
    );
    let e = expand("{{pw}}", &s, &NoDynamics);
    assert_eq!(e.text, "g-secret");
    assert_eq!(e.secrets, ["g-secret"]);
}

#[test]
fn an_override_shadowing_a_secret_is_not_reported_as_a_secret() {
    // 上書きは secret=false。上書きされた値は伏せ字の対象にならない（上書きの値は画面で書いたものだから）
    let s = VarScope::from_layers(
        layer(&[("pw", "typed-by-hand", false)]),
        layer(&[("pw", "from-keychain", true)]),
        layer(&[]),
    );
    let e = expand("{{pw}}", &s, &NoDynamics);
    assert_eq!(e.text, "typed-by-hand");
    assert!(e.secrets.is_empty());
}

// ---- 動的変数 ----

#[test]
fn dynamic_variable_shapes() {
    let d = SystemDynamics;

    // $uuid は UUID v4（8-4-4-4-12、バージョン 4、バリアント 8/9/a/b）
    let u = d.eval("$uuid").unwrap();
    let parsed = uuid::Uuid::parse_str(&u).unwrap();
    assert_eq!(parsed.get_version_num(), 4, "{u}");
    assert_eq!(u.len(), 36);
    assert_eq!(u, u.to_lowercase());

    // $timestamp は秒（10 桁）で、現在時刻に近い
    let ts = d.eval("$timestamp").unwrap();
    assert!(ts.chars().all(|c| c.is_ascii_digit()), "{ts}");
    assert_eq!(ts.len(), 10, "秒であってミリ秒ではない: {ts}");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let ts: i64 = ts.parse().unwrap();
    assert!((now - ts).abs() <= 2, "now={now} ts={ts}");

    // $isoTimestamp は RFC 3339（UTC・Z）
    let iso = d.eval("$isoTimestamp").unwrap();
    let dt = chrono::DateTime::parse_from_rfc3339(&iso).unwrap();
    assert!(iso.ends_with('Z'), "{iso}");
    assert_eq!(dt.offset().local_minus_utc(), 0);
    let diff = (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_seconds();
    assert!(diff.abs() <= 2, "{iso}");

    // $randomInt は 0〜1000（両端を含む）の整数。先頭ゼロや符号がない
    let mut seen_min = u32::MAX;
    let mut seen_max = 0u32;
    for _ in 0..3000 {
        let r = d.eval("$randomInt").unwrap();
        assert!(!r.starts_with('+') && !r.starts_with('-'), "{r}");
        assert!(r == "0" || !r.starts_with('0'), "{r}");
        let n: u32 = r.parse().unwrap();
        assert!(n <= 1000, "{n}");
        seen_min = seen_min.min(n);
        seen_max = seen_max.max(n);
    }
    // 偏りがひどくないこと（3000 回で全域の端に近い値が出る）
    assert!(seen_min < 100 && seen_max > 900, "{seen_min}..{seen_max}");
}

#[test]
fn dynamic_variables_are_new_for_each_occurrence_in_a_template() {
    let e = expand("{{$uuid}}/{{$uuid}}", &VarScope::new(), &SystemDynamics);
    let (a, b) = e.text.split_once('/').unwrap();
    assert_ne!(a, b, "書かれた箇所ごとに新しい値");
    assert!(e.unresolved.is_empty());
    assert!(e.secrets.is_empty(), "動的変数はシークレットではない");
}

#[test]
fn dynamic_names_are_case_sensitive_and_unknown_ones_are_unresolved() {
    let e = expand(
        "{{$UUID}}{{$Timestamp}}{{$random}}",
        &VarScope::new(),
        &SystemDynamics,
    );
    assert_eq!(e.text, "{{$UUID}}{{$Timestamp}}{{$random}}");
    assert_eq!(e.unresolved, ["$UUID", "$Timestamp", "$random"]);
}

#[test]
fn spaces_around_a_dynamic_name_are_trimmed() {
    let e = expand("{{ $timestamp }}", &VarScope::new(), &SystemDynamics);
    assert!(e.unresolved.is_empty());
    assert_eq!(e.text.len(), 10);
}

#[test]
fn a_variable_named_like_a_dynamic_one_in_a_layer_is_ignored() {
    // `$` から始まる名前は層を見ない
    let s = scope1(&[("$timestamp", "FIXED", false)]);
    let e = expand("{{$timestamp}}", &s, &SystemDynamics);
    assert_ne!(e.text, "FIXED");
}

// ---- 書式の癖 ----

#[test]
fn spaces_tabs_and_full_width_spaces_inside_braces() {
    let s = scope1(&[("name", "N", false)]);
    assert_eq!(expand("{{ name }}", &s, &NoDynamics).text, "N");
    assert_eq!(expand("{{\tname\t}}", &s, &NoDynamics).text, "N");
    assert_eq!(expand("{{  name}}", &s, &NoDynamics).text, "N");
    // 全角空白も trim される（str::trim は Unicode の空白を落とす）
    assert_eq!(
        expand("{{\u{3000}name\u{3000}}}", &s, &NoDynamics).text,
        "N"
    );
}

#[test]
fn unresolved_keeps_the_original_spacing_in_the_text() {
    let e = expand("a{{  nope  }}b", &VarScope::new(), &NoDynamics);
    assert_eq!(e.text, "a{{  nope  }}b");
    assert_eq!(e.unresolved, ["nope"], "名前は trim 済みで報告する");
}

#[test]
fn newline_inside_braces_is_not_a_variable() {
    let s = scope1(&[("a", "A", false), ("a\nb", "X", false)]);
    // 名前の途中の改行は変数ではない（両端の空白・改行は trim される）
    let e = expand("{{a\nb}}", &s, &NoDynamics);
    assert_eq!(e.text, "{{a\nb}}");
    assert!(e.unresolved.is_empty());
    assert_eq!(expand("{{a\n}}", &s, &NoDynamics).text, "A");
}

#[test]
fn unterminated_braces_keep_the_rest_verbatim_even_with_variables_before() {
    let s = scope1(&[("a", "A", false)]);
    let e = expand("{{a}} then {{b", &s, &NoDynamics);
    assert_eq!(e.text, "A then {{b");
    assert!(
        e.unresolved.is_empty(),
        "閉じていないものは変数として報告しない"
    );
    // 閉じていない `{{` の後ろに `}` が 1 つだけ
    assert_eq!(expand("{{a}", &s, &NoDynamics).text, "{{a}");
    // 閉じが先にある
    assert_eq!(expand("}}{{a}}", &s, &NoDynamics).text, "}}A");
}

#[test]
fn json_like_text_with_braces_is_left_alone() {
    let s = scope1(&[("v", "1", false)]);
    let e = expand(r#"{"a":{"b":"{{v}}"},"c":[{}]}"#, &s, &NoDynamics);
    assert_eq!(e.text, r#"{"a":{"b":"1"},"c":[{}]}"#);
    // JSON の閉じ括弧が `}}` になる並びは変数ではない
    let e = expand(r#"{"a":{"b":1}}"#, &s, &NoDynamics);
    assert_eq!(e.text, r#"{"a":{"b":1}}"#);
    assert!(e.unresolved.is_empty());
}

#[test]
fn a_json_object_opening_with_double_braces_before_a_variable() {
    // `{{"a":1}}` のような JSON っぽい並びは変数名にならない（`"a":1` は名前として読めるが未解決として報告される）
    let e = expand(r#"{{"a":1}}"#, &VarScope::new(), &NoDynamics);
    assert_eq!(e.text, r#"{{"a":1}}"#);
}

#[test]
fn value_with_braces_is_not_reexpanded_even_when_it_names_a_real_variable() {
    let s = VarScope::from_layers(
        layer(&[("a", "{{b}}", false)]),
        layer(&[("b", "BBB", false), ("c", "{{{{b}}}}", false)]),
        layer(&[]),
    );
    assert_eq!(expand("{{a}}", &s, &NoDynamics).text, "{{b}}");
    assert_eq!(expand("{{c}}", &s, &NoDynamics).text, "{{{{b}}}}");
    // 展開後の文字列と前後の文字がつながって新しい `{{x}}` ができても、それは展開しない
    let s = scope1(&[("l", "{", false), ("r", "}", false), ("x", "X", false)]);
    assert_eq!(
        expand("{{l}}{{l}}x{{r}}{{r}}", &s, &NoDynamics).text,
        "{{x}}"
    );
}

#[test]
fn a_secret_value_that_contains_braces_is_inserted_as_is() {
    let s = scope1(&[("pw", "p{{w}}d", true)]);
    let e = expand("[{{pw}}]", &s, &NoDynamics);
    assert_eq!(e.text, "[p{{w}}d]");
    assert_eq!(e.secrets, ["p{{w}}d"]);
    assert!(e.unresolved.is_empty());
}

#[test]
fn many_variables_and_large_input_finish_quickly() {
    let s = scope1(&[("v", "x", false)]);
    let template = "{{v}}".repeat(50_000);
    let start = std::time::Instant::now();
    let e = expand(&template, &s, &NoDynamics);
    assert_eq!(e.text.len(), 50_000);
    // 閉じていない `{{` が大量にあっても二乗にならない
    let bad = "{{".repeat(50_000);
    let e2 = expand(&bad, &s, &NoDynamics);
    assert_eq!(e2.text, bad);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn many_stray_opening_braces_with_a_late_closing_do_not_blow_up() {
    // `{{{{{{...x}}`: 開きが多くて閉じが最後にあるとき、探し直しが二乗になりうる
    let s = scope1(&[("x", "X", false)]);
    let template = format!("{}x}}}}", "{".repeat(20_000));
    let start = std::time::Instant::now();
    let e = expand(&template, &s, &NoDynamics);
    assert!(e.text.ends_with("X"), "{}", &e.text[e.text.len() - 5..]);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn multibyte_text_around_and_inside_braces_is_safe() {
    let s = scope1(&[("名前", "ネコ", false), ("emoji", "🐱", false)]);
    let e = expand(
        "こんにちは{{名前}}さん{{ emoji }}🐱{{未定義}}",
        &s,
        &NoDynamics,
    );
    assert_eq!(e.text, "こんにちはネコさん🐱🐱{{未定義}}");
    assert_eq!(e.unresolved, ["未定義"]);
    // 開き括弧の直前がマルチバイトでも、探し直し（1 バイト進める）で panic しない
    let e = expand("あ{{{{い}}", &s, &NoDynamics);
    assert_eq!(e.text, "あ{{{{い}}");
}

#[test]
fn secrets_are_listed_in_order_without_duplicates() {
    let s = scope1(&[("a", "AA", true), ("b", "BB", true), ("c", "CC", false)]);
    let e = expand("{{b}}{{a}}{{c}}{{b}}{{a}}", &s, &NoDynamics);
    assert_eq!(e.secrets, ["BB", "AA"]);
}

#[test]
fn two_secret_variables_with_the_same_value_are_listed_once() {
    let s = scope1(&[("a", "same", true), ("b", "same", true)]);
    let e = expand("{{a}}{{b}}", &s, &NoDynamics);
    assert_eq!(e.secrets, ["same"]);
}

// ---- 場所 ----

#[test]
fn location_wire_shapes_for_every_variant() {
    let cases: Vec<(VarLocation, serde_json::Value)> = vec![
        (VarLocation::Url, serde_json::json!({"kind": "url"})),
        (VarLocation::Body, serde_json::json!({"kind": "body"})),
        (
            VarLocation::BodyForm {
                row: 3,
                part: RowPart::Value,
            },
            serde_json::json!({"kind": "bodyForm", "row": 3, "part": "value"}),
        ),
        (
            VarLocation::BodyMultipart {
                row: 1,
                part: RowPart::Name,
            },
            serde_json::json!({"kind": "bodyMultipart", "row": 1, "part": "name"}),
        ),
        (
            VarLocation::Auth {
                field: "password".into(),
            },
            serde_json::json!({"kind": "auth", "field": "password"}),
        ),
        (
            VarLocation::Param {
                location: ParamIn::Cookie,
                row: 2,
                part: RowPart::Value,
            },
            serde_json::json!({"kind": "param", "location": "cookie", "row": 2, "part": "value"}),
        ),
    ];
    for (loc, want) in cases {
        let v = serde_json::to_value(&loc).unwrap();
        assert_eq!(v, want);
        let back: VarLocation = serde_json::from_value(v).unwrap();
        assert_eq!(back, loc);
    }
}

#[test]
fn unresolved_var_description_for_each_location() {
    let u = |loc| {
        UnresolvedVar {
            name: "n".into(),
            location: loc,
        }
        .describe()
    };
    assert_eq!(u(VarLocation::Url), "{{n}}（URL）");
    assert_eq!(u(VarLocation::Body), "{{n}}（ボディ）");
    assert_eq!(
        u(VarLocation::BodyForm {
            row: 2,
            part: RowPart::Name
        }),
        "{{n}}（ボディ（フォーム）2 行目のキー）"
    );
    assert_eq!(
        u(VarLocation::BodyMultipart {
            row: 1,
            part: RowPart::Value
        }),
        "{{n}}（ボディ（multipart）1 行目の値）"
    );
    for (p, label) in [
        (ParamIn::Query, "クエリパラメータ"),
        (ParamIn::Path, "パスパラメータ"),
        (ParamIn::Header, "ヘッダー"),
        (ParamIn::Cookie, "Cookie"),
    ] {
        assert_eq!(
            u(VarLocation::Param {
                location: p,
                row: 7,
                part: RowPart::Value
            }),
            format!("{{{{n}}}}（{label} 7 行目の値）")
        );
    }
}
