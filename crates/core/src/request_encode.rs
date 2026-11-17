//! URL・フォームに値を入れるときのパーセントエンコードの種類。

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};

/// クエリの名前と値。英数字と `-._~` 以外は変換する（空白は `%20`）。
pub(crate) const QUERY_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// パスの 1 区間。`/` も変換する。
pub(crate) const PATH_SEGMENT: &AsciiSet = QUERY_COMPONENT;

/// `application/x-www-form-urlencoded`（空白は `+` にするので、ここでは `%20` のまま）。
pub(crate) const FORM_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'*')
    .remove(b'-')
    .remove(b'.')
    .remove(b'_');
