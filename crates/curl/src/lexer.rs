//! コマンド文字列を単語に分ける。
//!
//! - bash 系（`Shell::Bash`）: 行末の `\`（行継続）、シングル・ダブルクォート、`$'...'`（ANSI-C クォート）を読む。
//!   `$'...'` と行継続は前処理で普通の引用符に直してから `shell-words` に渡す。
//! - Windows cmd（`Shell::Cmd`）: 行末の `^`（行継続）、`^` によるエスケープ（Chrome の `^"`・`^\^"`・`^{`）、
//!   ダブルクォートだけの引用、`""`（引用符の中の `"`）、`"%"` を読む。cmd.exe の `^` の処理のあと、
//!   MS C ランタイムの引数の分け方（`\"`、`\\"`）で単語に分ける。

use crate::error::{CurlError, Position};
use crate::Shell;

/// 貼り付けた文字列がどちらのシェルの書き方か見分ける。
/// 行末の `^`（`\` の継続がない）か `^"` を含むものは cmd、それ以外は bash。
pub fn detect_shell(cmd: &str) -> Shell {
    let normalized = cmd.replace("\r\n", "\n");
    let ends_with_caret = normalized.lines().any(|l| l.trim_end().ends_with('^'));
    let ends_with_backslash = normalized.lines().any(|l| l.trim_end().ends_with('\\'));
    let caret_quote = normalized.contains("^\"") || normalized.contains("^\\^\"");
    let starts_exe = normalized.trim_start().starts_with("curl.exe");
    if (ends_with_caret && !ends_with_backslash)
        || caret_quote
        || starts_exe
        || has_doubled_quote_in_word(&normalized)
    {
        Shell::Cmd
    } else {
        Shell::Bash
    }
}

/// 古い Chrome・Firefox の cmd 用の書き方（`"{""a"":1}"`。単語の途中の `""` が引用符 1 つ）か。
/// bash で単語の途中に `""` を書くことはまずないが、シングルクォートの中の JSON の空文字列（`:""}`）と
/// 区別するため、シングルクォートが 1 つもないものだけを対象にする。
fn has_doubled_quote_in_word(s: &str) -> bool {
    if s.contains('\'') {
        return false;
    }
    let c: Vec<char> = s.chars().collect();
    c.windows(4).any(|w| {
        !w[0].is_whitespace()
            && w[0] != '"'
            && w[1] == '"'
            && w[2] == '"'
            && !w[3].is_whitespace()
            && w[3] != '"'
    })
}

pub(crate) fn tokenize(input: &str, shell: Shell) -> Result<Vec<String>, CurlError> {
    let text = input.replace("\r\n", "\n");
    match shell {
        Shell::Bash => {
            let normalized = normalize_posix(&text)?;
            shell_words::split(&normalized).map_err(|e| CurlError::Syntax(e.to_string()))
        }
        Shell::Cmd => split_msvcrt(&unescape_cmd(&text))?
            .into_iter()
            .map(Ok)
            .collect(),
    }
}

fn position_of(chars: &[char], index: usize) -> Position {
    let mut line = 1;
    let mut column = 1;
    for &c in &chars[..index.min(chars.len())] {
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Position { line, column }
}

/// 行継続を除き、`$'...'` を `'...'` に直す。引用符が閉じていなければエラー。
fn normalize_posix(s: &str) -> Result<String, CurlError> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => match chars.get(i + 1) {
                Some('\n') => i += 2,
                Some(&n) => {
                    out.push('\\');
                    out.push(n);
                    i += 2;
                }
                // 末尾に残った `\`（貼り付けで継続の続きが切れた）は捨てる
                None => i += 1,
            },
            '\'' => {
                let start = i;
                out.push('\'');
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err(unterminated('\'', &chars, start)),
                        Some('\'') => {
                            out.push('\'');
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            out.push(ch);
                            i += 1;
                        }
                    }
                }
            }
            '"' => {
                let start = i;
                out.push('"');
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err(unterminated('"', &chars, start)),
                        Some('"') => {
                            out.push('"');
                            i += 1;
                            break;
                        }
                        Some('\\') => match chars.get(i + 1) {
                            Some('\n') => i += 2,
                            Some(&n) => {
                                out.push('\\');
                                out.push(n);
                                i += 2;
                            }
                            None => return Err(unterminated('"', &chars, start)),
                        },
                        Some(&ch) => {
                            out.push(ch);
                            i += 1;
                        }
                    }
                }
            }
            '$' if chars.get(i + 1) == Some(&'\'') => {
                let start = i;
                i += 2;
                let mut bytes: Vec<u8> = Vec::new();
                loop {
                    match chars.get(i) {
                        None => return Err(unterminated('\'', &chars, start)),
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some('\\') => {
                            i += 1;
                            match decode_ansi_escape(&chars, &mut i, &mut bytes) {
                                Ok(()) => {}
                                Err(()) => return Err(unterminated('\'', &chars, start)),
                            }
                        }
                        Some(&ch) => {
                            push_char(&mut bytes, ch);
                            i += 1;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&bytes).replace('\'', "'\\''");
                out.push('\'');
                out.push_str(&text);
                out.push('\'');
            }
            // `$"..."`（ロケール付きの引用）は普通のダブルクォートとして読む
            '$' if chars.get(i + 1) == Some(&'"') => i += 1,
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn unterminated(quote: char, chars: &[char], start: usize) -> CurlError {
    CurlError::UnterminatedQuote {
        quote,
        position: Some(position_of(chars, start)),
    }
}

fn push_char(bytes: &mut Vec<u8>, c: char) {
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

fn hex_value(c: char) -> Option<u32> {
    c.to_digit(16)
}

/// `$'...'` の中の `\` のあとを読む（`i` は `\` の次を指す）。
/// バイト列に積む（`\xNN` と `\NNN` は 1 バイト、`\u`・`\U` は文字）。Chrome が出す UTF-16 のサロゲートの組
/// （`😀`）は 1 文字にまとめる。閉じていないときは `Err`。
fn decode_ansi_escape(chars: &[char], i: &mut usize, bytes: &mut Vec<u8>) -> Result<(), ()> {
    let Some(&c) = chars.get(*i) else {
        return Err(());
    };
    *i += 1;
    match c {
        'n' => bytes.push(b'\n'),
        't' => bytes.push(b'\t'),
        'r' => bytes.push(b'\r'),
        'a' => bytes.push(0x07),
        'b' => bytes.push(0x08),
        'e' | 'E' => bytes.push(0x1b),
        'f' => bytes.push(0x0c),
        'v' => bytes.push(0x0b),
        '\\' => bytes.push(b'\\'),
        '\'' => bytes.push(b'\''),
        '"' => bytes.push(b'"'),
        '?' => bytes.push(b'?'),
        '0'..='7' => {
            let mut v = c.to_digit(8).unwrap_or(0);
            for _ in 0..2 {
                match chars.get(*i).and_then(|d| d.to_digit(8)) {
                    Some(d) => {
                        v = v * 8 + d;
                        *i += 1;
                    }
                    None => break,
                }
            }
            bytes.push((v & 0xff) as u8);
        }
        'x' => {
            let mut v = 0u32;
            let mut n = 0;
            while n < 2 {
                match chars.get(*i).copied().and_then(hex_value) {
                    Some(d) => {
                        v = v * 16 + d;
                        *i += 1;
                        n += 1;
                    }
                    None => break,
                }
            }
            if n == 0 {
                bytes.extend_from_slice(b"\\x");
            } else {
                bytes.push(v as u8);
            }
        }
        'u' | 'U' => {
            let max = if c == 'u' { 4 } else { 8 };
            let (mut v, n) = read_hex(chars, i, max);
            if n == 0 {
                bytes.push(b'\\');
                bytes.push(c as u8);
            } else {
                // 上位サロゲートに下位サロゲートが続くときは 1 文字にする
                if (0xD800..0xDC00).contains(&v)
                    && chars.get(*i) == Some(&'\\')
                    && chars.get(*i + 1) == Some(&'u')
                {
                    let save = *i;
                    *i += 2;
                    let (lo, ln) = read_hex(chars, i, 4);
                    if ln == 4 && (0xDC00..0xE000).contains(&lo) {
                        v = 0x10000 + ((v - 0xD800) << 10) + (lo - 0xDC00);
                    } else {
                        *i = save;
                    }
                }
                push_char(bytes, char::from_u32(v).unwrap_or('\u{FFFD}'));
            }
        }
        'c' => match chars.get(*i) {
            Some(&x) => {
                *i += 1;
                bytes.push((x as u32 & 0x1f) as u8);
            }
            None => return Err(()),
        },
        other => {
            bytes.push(b'\\');
            push_char(bytes, other);
        }
    }
    Ok(())
}

fn read_hex(chars: &[char], i: &mut usize, max: usize) -> (u32, usize) {
    let mut v = 0u32;
    let mut n = 0;
    while n < max {
        match chars.get(*i).copied().and_then(hex_value) {
            Some(d) => {
                v = v.wrapping_mul(16).wrapping_add(d);
                *i += 1;
                n += 1;
            }
            None => break,
        }
    }
    (v, n)
}

/// cmd.exe の `^` の処理。引用符（`"`）の外の `^X` は `X` をそのまま残す（`^"` の `"` は引用符の切り替えにしない）。
/// `^` + 改行は行継続（その次の改行 1 つは文字として残す。Chrome・Firefox の「`"^\n\n"`」の形）。
/// 引用符の中では `^` は普通の文字。引用符の外の改行は空白にする（貼り付けで継続の `^` が欠けた複数行を読めるように）。
fn unescape_cmd(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_quote = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                in_quote = !in_quote;
                out.push('"');
                i += 1;
            }
            '^' if !in_quote => match chars.get(i + 1) {
                Some('\n') => {
                    i += 2;
                    if chars.get(i) == Some(&'\n') {
                        out.push('\n');
                        i += 1;
                    }
                }
                Some(&n) => {
                    out.push(n);
                    i += 2;
                }
                None => i += 1,
            },
            '\n' if !in_quote => {
                out.push(' ');
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// MS C ランタイム（`CommandLineToArgvW` に近い）の規則で単語に分ける。
/// 空白は空白とタブだけ（改行は文字）。`"` で囲んだ中では `""` が `"` 1 つ。`\` の連なりは、
/// 直後が `"` のときだけ（2n 個 → n 個 + 引用符の切り替え、2n+1 個 → n 個 + 文字の `"`）数え直す。
fn split_msvcrt(s: &str) -> Result<Vec<String>, CurlError> {
    let chars: Vec<char> = s.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut in_quote = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let mut n = 0;
                while chars.get(i + n) == Some(&'\\') {
                    n += 1;
                }
                started = true;
                if chars.get(i + n) == Some(&'"') {
                    cur.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 {
                        cur.push('"');
                        i += n + 1;
                    } else {
                        // 引用符の切り替えは次の周で処理する
                        i += n;
                    }
                } else {
                    cur.extend(std::iter::repeat_n('\\', n));
                    i += n;
                }
            }
            '"' => {
                started = true;
                if in_quote {
                    if chars.get(i + 1) == Some(&'"') {
                        cur.push('"');
                        i += 2;
                    } else {
                        in_quote = false;
                        i += 1;
                    }
                } else {
                    in_quote = true;
                    i += 1;
                }
            }
            ' ' | '\t' if !in_quote => {
                if started {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
                i += 1;
            }
            _ => {
                started = true;
                cur.push(c);
                i += 1;
            }
        }
    }
    if in_quote {
        return Err(CurlError::UnterminatedQuote {
            quote: '"',
            position: None,
        });
    }
    if started {
        words.push(cur);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(s: &str) -> Vec<String> {
        tokenize(s, Shell::Bash).unwrap()
    }
    fn cmd(s: &str) -> Vec<String> {
        tokenize(s, Shell::Cmd).unwrap()
    }

    #[test]
    fn bash_continuation_and_quotes() {
        assert_eq!(
            bash("curl -X POST \\\n  -H 'A: b c' \\\n  \"http://x\""),
            ["curl", "-X", "POST", "-H", "A: b c", "http://x"]
        );
    }

    #[test]
    fn ansi_c_quotes() {
        assert_eq!(
            bash(r"echo $'a\nb\'cあ😀\xe3\x81\x82'"),
            ["echo", "a\nb'cあ😀あ"]
        );
    }

    #[test]
    fn unterminated_reports_position() {
        let e = tokenize("curl -d 'abc\ndef", Shell::Bash).unwrap_err();
        assert_eq!(
            e,
            CurlError::UnterminatedQuote {
                quote: '\'',
                position: Some(Position { line: 1, column: 9 })
            }
        );
    }

    #[test]
    fn cmd_chrome_caret_quotes() {
        assert_eq!(
            cmd("curl ^\"http://x/^\" ^\n  --data-raw ^\"^{^\\^\"a^\\^\":1^}^\""),
            ["curl", "http://x/", "--data-raw", "{\"a\":1}"]
        );
    }

    #[test]
    fn cmd_double_quote_style() {
        assert_eq!(
            cmd("curl \"http://x\" -d \"a\"\"b\"%\"c\" -H \"\""),
            ["curl", "http://x", "-d", "a\"b%c", "-H", ""]
        );
    }

    #[test]
    fn detects_shell() {
        assert_eq!(detect_shell("curl 'x' \\\n -H 'a'"), Shell::Bash);
        assert_eq!(detect_shell("curl \"x\" ^\r\n -H \"a\""), Shell::Cmd);
        assert_eq!(detect_shell("curl -s http://x"), Shell::Bash);
    }
}
