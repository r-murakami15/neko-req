//! M6 で足したもの（履歴・全文検索・タブの復元）とマイグレーション v6 のテスト。

use nekoreq_core::{
    HeaderPair, HistoryBodyKind, HistoryEntry, HistoryPart, OpKey, RequestSpec, SessionState,
    SessionTab, SessionTabKind, StatusClass, Timing,
};

use crate::{Database, HistoryFilter};

fn entry(project_id: &str, n: u32) -> HistoryEntry {
    HistoryEntry {
        id: format!("h{n:06}"),
        project_id: project_id.to_string(),
        // n が大きいほど新しい（2026-11-01 から 1 分ずつ）
        sent_at: format!(
            "2026-11-{:02}T{:02}:{:02}:00.000Z",
            1 + (n / 1440) % 28,
            (n / 60) % 24,
            n % 60
        ),
        environment_id: Some("env1".into()),
        environment_name: Some("ローカル".into()),
        op_key: Some(OpKey::Id("getClientList".into())),
        op_summary: Some("取引先一覧取得".into()),
        method: "GET".into(),
        url: format!("http://localhost:8080/api/v1/video/clients?q={n}"),
        final_url: None,
        request_headers: vec![HeaderPair {
            name: "Accept".into(),
            value: "*/*".into(),
        }],
        request_body: None,
        request_body_kind: HistoryBodyKind::None,
        request_body_size: 0,
        request_body_truncated: false,
        request_parts: vec![],
        request_file: None,
        status: Some(200),
        status_text: Some("OK".into()),
        response_headers: vec![],
        response_content_type: Some("application/json".into()),
        response_body: Some(format!("{{\"n\":{n}}}")),
        response_binary: false,
        response_body_truncated: false,
        response_size: 10,
        timing: Some(Timing {
            dns_ms: None,
            connect_ms: Some(1.0),
            tls_ms: None,
            send_ms: None,
            wait_ms: Some(5.0),
            download_ms: Some(1.0),
            total_ms: 7.0,
        }),
        error: None,
        error_kind: None,
        input: Some(RequestSpec {
            method: "GET".into(),
            url: "{{baseUrl}}/api/v1/video/clients".into(),
            params: vec![],
            body: nekoreq_core::Body::None,
            auth: None,
        }),
        masked_fields: vec![],
        override_names: vec![],
    }
}

fn filter() -> HistoryFilter {
    HistoryFilter {
        limit: 100,
        ..HistoryFilter::default()
    }
}

fn ids(db: &Database, project: &str, f: &HistoryFilter) -> Vec<String> {
    db.history_list(project, f)
        .unwrap()
        .items
        .into_iter()
        .map(|i| i.id)
        .collect()
}

#[test]
fn migration_v6_creates_history_and_sessions() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn();
    for table in ["history", "history_fts", "sessions"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{table}");
    }
}

#[test]
fn insert_and_get_round_trip_every_field() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("p").unwrap();
    let mut e = entry(&p.id, 1);
    e.request_body = Some("name=value\nfile=@/tmp/a.txt".into());
    e.request_body_kind = HistoryBodyKind::Multipart;
    e.request_body_size = 123;
    e.request_body_truncated = true;
    e.request_parts = vec![
        HistoryPart {
            name: "name".into(),
            is_file: false,
            value: "value".into(),
            content_type: Some("text/plain".into()),
        },
        HistoryPart {
            name: "file".into(),
            is_file: true,
            value: "/tmp/a.txt".into(),
            content_type: None,
        },
    ];
    e.request_file = Some("/tmp/x.bin".into());
    e.final_url = Some("http://localhost:8080/final".into());
    e.masked_fields = vec!["Authorization ヘッダー".into()];
    e.override_names = vec!["token".into()];
    e.response_binary = true;
    e.response_body_truncated = true;
    e.response_headers = vec![HeaderPair {
        name: "Set-Cookie".into(),
        value: "SESSION=••••; Path=/".into(),
    }];
    db.history_insert(&e, 1000).unwrap();
    let got = db.history_get(&e.id).unwrap().unwrap();
    assert_eq!(got, e);
    assert_eq!(db.history_get("nope").unwrap(), None);

    // 失敗（応答なし）の行
    let mut f = entry(&p.id, 2);
    f.status = None;
    f.status_text = None;
    f.response_body = None;
    f.response_headers = vec![];
    f.timing = None;
    f.error = Some("接続できません".into());
    f.error_kind = Some("connect".into());
    f.input = None;
    db.history_insert(&f, 1000).unwrap();
    assert_eq!(db.history_get(&f.id).unwrap().unwrap(), f);
    let item = db.history_item(&f.id).unwrap().unwrap();
    assert!(item.failed);
    assert_eq!(item.total_ms, None);
    assert_eq!(item.error_kind.as_deref(), Some("connect"));
}

#[test]
fn old_entries_are_dropped_per_project() {
    let db = Database::open_in_memory().unwrap();
    let a = db.project_create("a").unwrap();
    let b = db.project_create("b").unwrap();
    for n in 0..10 {
        db.history_insert(&entry(&a.id, n), 3).unwrap();
    }
    let mut other = entry(&b.id, 100);
    other.id = "other".into();
    db.history_insert(&other, 3).unwrap();
    // 新しい 3 件だけ残る
    assert_eq!(
        ids(&db, &a.id, &filter()),
        vec!["h000009", "h000008", "h000007"]
    );
    assert_eq!(db.history_count(&a.id).unwrap(), 3);
    // 別のプロジェクトは数えない
    assert_eq!(db.history_count(&b.id).unwrap(), 1);
    // 古いものの全文検索の行も消えている
    let n: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM history_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 4);
    // 上限 0 は「消さない」（呼び出し側が保存しないと決める）
    for n in 20..25 {
        db.history_insert(&entry(&a.id, n), 0).unwrap();
    }
    assert_eq!(db.history_count(&a.id).unwrap(), 8);
}

#[test]
fn list_is_newest_first_and_pages_through_everything() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("p").unwrap();
    // sent_at が同じものも含める（seq で順序が決まる）
    for n in 0..25 {
        let mut e = entry(&p.id, n / 2 * 2);
        e.id = format!("h{n:06}");
        db.history_insert(&e, 1000).unwrap();
    }
    let mut seen: Vec<String> = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let rows = db
            .history_list(
                &p.id,
                &HistoryFilter {
                    limit: 10,
                    cursor: cursor.clone(),
                    ..HistoryFilter::default()
                },
            )
            .unwrap();
        assert_eq!(rows.total, 25);
        seen.extend(rows.items.iter().map(|i| i.id.clone()));
        pages += 1;
        match rows.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen.len(), 25);
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 25, "重複も抜けもない");
    // 新しい順（sent_at の降順、同じなら seq の降順）
    assert_eq!(seen[0], "h000024");
    assert_eq!(seen[1], "h000023");
    assert_eq!(seen[24], "h000000");
}

#[test]
fn filters_by_status_environment_and_period() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("p").unwrap();
    let mk = |n: u32, status: Option<u16>, env: &str| {
        let mut e = entry(&p.id, n);
        e.status = status;
        e.environment_id = Some(env.into());
        if status.is_none() {
            e.timing = None;
        }
        db.history_insert(&e, 1000).unwrap();
    };
    mk(1, Some(200), "env1");
    mk(2, Some(204), "env1");
    mk(3, Some(302), "env1");
    mk(4, Some(404), "env2");
    mk(5, Some(500), "env2");
    mk(6, None, "env2");
    let q = |statuses: Vec<StatusClass>| {
        ids(
            &db,
            &p.id,
            &HistoryFilter {
                statuses,
                limit: 100,
                ..HistoryFilter::default()
            },
        )
    };
    assert_eq!(q(vec![StatusClass::S2xx]), vec!["h000002", "h000001"]);
    assert_eq!(q(vec![StatusClass::S3xx]), vec!["h000003"]);
    assert_eq!(
        q(vec![StatusClass::S4xx, StatusClass::S5xx]),
        vec!["h000005", "h000004"]
    );
    assert_eq!(q(vec![StatusClass::Failed]), vec!["h000006"]);
    assert_eq!(q(vec![]).len(), 6);

    let env2 = ids(
        &db,
        &p.id,
        &HistoryFilter {
            environment_id: Some("env2".into()),
            limit: 100,
            ..HistoryFilter::default()
        },
    );
    assert_eq!(env2, vec!["h000006", "h000005", "h000004"]);

    // 期間: sent_at は 2026-11-01T00:0n
    let period = ids(
        &db,
        &p.id,
        &HistoryFilter {
            from_utc: Some("2026-11-01T00:02:00.000Z".into()),
            to_utc: Some("2026-11-01T00:05:00.000Z".into()),
            limit: 100,
            ..HistoryFilter::default()
        },
    );
    assert_eq!(
        period,
        vec!["h000004", "h000003", "h000002"],
        "終わりは含まない"
    );

    // 組み合わせ
    let both = ids(
        &db,
        &p.id,
        &HistoryFilter {
            statuses: vec![StatusClass::S4xx],
            environment_id: Some("env1".into()),
            limit: 100,
            ..HistoryFilter::default()
        },
    );
    assert!(both.is_empty());
}

#[test]
fn full_text_search_finds_japanese_and_short_terms() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("p").unwrap();
    let mut a = entry(&p.id, 1);
    a.url = "http://localhost:8080/api/v1/video/clients?q=Netflix".into();
    a.response_body = Some(r#"{"name":"株式会社ネコネット","code":"AB"}"#.into());
    let mut b = entry(&p.id, 2);
    b.request_body = Some(r#"{"title":"入力内容に誤りがあります"}"#.into());
    b.response_body = Some(r#"{"detail":"VALIDATION_FAILED"}"#.into());
    let mut c = entry(&p.id, 3);
    c.url = "http://localhost:8080/api/v1/auth/session".into();
    c.response_body = Some("100%_done \\ ok".into());
    for e in [&a, &b, &c] {
        db.history_insert(e, 1000).unwrap();
    }
    let find = |text: &str| {
        ids(
            &db,
            &p.id,
            &HistoryFilter {
                text: Some(text.into()),
                limit: 100,
                ..HistoryFilter::default()
            },
        )
    };
    // URL
    assert_eq!(find("Netflix"), vec![a.id.clone()]);
    assert_eq!(
        find("netflix"),
        vec![a.id.clone()],
        "大文字小文字は区別しない"
    );
    // 日本語（レスポンス・リクエストのボディ）
    assert_eq!(find("ネコネット"), vec![a.id.clone()]);
    assert_eq!(find("株式会社"), vec![a.id.clone()]);
    assert_eq!(find("誤りが"), vec![b.id.clone()]);
    assert_eq!(find("VALIDATION"), vec![b.id.clone()]);
    // 3 文字未満は部分一致（LIKE）
    assert_eq!(find("株式"), vec![a.id.clone()]);
    assert_eq!(find("AB"), vec![a.id.clone()]);
    assert_eq!(find("誤"), vec![b.id.clone()]);
    // LIKE の特別な文字は文字として探す
    assert_eq!(find("%_"), vec![c.id.clone()]);
    assert_eq!(find("\\ "), vec![c.id.clone()]);
    // 引用符・FTS の記号が入っていても壊れない
    assert!(find("\"Netflix\" OR x*").is_empty());
    assert_eq!(find("q=Netflix"), vec![a.id.clone()]);
    // 空白だけは絞らない
    assert_eq!(find("   ").len(), 3);
    // 当てはまらない
    assert!(find("存在しない語句").is_empty());
    // 総数も絞り込みに従う
    let rows = db
        .history_list(
            &p.id,
            &HistoryFilter {
                text: Some("Netflix".into()),
                limit: 1,
                ..HistoryFilter::default()
            },
        )
        .unwrap();
    assert_eq!(rows.total, 1);
}

#[test]
fn delete_and_clear_remove_search_rows_and_the_text_from_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let db = Database::open(&path).unwrap();
    let p = db.project_create("p").unwrap();
    let marker = "ユニークな目印ZXQ9981";
    let mut e = entry(&p.id, 1);
    e.response_body = Some(format!("{{\"m\":\"{marker}\"}}"));
    e.url = format!("http://localhost/{marker}");
    e.request_body = Some(marker.to_string());
    db.history_insert(&e, 1000).unwrap();
    let mut keep = entry(&p.id, 2);
    keep.response_body = Some("残すほう".into());
    db.history_insert(&keep, 1000).unwrap();

    let fts_rows = |db: &Database| -> i64 {
        db.conn()
            .query_row("SELECT COUNT(*) FROM history_fts", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(fts_rows(&db), 2);

    db.history_delete(&e.id).unwrap();
    assert!(db.history_get(&e.id).unwrap().is_none());
    assert_eq!(fts_rows(&db), 1);
    assert!(matches!(
        db.history_delete(&e.id).unwrap_err(),
        crate::StorageError::NotFound { .. }
    ));
    // 本体にも WAL にも、消した文字列は残っていない
    assert_no_marker(&path, marker);

    // すべて削除
    let marker2 = "もう一つの目印QWE7712";
    let mut e2 = entry(&p.id, 3);
    e2.response_body = Some(marker2.to_string());
    db.history_insert(&e2, 1000).unwrap();
    assert_eq!(db.history_clear(&p.id).unwrap(), 2);
    assert_eq!(db.history_count(&p.id).unwrap(), 0);
    assert_eq!(fts_rows(&db), 0);
    assert_no_marker(&path, marker2);
    assert_no_marker(&path, "残すほう");
}

fn assert_no_marker(db_path: &std::path::Path, marker: &str) {
    for suffix in ["", "-wal"] {
        let mut p = db_path.as_os_str().to_owned();
        p.push(suffix);
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let found = bytes.windows(marker.len()).any(|w| w == marker.as_bytes());
        assert!(!found, "{} に {marker} が残っている", p.to_string_lossy());
    }
}

#[test]
fn deleting_a_project_removes_history_search_rows_and_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let db = Database::open(&path).unwrap();
    let p = db.project_create("p").unwrap();
    let marker = "プロジェクト削除の目印LKJ5503";
    let mut e = entry(&p.id, 1);
    e.response_body = Some(marker.into());
    db.history_insert(&e, 1000).unwrap();
    db.session_save(
        &p.id,
        &SessionState {
            tabs: vec![],
            active_tab_id: Some(marker.into()),
            saved_at: None,
        },
    )
    .unwrap();
    db.project_delete(&p.id).unwrap();
    let count = |sql: &str| -> i64 { db.conn().query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count("SELECT COUNT(*) FROM history"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM history_fts"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM sessions"), 0);
    assert_no_marker(&path, marker);
}

#[test]
fn ten_thousand_entries_list_and_search_quickly() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("p").unwrap();
    {
        let started = std::time::Instant::now();
        for n in 0..10_000 {
            let mut e = entry(&p.id, n);
            // 長めのボディ（一覧は読まない）
            e.response_body = Some(format!("{{\"n\":{n},\"pad\":\"{}\"}}", "あ".repeat(300)));
            db.history_insert(&e, 10_000).unwrap();
        }
        eprintln!("10,000 件の保存: {:?}", started.elapsed());
    }
    assert_eq!(db.history_count(&p.id).unwrap(), 10_000);

    let t = std::time::Instant::now();
    let rows = db.history_list(&p.id, &filter()).unwrap();
    let first_page = t.elapsed();
    assert_eq!(rows.items.len(), 100);
    assert_eq!(rows.total, 10_000);
    assert!(rows.next_cursor.is_some());

    let t = std::time::Instant::now();
    let rows = db
        .history_list(
            &p.id,
            &HistoryFilter {
                text: Some("q=9999".into()),
                limit: 100,
                ..HistoryFilter::default()
            },
        )
        .unwrap();
    let search = t.elapsed();
    assert_eq!(rows.items.len(), 1);

    let t = std::time::Instant::now();
    let rows = db
        .history_list(
            &p.id,
            &HistoryFilter {
                text: Some("あ".into()), // 3 文字未満: LIKE（全件を見る）
                limit: 100,
                ..HistoryFilter::default()
            },
        )
        .unwrap();
    let like = t.elapsed();
    assert_eq!(rows.total, 10_000);

    let t = std::time::Instant::now();
    let rows = db
        .history_list(
            &p.id,
            &HistoryFilter {
                statuses: vec![StatusClass::S2xx],
                environment_id: Some("env1".into()),
                limit: 100,
                cursor: db.history_list(&p.id, &filter()).unwrap().next_cursor,
                ..HistoryFilter::default()
            },
        )
        .unwrap();
    let filtered = t.elapsed();
    assert_eq!(rows.items.len(), 100);
    eprintln!(
        "10,000 件: 1 ページ目 {first_page:?} / 全文検索 {search:?} / LIKE {like:?} / 絞り込み+2 ページ目 {filtered:?}"
    );
    // デバッグビルドでも十分に速い（余裕を持たせた上限）
    assert!(first_page.as_millis() < 300, "{first_page:?}");
    assert!(search.as_millis() < 300, "{search:?}");
    assert!(like.as_millis() < 1500, "{like:?}");
    assert!(filtered.as_millis() < 300, "{filtered:?}");
}

#[test]
fn sessions_round_trip_and_follow_the_project() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("p").unwrap();
    assert_eq!(db.session_load(&p.id).unwrap(), None);
    let state = SessionState {
        tabs: vec![SessionTab {
            id: "t1".into(),
            kind: SessionTabKind::Operation,
            op_key: Some(OpKey::Id("getClientList".into())),
            adhoc_id: None,
            title: "取引先一覧取得".into(),
            request: None,
            dirty: false,
            view_state: Some("{\"panel\":\"params\"}".into()),
            dropped_fields: vec![],
        }],
        active_tab_id: Some("t1".into()),
        saved_at: None,
    };
    let saved = db.session_save(&p.id, &state).unwrap();
    assert!(saved.saved_at.is_some());
    assert_eq!(db.session_load(&p.id).unwrap().unwrap(), saved);
    // 置き換え
    let empty = db.session_save(&p.id, &SessionState::default()).unwrap();
    assert_eq!(db.session_load(&p.id).unwrap().unwrap(), empty);
    // 存在しないプロジェクト
    assert!(db.session_save("none", &state).is_err());
    // 壊れた JSON は復元しない（起動を止めない）
    db.conn()
        .execute("UPDATE sessions SET json = '{broken'", [])
        .unwrap();
    assert_eq!(db.session_load(&p.id).unwrap(), None);
}

#[test]
fn secure_delete_is_on_for_history_search() {
    let db = Database::open_in_memory().unwrap();
    let on: i64 = db
        .conn()
        .query_row("PRAGMA secure_delete", [], |r| r.get(0))
        .unwrap();
    assert_eq!(on, 1);
    // FTS5 の secure-delete（設定は表に残る）
    let v: Option<i64> = db
        .conn()
        .query_row(
            "SELECT v FROM history_fts_config WHERE k = 'secure-delete'",
            [],
            |r| r.get(0),
        )
        .ok();
    assert_eq!(v, Some(1));
}
