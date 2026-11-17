use rusqlite::Connection;

use super::*;

fn temp_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    (dir, path)
}

#[test]
fn migration_runs_on_a_fresh_file_and_creates_all_tables() {
    let (_dir, path) = temp_db_path();
    let db = Database::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
    drop(db);

    let conn = Connection::open(&path).unwrap();
    for table in [
        "projects",
        "api_sources",
        "operations",
        "request_drafts",
        "adhoc_requests",
        "environments",
        "env_vars",
        "history",
        "history_fts",
    ] {
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "テーブル {table} がない");
    }
}

#[test]
fn opening_twice_does_not_break_anything() {
    let (_dir, path) = temp_db_path();
    let first = Database::open(&path).unwrap();
    let project = first.project_create("A").unwrap();
    drop(first);

    let second = Database::open(&path).unwrap();
    let third = Database::open(&path).unwrap(); // 同時に 2 本開いてもよい
    assert_eq!(second.schema_version().unwrap(), LATEST_VERSION);
    assert_eq!(third.project_list().unwrap(), vec![project]);
}

#[test]
fn user_version_is_latest() {
    let (_dir, path) = temp_db_path();
    drop(Database::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, LATEST_VERSION);
    assert_eq!(v, 4);
}

#[test]
fn database_newer_than_the_app_is_refused() {
    let (_dir, path) = temp_db_path();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 999;").unwrap();
    }
    match Database::open(&path) {
        Err(StorageError::SchemaTooNew { found: 999, .. }) => {}
        other => panic!("SchemaTooNew になるはず: {other:?}"),
    }
}

#[test]
fn project_survives_closing_and_reopening() {
    let (_dir, path) = temp_db_path();
    let created = {
        let db = Database::open(&path).unwrap();
        db.project_create("日本アニメーション ライセンス契約管理システム API")
            .unwrap()
    }; // ここで接続を閉じる

    let db = Database::open(&path).unwrap();
    let list = db.project_list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0], created);
    assert_eq!(db.project_get(&created.id).unwrap(), Some(created));
}

#[test]
fn project_create_list_get_rename_delete() {
    let db = Database::open_in_memory().unwrap();
    assert!(db.project_list().unwrap().is_empty());

    let a = db.project_create("  A  ").unwrap();
    let b = db.project_create("B").unwrap();
    assert_eq!(a.name, "A", "前後の空白は除く");
    assert!(a.source.is_none());
    assert_eq!(
        db.project_list()
            .unwrap()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );

    let renamed = db.project_rename(&a.id, "A2").unwrap();
    assert_eq!(renamed.name, "A2");
    assert_eq!(renamed.created_at, a.created_at);
    assert_eq!(db.project_get(&a.id).unwrap().unwrap().name, "A2");

    db.project_delete(&a.id).unwrap();
    assert_eq!(db.project_get(&a.id).unwrap(), None);
    assert_eq!(db.project_list().unwrap(), vec![b]);
}

#[test]
fn invalid_names_and_unknown_ids_are_errors() {
    let db = Database::open_in_memory().unwrap();
    assert!(matches!(
        db.project_create("   "),
        Err(StorageError::InvalidName(_))
    ));
    assert!(matches!(
        db.project_create(&"あ".repeat(101)),
        Err(StorageError::InvalidName(_))
    ));
    assert!(db.project_create(&"あ".repeat(100)).is_ok());
    assert!(matches!(
        db.project_rename("nope", "x"),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.project_delete("nope"),
        Err(StorageError::NotFound { .. })
    ));
}

#[test]
fn secret_env_var_cannot_hold_a_value() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let conn = db.conn();
    conn.execute(
        "INSERT INTO environments (id, project_id, name, created_at, updated_at) VALUES ('e', ?1, 'local', 't', 't')",
        [&p.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO env_vars (environment_id, name, kind, value) VALUES ('e', 'baseUrl', 'plain', 'http://localhost')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO env_vars (environment_id, name, kind, value) VALUES ('e', 'session', 'secret', NULL)",
        [],
    )
    .unwrap();
    let err = conn.execute(
        "INSERT INTO env_vars (environment_id, name, kind, value) VALUES ('e', 'password', 'secret', 'hunter2')",
        [],
    );
    assert!(err.is_err(), "シークレットの値は SQLite に入れられない");
}

fn insert_history(conn: &Connection, project_id: &str, id: &str, url: &str, body: &str) {
    conn.execute(
        "INSERT INTO history (id, project_id, sent_at, method, url, response_body)
         VALUES (?1, ?2, 't', 'GET', ?3, ?4)",
        rusqlite::params![id, project_id, url, body],
    )
    .unwrap();
}

fn fts_hits(conn: &Connection, q: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT h.id FROM history_fts f JOIN history h ON h.seq = f.rowid
             WHERE history_fts MATCH ?1 ORDER BY h.seq",
        )
        .unwrap();
    stmt.query_map([q], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn history_full_text_search_works_and_follows_deletes() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    {
        let conn = db.conn();
        insert_history(
            &conn,
            &p.id,
            "h1",
            "http://localhost:8080/api/v1/video/clients?q=Netflix",
            r#"{"clientName":"ちびまる子ちゃん"}"#,
        );
        insert_history(
            &conn,
            &p.id,
            "h2",
            "http://localhost:8080/api/v1/auth/session",
            "{}",
        );

        assert_eq!(fts_hits(&conn, "\"video/clients\""), ["h1"]);
        assert_eq!(
            fts_hits(&conn, "\"まる子\""),
            ["h1"],
            "日本語も部分一致で探せる"
        );
        assert_eq!(fts_hits(&conn, "\"auth/session\""), ["h2"]);

        conn.execute("DELETE FROM history WHERE id = 'h1'", [])
            .unwrap();
        assert!(fts_hits(&conn, "\"まる子\"").is_empty());
    }

    // プロジェクトを消すと（外部キーの連鎖で）履歴と検索用の行も消える
    db.project_delete(&p.id).unwrap();
    let conn = db.conn();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM history_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn deleting_a_project_removes_its_children() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    {
        let conn = db.conn();
        conn.execute(
            "INSERT INTO api_sources (id, project_id, kind, entry, files_json, openapi_version)
             VALUES ('s', ?1, 'path', 'openapi.yaml', '[{\"path\":\"openapi.yaml\",\"modifiedAt\":null,\"hash\":\"ab\"}]', '3.0.3')",
            [&p.id],
        )
        .unwrap();
    }
    // 取り込み元は Project.source として読める
    let loaded = db.project_get(&p.id).unwrap().unwrap();
    let source = loaded.source.expect("source が読める");
    assert_eq!(source.entry, "openapi.yaml");
    assert_eq!(source.files.len(), 1);

    db.project_delete(&p.id).unwrap();
    let conn = db.conn();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM api_sources", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}
