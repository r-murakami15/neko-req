//! テスト担当による独立した検証（storage の公開 API だけを使う）。
//!
//! - 履歴の全文検索: FTS5 の構文として意味を持つ文字・SQL インジェクション・LIKE の特殊文字
//! - 履歴のカーソルのページング: 同じ時刻の大量の行・途中の挿入・削除・壊れたカーソル
//! - 件数の上限（同じ時刻・複数のスレッド・自動で消した行の跡）

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use nekoreq_core::{HistoryBodyKind, HistoryEntry, StatusClass};
use nekoreq_storage::{Database, HistoryFilter};

fn entry(
    project: &str,
    id: &str,
    sent_at: &str,
    url: &str,
    req: Option<&str>,
    resp: Option<&str>,
) -> HistoryEntry {
    HistoryEntry {
        id: id.into(),
        project_id: project.into(),
        sent_at: sent_at.into(),
        environment_id: None,
        environment_name: None,
        op_key: None,
        op_summary: None,
        method: "GET".into(),
        url: url.into(),
        final_url: None,
        request_headers: vec![],
        request_body: req.map(str::to_string),
        request_body_kind: HistoryBodyKind::Text,
        request_body_size: 0,
        request_body_truncated: false,
        request_parts: vec![],
        request_file: None,
        status: Some(200),
        status_text: Some("OK".into()),
        response_headers: vec![],
        response_content_type: None,
        response_body: resp.map(str::to_string),
        response_binary: false,
        response_body_truncated: false,
        response_size: 0,
        timing: None,
        error: None,
        error_kind: None,
        input: None,
        masked_fields: vec![],
        override_names: vec![],
    }
}

fn open(dir: &Path) -> (Database, nekoreq_core::Project) {
    let db = Database::open(dir.join("data.sqlite")).unwrap();
    let p = db.project_create("p").unwrap();
    (db, p)
}

fn f(limit: u32) -> HistoryFilter {
    HistoryFilter {
        limit,
        ..HistoryFilter::default()
    }
}

/// 全部のページをたどって、順番どおりの id を返す。
fn walk(db: &Database, project: &str, base: &HistoryFilter) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10_000 {
        let mut q = base.clone();
        q.cursor = cursor.clone();
        let page = db.history_list(project, &q).unwrap();
        out.extend(page.items.into_iter().map(|i| i.id));
        match page.next_cursor {
            Some(c) => {
                assert_ne!(Some(c.clone()), cursor, "カーソルが進まない");
                cursor = Some(c);
            }
            None => return out,
        }
    }
    panic!("ページングが終わらない");
}

// ---- 全文検索 ----

/// 検索語を含むか（トライグラム: URL と、送ったボディ + 改行 + 受け取ったボディの全体を、大文字小文字を区別せずに）。
/// 3 文字未満は LIKE（URL・送ったボディ・受け取ったボディを別々に）。
fn oracle(docs: &[HistoryEntry], query: &str) -> Vec<String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return docs.iter().map(|d| d.id.clone()).collect();
    }
    docs.iter()
        .filter(|d| {
            let url = d.url.to_lowercase();
            let req = d.request_body.clone().unwrap_or_default().to_lowercase();
            let resp = d.response_body.clone().unwrap_or_default().to_lowercase();
            if q.chars().count() >= 3 {
                url.contains(&q) || format!("{req}\n{resp}").contains(&q)
            } else {
                url.contains(&q) || req.contains(&q) || resp.contains(&q)
            }
        })
        .map(|d| d.id.clone())
        .collect()
}

#[test]
fn search_terms_with_fts_syntax_sql_and_like_characters_behave_as_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    let docs = vec![
        entry(
            &p.id,
            "d1",
            "2026-11-01T00:00:01.000Z",
            "http://x/hello",
            None,
            Some("plain hello world"),
        ),
        entry(
            &p.id,
            "d2",
            "2026-11-01T00:00:02.000Z",
            "http://x/q",
            Some(r#"say "quoted" text"#),
            None,
        ),
        entry(
            &p.id,
            "d3",
            "2026-11-01T00:00:03.000Z",
            "http://x/star",
            None,
            Some("star * and ? and ^caret foo* bar"),
        ),
        entry(
            &p.id,
            "d4",
            "2026-11-01T00:00:04.000Z",
            "http://x/near",
            None,
            Some("NEAR(foo bar) AND baz OR qux NOT quux NEAR/2"),
        ),
        entry(
            &p.id,
            "d5",
            "2026-11-01T00:00:05.000Z",
            "http://x/dash",
            None,
            Some("dash-separated col:on url:foo -foo +plus ( ) { } [ ] , ;"),
        ),
        entry(
            &p.id,
            "d6",
            "2026-11-01T00:00:06.000Z",
            "http://x/sql",
            None,
            Some("'; DROP TABLE history; -- \" OR 1=1"),
        ),
        entry(
            &p.id,
            "d7",
            "2026-11-01T00:00:07.000Z",
            "http://x/like?pct=100%25&u=a_b",
            Some("100% sure_thing back\\slash"),
            Some("100%"),
        ),
        entry(
            &p.id,
            "d8",
            "2026-11-01T00:00:08.000Z",
            "http://x/ja",
            Some("日本語のテスト 😀"),
            Some("絵文字😀とカタカナ"),
        ),
        entry(
            &p.id,
            "d9",
            "2026-11-01T00:00:09.000Z",
            "http://x/both",
            Some("tail-of-request"),
            Some("head-of-response"),
        ),
    ];
    for d in &docs {
        db.history_insert(d, 0).unwrap();
    }
    let long = "a".repeat(10_000);
    let queries: Vec<String> = [
        "\"",
        "\"\"",
        "\"\"\"",
        "\"quoted\"",
        "*",
        "**",
        "?",
        "NEAR(",
        "NEAR(foo",
        "NEAR(foo bar)",
        "NEAR/2",
        "AND",
        "OR",
        "NOT",
        "-",
        "--",
        "-foo",
        "+plus",
        ":",
        "col:on",
        "url:foo",
        "url:",
        "^caret",
        "foo*",
        "foo* bar",
        "'; DROP",
        "DROP TABLE",
        "' OR 1=1 --",
        "\" OR 1=1",
        "%",
        "_",
        "\\",
        "100%",
        "sure_",
        "a_b",
        "back\\slash",
        "😀",
        "日本",
        "日本語",
        "テスト",
        " hello  ",
        "(",
        ")",
        "()",
        "{",
        "[",
        "\"a\" OR \"b\"",
        "a AND b",
        ",",
        ";",
        "\u{1}",
        "tail-of-request\nhead-of-response",
        "ab",
        "zz",
        "é",
        "ＡＢＣ",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(std::iter::once(long))
    .collect();
    for q in &queries {
        let page = db
            .history_list(
                &p.id,
                &HistoryFilter {
                    text: Some(q.clone()),
                    limit: 100,
                    ..HistoryFilter::default()
                },
            )
            .unwrap_or_else(|e| panic!("検索語 {q:?} がエラーになった: {e}"));
        let mut got: Vec<String> = page.items.iter().map(|i| i.id.clone()).collect();
        got.sort();
        let mut want = oracle(&docs, q);
        want.sort();
        assert_eq!(got, want, "検索語 {q:?}");
        assert_eq!(page.total as usize, want.len(), "total: {q:?}");
    }
    // 検索のあとも表は無事
    assert_eq!(db.history_count(&p.id).unwrap(), 9);
    db.history_insert(
        &entry(
            &p.id,
            "d10",
            "2026-11-01T00:00:10.000Z",
            "http://x/after",
            None,
            Some("after"),
        ),
        0,
    )
    .unwrap();
    assert_eq!(db.history_count(&p.id).unwrap(), 10);
}

/// NUL 文字（U+0000）を含む 3 文字以上の検索語は、FTS5 の構文エラー（unterminated string）になる。
#[test]
fn a_search_term_with_a_nul_character_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    db.history_insert(
        &entry(
            &p.id,
            "d1",
            "2026-11-01T00:00:01.000Z",
            "http://x/helloworld",
            None,
            Some("b"),
        ),
        0,
    )
    .unwrap();
    for q in ["hello\u{0}world", "\u{0}\u{0}\u{0}", "abc\u{0}"] {
        let r = db.history_list(
            &p.id,
            &HistoryFilter {
                text: Some(q.to_string()),
                limit: 10,
                ..HistoryFilter::default()
            },
        );
        assert!(r.is_ok(), "検索語 {q:?}: {:?}", r.err());
    }
}

#[test]
fn search_combines_with_filters_and_paging_with_a_stable_total() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    for i in 0..60 {
        let mut e = entry(
            &p.id,
            &format!("h{i:03}"),
            &format!("2026-11-01T00:{:02}:00.000Z", i),
            &format!("http://x/{}", if i % 3 == 0 { "needle" } else { "hay" }),
            None,
            Some("body"),
        );
        e.status = if i % 2 == 0 { Some(200) } else { Some(500) };
        db.history_insert(&e, 0).unwrap();
    }
    let base = HistoryFilter {
        text: Some("needle".into()),
        statuses: vec![StatusClass::S2xx],
        limit: 4,
        ..HistoryFilter::default()
    };
    // i % 3 == 0 かつ偶数 = i % 6 == 0 → 10 件
    let all = walk(&db, &p.id, &base);
    assert_eq!(all.len(), 10);
    let uniq: HashSet<_> = all.iter().collect();
    assert_eq!(uniq.len(), 10);
    assert!(all.windows(2).all(|w| w[0] > w[1]), "新しい順: {all:?}");
    // total はページに関係なく同じ
    let mut cursor = None;
    let mut totals = HashSet::new();
    loop {
        let mut q = base.clone();
        q.cursor = cursor.clone();
        let page = db.history_list(&p.id, &q).unwrap();
        totals.insert(page.total);
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(totals, HashSet::from([10]));
}

// ---- ページング ----

#[test]
fn a_thousand_rows_with_the_same_timestamp_page_without_gaps_or_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    for i in 0..1000 {
        db.history_insert(
            &entry(
                &p.id,
                &format!("s{i:04}"),
                "2026-11-01T00:00:00.000Z",
                "http://x/",
                None,
                Some("b"),
            ),
            0,
        )
        .unwrap();
    }
    for page_size in [1u32, 7, 100, 333, 500] {
        let got = walk(&db, &p.id, &f(page_size));
        assert_eq!(got.len(), 1000, "page size {page_size}");
        let uniq: HashSet<_> = got.iter().collect();
        assert_eq!(uniq.len(), 1000, "page size {page_size}: 重複");
        // 同じ時刻なら、あとから入れたものが先
        let want: Vec<String> = (0..1000).rev().map(|i| format!("s{i:04}")).collect();
        assert_eq!(got, want, "page size {page_size}");
    }
}

#[test]
fn rows_inserted_or_deleted_between_pages_never_cause_duplicates_or_skips() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    for i in 0..100 {
        db.history_insert(
            &entry(
                &p.id,
                &format!("o{i:03}"),
                &format!("2026-11-01T00:{:02}:{:02}.000Z", i / 60, i % 60),
                "http://x/",
                None,
                Some("b"),
            ),
            0,
        )
        .unwrap();
    }
    // 1 ページ目（新しい順に 10 件: o099..o090）
    let page1 = db.history_list(&p.id, &f(10)).unwrap();
    assert_eq!(page1.items.first().unwrap().id, "o099");
    let cursor = page1.next_cursor.clone().unwrap();
    let mut seen: Vec<String> = page1.items.iter().map(|i| i.id.clone()).collect();
    // 途中で: 新しい送信（先頭に追加）・カーソルと同じ時刻の送信・古い時刻（過去の日付）の送信・
    // カーソルの行そのもの（o090）と先の行（o085）・すでに見た行（o095）の削除
    db.history_insert(
        &entry(
            &p.id,
            "new1",
            "2026-11-02T00:00:00.000Z",
            "http://x/",
            None,
            Some("b"),
        ),
        0,
    )
    .unwrap();
    db.history_insert(
        &entry(
            &p.id,
            "same-as-cursor",
            "2026-11-01T00:01:30.000Z",
            "http://x/",
            None,
            Some("b"),
        ),
        0,
    )
    .unwrap();
    db.history_insert(
        &entry(
            &p.id,
            "backdated",
            "2026-10-01T00:00:00.000Z",
            "http://x/",
            None,
            Some("b"),
        ),
        0,
    )
    .unwrap();
    db.history_delete("o090").unwrap();
    db.history_delete("o085").unwrap();
    db.history_delete("o095").unwrap();
    // 続き
    let mut rest = Vec::new();
    let mut cur = Some(cursor);
    while let Some(c) = cur {
        let mut q = f(10);
        q.cursor = Some(c);
        let page = db.history_list(&p.id, &q).unwrap();
        rest.extend(page.items.into_iter().map(|i| i.id));
        cur = page.next_cursor;
    }
    // 重複なし
    let all: Vec<String> = seen.iter().cloned().chain(rest.iter().cloned()).collect();
    let uniq: HashSet<_> = all.iter().collect();
    assert_eq!(uniq.len(), all.len(), "重複: {all:?}");
    // 続きは、まだ見ていなかった古い側の行（o089..o000 から o085 を除く）と、過去の日付の行だけ
    let mut want: Vec<String> = (0..90).rev().map(|i| format!("o{i:03}")).collect();
    want.retain(|id| id != "o085");
    want.push("backdated".into());
    assert_eq!(rest, want);
    // 先頭に入った新しいものは、この一覧では出ない（次に取り直すと出る）
    assert!(!all.contains(&"new1".to_string()));
    let fresh = db.history_list(&p.id, &f(3)).unwrap();
    assert_eq!(fresh.items[0].id, "new1");
    seen.clear();
}

#[test]
fn broken_cursors_do_not_error_and_never_inject_sql() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    for i in 0..5 {
        db.history_insert(
            &entry(
                &p.id,
                &format!("c{i}"),
                &format!("2026-11-01T00:00:0{i}.000Z"),
                "http://x/",
                None,
                Some("b"),
            ),
            0,
        )
        .unwrap();
    }
    for cursor in [
        "",
        "abc",
        "|",
        "x|y",
        "2026-11-01T00:00:03.000Z|",
        "2026-11-01T00:00:03.000Z|abc",
        "2026-11-01T00:00:03.000Z|99999999999999999999",
        "2026-11-01T00:00:03.000Z|-5",
        "'; DROP TABLE history; --|1",
        "2026-11-01T00:00:03.000Z' OR '1'='1|1",
        "||||",
        "\u{0}|1",
    ] {
        let page = db
            .history_list(
                &p.id,
                &HistoryFilter {
                    cursor: Some(cursor.to_string()),
                    limit: 100,
                    ..HistoryFilter::default()
                },
            )
            .unwrap_or_else(|e| panic!("cursor {cursor:?}: {e}"));
        assert!(page.items.len() <= 5);
        assert_eq!(page.total, 5);
    }
    // 表は無事
    assert_eq!(db.history_count(&p.id).unwrap(), 5);
    // 範囲外の新しいカーソルは空、古いカーソルは全部
    let page = db
        .history_list(
            &p.id,
            &HistoryFilter {
                cursor: Some("2000-01-01T00:00:00.000Z|1".into()),
                limit: 100,
                ..HistoryFilter::default()
            },
        )
        .unwrap();
    assert!(page.items.is_empty() && page.next_cursor.is_none());
}

// ---- 件数の上限 ----

fn row_count(path: &Path, table: &str) -> i64 {
    rusqlite::Connection::open(path.join("data.sqlite"))
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn the_cap_keeps_the_newest_rows_for_equal_timestamps_and_is_per_project() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a) = open(dir.path());
    let b = db.project_create("b").unwrap();
    for i in 0..10 {
        db.history_insert(
            &entry(
                &a.id,
                &format!("a{i}"),
                "2026-11-01T00:00:00.000Z",
                "http://a/",
                None,
                Some("x"),
            ),
            3,
        )
        .unwrap();
        db.history_insert(
            &entry(
                &b.id,
                &format!("b{i}"),
                "2026-11-01T00:00:00.000Z",
                "http://b/",
                None,
                Some("x"),
            ),
            5,
        )
        .unwrap();
    }
    let ids_a = walk(&db, &a.id, &f(100));
    assert_eq!(ids_a, ["a9", "a8", "a7"]);
    assert_eq!(walk(&db, &b.id, &f(100)), ["b9", "b8", "b7", "b6", "b5"]);
    assert_eq!(row_count(dir.path(), "history"), 8);
    assert_eq!(row_count(dir.path(), "history_fts"), 8);
    // 上限 1・上限が現在の件数より大きい（減らさない）
    db.history_insert(
        &entry(
            &a.id,
            "a10",
            "2026-11-01T00:00:00.000Z",
            "http://a/",
            None,
            Some("x"),
        ),
        1,
    )
    .unwrap();
    assert_eq!(walk(&db, &a.id, &f(100)), ["a10"]);
    db.history_insert(
        &entry(
            &a.id,
            "a11",
            "2026-11-01T00:00:00.000Z",
            "http://a/",
            None,
            Some("x"),
        ),
        1000,
    )
    .unwrap();
    assert_eq!(walk(&db, &a.id, &f(100)), ["a11", "a10"]);
    // 0 は上限なし
    db.history_insert(
        &entry(
            &a.id,
            "a12",
            "2026-11-01T00:00:00.000Z",
            "http://a/",
            None,
            Some("x"),
        ),
        0,
    )
    .unwrap();
    assert_eq!(walk(&db, &a.id, &f(100)).len(), 3);
}

#[test]
fn many_threads_inserting_with_a_cap_end_with_exactly_the_cap_and_a_matching_index() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    let db = Arc::new(db);
    let mut handles = Vec::new();
    for t in 0..8 {
        let db = db.clone();
        let pid = p.id.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..50 {
                db.history_insert(
                    &entry(
                        &pid,
                        &format!("t{t}-{i:02}"),
                        "2026-11-01T00:00:00.000Z",
                        "http://x/",
                        None,
                        Some(&format!("marker-{t}-{i}")),
                    ),
                    50,
                )
                .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(db.history_count(&p.id).unwrap(), 50);
    assert_eq!(row_count(dir.path(), "history"), 50);
    assert_eq!(row_count(dir.path(), "history_fts"), 50);
    // どのスレッドの最後の行も、順序が保たれている（同じスレッドの行は、あとのものが先に出る）
    let all = walk(&db, &p.id, &f(500));
    for t in 0..8 {
        let mine: Vec<&String> = all
            .iter()
            .filter(|id| id.starts_with(&format!("t{t}-")))
            .collect();
        assert!(mine.windows(2).all(|w| w[0] > w[1]), "t{t}: {mine:?}");
    }
}

fn contains(dir: &Path, needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
            hits.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    hits
}

/// 設定の件数を超えて自動で消した行の中身は、本体にも WAL にも全文検索の領域にも残らない。
#[test]
fn rows_trimmed_by_the_cap_leave_no_text_in_any_file() {
    let dir = tempfile::tempdir().unwrap();
    let (db, p) = open(dir.path());
    db.history_insert(
        &entry(
            &p.id,
            "old",
            "2026-11-01T00:00:00.000Z",
            "http://x/TRIM-MARKER-URL",
            Some("TRIM-MARKER-REQ"),
            Some("TRIM-MARKER-RESP"),
        ),
        2,
    )
    .unwrap();
    for i in 1..=3 {
        db.history_insert(
            &entry(
                &p.id,
                &format!("n{i}"),
                &format!("2026-11-01T00:00:0{i}.000Z"),
                "http://x/",
                None,
                Some("fresh"),
            ),
            2,
        )
        .unwrap();
    }
    assert!(db.history_get("old").unwrap().is_none());
    for marker in ["TRIM-MARKER-URL", "TRIM-MARKER-REQ", "TRIM-MARKER-RESP"] {
        let hits = contains(dir.path(), marker);
        assert!(hits.is_empty(), "{marker} が残っているファイル: {hits:?}");
    }
}
