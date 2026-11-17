//! 壊れた入力のエラー文。

mod common;

use nekoreq_curl::{parse, CurlError, Position};

fn err(cmd: &str) -> CurlError {
    parse(cmd).expect_err(cmd)
}

#[test]
fn empty_input() {
    assert_eq!(err(""), CurlError::Empty);
    assert_eq!(err("  \n  "), CurlError::Empty);
    assert_eq!(err("").to_string(), "cURL コマンドが空です");
}

#[test]
fn unterminated_single_quote() {
    let e = err("curl http://x/a -d '{\"a\":1}");
    assert_eq!(
        e,
        CurlError::UnterminatedQuote {
            quote: '\'',
            position: Some(Position {
                line: 1,
                column: 20
            })
        }
    );
    assert_eq!(
        e.to_string(),
        "引用符（'）が閉じられていません（1 行目 20 文字目から）"
    );
}

#[test]
fn unterminated_double_quote_on_second_line() {
    let e = err("curl http://x/a \\\n  -H \"X-A: 1");
    assert_eq!(
        e.to_string(),
        "引用符（\"）が閉じられていません（2 行目 6 文字目から）"
    );
}

#[test]
fn unterminated_ansi_c_quote() {
    let e = err("curl http://x/a --data-raw $'abc");
    assert!(matches!(
        e,
        CurlError::UnterminatedQuote { quote: '\'', .. }
    ));
}

#[test]
fn unterminated_in_cmd_has_no_position() {
    let e = err("curl ^\"http://x/a^\" ^\n -H \"X-A: 1");
    assert_eq!(
        e,
        CurlError::UnterminatedQuote {
            quote: '"',
            position: None
        }
    );
    assert_eq!(e.to_string(), "引用符（\"）が閉じられていません");
}

#[test]
fn no_url() {
    assert_eq!(err("curl"), CurlError::NoUrl);
    assert_eq!(err("curl -X POST -H 'A: b'"), CurlError::NoUrl);
    assert_eq!(
        err("curl -s").to_string(),
        "URL が見つかりません。cURL コマンドに URL を含めてください"
    );
}

#[test]
fn option_without_value() {
    assert_eq!(
        err("curl http://x/a -H"),
        CurlError::MissingArgument {
            option: "-H".into()
        }
    );
    assert_eq!(
        err("curl http://x/a --data").to_string(),
        "オプション --data の値がありません"
    );
    assert_eq!(
        err("curl http://x/a -sX").to_string(),
        "オプション -X の値がありません"
    );
}

#[test]
fn invalid_urls() {
    assert!(matches!(err("curl http:///a"), CurlError::InvalidUrl(_)));
    assert!(matches!(err("curl '://x/a'"), CurlError::InvalidUrl(_)));
    assert!(matches!(err("curl ''"), CurlError::InvalidUrl(_)));
    assert!(err("curl http:///a")
        .to_string()
        .starts_with("URL を読めません: "));
}
