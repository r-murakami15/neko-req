//! description（Markdown）の読み取り。表の有無と、「返すエラー」の表（status・code・状況）。

use crate::model::ErrorCodeEntry;

fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|')
        && t.contains("---")
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Markdown の表（ヘッダー行 + 区切り行）を含むか。
pub fn has_markdown_table(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    lines
        .windows(2)
        .any(|w| w[0].trim().starts_with('|') && is_separator_row(w[1]))
}

/// description 中の表から、ヘッダーに `status` と `code` の列がある表の行を読む。
/// 3 列目以降（`状況` など）は `note` にまとめる。
pub fn error_codes(text: &str) -> Vec<ErrorCodeEntry> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < lines.len() {
        if lines[i].trim().starts_with('|') && is_separator_row(lines[i + 1]) {
            let header: Vec<String> = split_row(lines[i])
                .into_iter()
                .map(|h| h.to_ascii_lowercase())
                .collect();
            let status_col = header.iter().position(|h| h == "status");
            let code_col = header.iter().position(|h| h == "code");
            let mut j = i + 2;
            while j < lines.len() && lines[j].trim().starts_with('|') {
                if let (Some(sc), Some(cc)) = (status_col, code_col) {
                    let cells = split_row(lines[j]);
                    if let (Some(status), Some(code)) = (cells.get(sc), cells.get(cc)) {
                        if !status.is_empty() && !code.is_empty() {
                            let note = cells
                                .iter()
                                .enumerate()
                                .filter(|(k, _)| *k != sc && *k != cc)
                                .map(|(_, c)| c.as_str())
                                .filter(|c| !c.is_empty())
                                .collect::<Vec<_>>()
                                .join(" / ");
                            out.push(ErrorCodeEntry {
                                status: status.clone(),
                                code: code.clone(),
                                note,
                            });
                        }
                    }
                }
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = "本文。\n- 箇条書き\n\n画面に出すメッセージ:\n| 状況 | メッセージ | 返し方 |\n| --- | --- | --- |\n| ログインできない | メールアドレスまたはパスワードが正しくありません | 401 (LOGIN_FAILED) |\n\n返すエラー:\n| status | code | 状況 |\n| --- | --- | --- |\n| 401 | LOGIN_FAILED | ログインの失敗 |\n| 409 | INVALID_STATE | 期限切れ |\n";

    #[test]
    fn detects_tables() {
        assert!(has_markdown_table(DESC));
        assert!(!has_markdown_table("a | b\nc | d"));
        assert!(!has_markdown_table("- 箇条書き\n- だけ"));
    }

    #[test]
    fn reads_error_code_table_only() {
        let e = error_codes(DESC);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].status, "401");
        assert_eq!(e[0].code, "LOGIN_FAILED");
        assert_eq!(e[0].note, "ログインの失敗");
        assert_eq!(e[1].code, "INVALID_STATE");
    }
}
