//! 環境・変数・選んでいる環境の結合テスト。公開 API（`Database`）と、ファイルの DB を直接開く SQL だけを使う。
//! 既存の単体テスト（crates/storage/src/tests_m2.rs）と重ならない観点を中心にする。

use std::path::Path;
use std::sync::Arc;

use nekoreq_core::{EnvVar, Environment, VarValue, GLOBAL_ENVIRONMENT_NAME};
use nekoreq_storage::{Database, StorageError, LATEST_VERSION};

fn plain(name: &str, value: &str) -> EnvVar {
    EnvVar {
        name: name.into(),
        value: VarValue::Plain(value.into()),
        description: String::new(),
        enabled: true,
    }
}

fn secret(name: &str) -> EnvVar {
    EnvVar {
        name: name.into(),
        value: VarValue::Secret,
        description: String::new(),
        enabled: true,
    }
}

fn with_vars(mut env: Environment, vars: Vec<EnvVar>) -> Environment {
    env.vars = vars;
    env
}

fn raw(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

fn is_invalid(e: &StorageError) -> bool {
    matches!(e, StorageError::Invalid(_) | StorageError::InvalidName(_))
}

// ---- グローバル環境は 1 つだけ ----

#[test]
fn exactly_one_global_environment_survives_creation_duplication_deletion_and_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let mut project_ids = Vec::new();
    {
        let db = Database::open(&path).unwrap();
        for i in 0..3 {
            let p = db.project_create(&format!("P{i}")).unwrap();
            let a = db.env_create(&p.id, "a").unwrap();
            let b = db.env_create(&p.id, "b").unwrap();
            let c = db.env_duplicate(&a.id).unwrap();
            db.env_delete(&b.id).unwrap();
            db.env_delete(&c.id).unwrap();
            project_ids.push(p.id);
        }
    }
    // マイグレーション（起動時）を何度走らせても増えない
    for _ in 0..3 {
        let db = Database::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
        for id in &project_ids {
            let envs = db.env_list(id).unwrap();
            assert_eq!(envs.iter().filter(|e| e.is_global).count(), 1, "{envs:?}");
            assert!(
                envs[0].is_global && envs[0].name == GLOBAL_ENVIRONMENT_NAME,
                "グローバルが先頭"
            );
            assert_eq!(db.env_global(id).unwrap().id, envs[0].id);
        }
    }
    let conn = raw(&path);
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM environments WHERE is_global = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 3, "プロジェクトごとに 1 つ");
}

#[test]
fn the_database_itself_refuses_a_second_global_environment_for_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let pid = {
        let db = Database::open(&path).unwrap();
        db.project_create("P").unwrap().id
    };
    let conn = raw(&path);
    let err = conn
        .execute(
            "INSERT INTO environments (id, project_id, name, sort_order, created_at, updated_at, is_global)
             VALUES ('x', ?1, 'もう 1 つ', 9, 't', 't', 1)",
            [&pid],
        )
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("unique"), "{err}");
    // 別のプロジェクトにはあってよい
    let db = Database::open(&path).unwrap();
    let p2 = db.project_create("P2").unwrap();
    assert!(db.env_global(&p2.id).unwrap().is_global);
}

#[test]
fn a_normal_environment_cannot_be_turned_global_or_a_global_turned_normal_through_save() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let normal = db.env_create(&p.id, "dev").unwrap();
    let mut spoof = normal.clone();
    spoof.is_global = true;
    spoof.project_id = "other".into();
    let saved = db.env_save(&spoof).unwrap();
    assert!(!saved.is_global);
    assert_eq!(saved.project_id, p.id);

    let g = db.env_global(&p.id).unwrap();
    let mut spoof = g.clone();
    spoof.is_global = false;
    let saved = db.env_save(&spoof).unwrap();
    assert!(saved.is_global);
    assert_eq!(
        db.env_list(&p.id)
            .unwrap()
            .iter()
            .filter(|e| e.is_global)
            .count(),
        1
    );
}

#[test]
fn global_can_hold_variables_but_keeps_its_name_even_with_surrounding_spaces() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let g = db.env_global(&p.id).unwrap();
    let mut edited = with_vars(
        g.clone(),
        vec![plain("baseUrl", "http://g"), secret("token")],
    );
    edited.name = format!("  {GLOBAL_ENVIRONMENT_NAME} ");
    let saved = db.env_save(&edited).unwrap();
    assert_eq!(saved.name, GLOBAL_ENVIRONMENT_NAME);
    assert_eq!(saved.vars.len(), 2);
    edited.name = "別名".into();
    assert!(is_invalid(&db.env_save(&edited).unwrap_err()));
    assert_eq!(db.env_global(&p.id).unwrap().name, GLOBAL_ENVIRONMENT_NAME);
}

#[test]
fn a_normal_environment_cannot_be_named_like_the_global_one() {
    // 画面で紛らわしいので、「グローバル」は通常の環境の名前に付けられない（作成も改名も）
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    for name in [GLOBAL_ENVIRONMENT_NAME, "  グローバル  "] {
        assert!(
            matches!(
                db.env_create(&p.id, name).unwrap_err(),
                StorageError::InvalidName(_)
            ),
            "{name:?}"
        );
    }
    let e = db.env_create(&p.id, "dev").unwrap();
    let mut renamed = e.clone();
    renamed.name = GLOBAL_ENVIRONMENT_NAME.into();
    assert!(matches!(
        db.env_save(&renamed).unwrap_err(),
        StorageError::InvalidName(_)
    ));
    assert_eq!(
        db.env_list(&p.id).unwrap().len(),
        2,
        "グローバルと dev だけ"
    );
    // 似た名前はよい
    db.env_create(&p.id, "グローバル2").unwrap();
}

// ---- 選んでいる環境 ----

fn selected(db: &Database, pid: &str) -> Option<String> {
    db.project_get(pid).unwrap().unwrap().default_environment_id
}

#[test]
fn deleting_the_selected_environment_selects_the_first_remaining_by_creation_order() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let a = db.env_create(&p.id, "a").unwrap();
    let b = db.env_create(&p.id, "b").unwrap();
    let c = db.env_create(&p.id, "c").unwrap();
    assert_eq!(
        selected(&db, &p.id),
        Some(a.id.clone()),
        "最初に作った環境が選ばれる"
    );

    db.env_select(&p.id, Some(&b.id)).unwrap();
    db.env_delete(&b.id).unwrap();
    assert_eq!(selected(&db, &p.id), Some(a.id.clone()), "残りの最初（a）");

    db.env_select(&p.id, Some(&c.id)).unwrap();
    db.env_delete(&a.id).unwrap();
    assert_eq!(
        selected(&db, &p.id),
        Some(c.id.clone()),
        "選んでいない環境（a）を消しても選択は変わらない"
    );

    db.env_delete(&c.id).unwrap();
    assert_eq!(
        selected(&db, &p.id),
        None,
        "グローバルだけが残っても選ばない"
    );
    assert!(db.env_global(&p.id).is_ok());
}

#[test]
fn deleting_a_non_selected_environment_keeps_the_selection_and_other_projects_are_unaffected() {
    let db = Database::open_in_memory().unwrap();
    let p1 = db.project_create("P1").unwrap();
    let p2 = db.project_create("P2").unwrap();
    let a1 = db.env_create(&p1.id, "a").unwrap();
    let b1 = db.env_create(&p1.id, "b").unwrap();
    let a2 = db.env_create(&p2.id, "a").unwrap();
    db.env_delete(&b1.id).unwrap();
    assert_eq!(selected(&db, &p1.id), Some(a1.id));
    assert_eq!(selected(&db, &p2.id), Some(a2.id.clone()));
    db.env_delete(&a2.id).unwrap();
    assert_eq!(selected(&db, &p2.id), None);
    assert!(selected(&db, &p1.id).is_some());
}

#[test]
fn after_everything_is_deleted_the_next_created_environment_is_selected_again() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let a = db.env_create(&p.id, "a").unwrap();
    db.env_delete(&a.id).unwrap();
    assert_eq!(selected(&db, &p.id), None);
    let b = db.env_create(&p.id, "b").unwrap();
    assert_eq!(selected(&db, &p.id), Some(b.id));
}

#[test]
fn an_explicit_deselect_is_overridden_by_the_next_created_environment() {
    // 「未選択」にしても、次に環境を作ると勝手に選ばれる（設計: 最初に作った環境が自動で選ばれる。
    // 「最初」ではない 2 つ目でも選択が NULL なら選ばれる）
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    db.env_create(&p.id, "a").unwrap();
    db.env_select(&p.id, None).unwrap();
    assert_eq!(selected(&db, &p.id), None);
    let b = db.env_create(&p.id, "b").unwrap();
    assert_eq!(selected(&db, &p.id), Some(b.id));
}

#[test]
fn selecting_an_environment_of_another_project_or_an_unknown_one_fails_and_changes_nothing() {
    let db = Database::open_in_memory().unwrap();
    let p1 = db.project_create("P1").unwrap();
    let p2 = db.project_create("P2").unwrap();
    let a1 = db.env_create(&p1.id, "a").unwrap();
    let a2 = db.env_create(&p2.id, "a").unwrap();
    assert!(matches!(
        db.env_select(&p1.id, Some(&a2.id)),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.env_select(&p1.id, Some("nope")),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.env_select("nope", Some(&a1.id)),
        Err(StorageError::NotFound { .. })
    ));
    assert_eq!(selected(&db, &p1.id), Some(a1.id));
}

// ---- 複製 ----

#[test]
fn duplicate_keeps_vars_in_order_with_flags_and_descriptions_and_appends_at_the_end() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let a = db.env_create(&p.id, "検証").unwrap();
    let mut vars = vec![
        plain("z", "1"),
        plain("a", "2"),
        secret("pw"),
        plain("off", "3"),
    ];
    vars[1].description = "説明".into();
    vars[3].enabled = false;
    db.env_save(&with_vars(a.clone(), vars.clone())).unwrap();
    let b = db.env_create(&p.id, "b").unwrap();

    let copy = db.env_duplicate(&a.id).unwrap();
    assert_eq!(copy.name, "検証 のコピー");
    assert_ne!(copy.id, a.id);
    assert!(!copy.is_global);
    assert_eq!(copy.vars, vars);
    let order: Vec<_> = db
        .env_list(&p.id)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(
        order,
        [GLOBAL_ENVIRONMENT_NAME, "検証", "b", "検証 のコピー"]
    );
    assert_eq!(selected(&db, &p.id), Some(a.id.clone()), "複製は選ばない");
    // コピーを編集しても元は変わらない
    db.env_save(&with_vars(copy.clone(), vec![plain("only", "copy")]))
        .unwrap();
    assert_eq!(db.env_get(&a.id).unwrap().unwrap().vars, vars);
    let _ = b;
}

#[test]
fn duplicate_of_a_long_multibyte_name_stays_within_the_name_limit() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let long = "あ".repeat(100);
    let a = db.env_create(&p.id, &long).unwrap();
    let c1 = db.env_duplicate(&a.id).unwrap();
    assert!(
        c1.name.chars().count() <= 100,
        "{}",
        c1.name.chars().count()
    );
    assert!(c1.name.ends_with(" のコピー"));
    // 複製の複製もエラーにならない
    let c2 = db.env_duplicate(&c1.id).unwrap();
    assert!(c2.name.chars().count() <= 100);
    // 保存し直せる（上限を超える名前を作っていない）
    db.env_save(&c2).unwrap();
}

#[test]
fn duplicating_an_unknown_environment_is_not_found() {
    let db = Database::open_in_memory().unwrap();
    assert!(matches!(
        db.env_duplicate("nope"),
        Err(StorageError::NotFound { .. })
    ));
}

// ---- 変数名の検証 ----

#[test]
fn variable_name_validation_table() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let e = db.env_create(&p.id, "e").unwrap();
    let ok = [
        "a",
        "baseUrl",
        "base_url",
        "base-url",
        "a.b",
        "日本語",
        "トークン1",
        "a:b",
        "a/b",
        "a=b",
        "x$y",
        "A",
        "_",
        "1",
        "a%b",
        "a'b",
        "a\"b",
    ];
    for name in ok {
        let saved = db.env_save(&with_vars(e.clone(), vec![plain(name, "v")]));
        assert!(
            saved.is_ok(),
            "{name:?} は使える名前のはず: {:?}",
            saved.err()
        );
        assert_eq!(saved.unwrap().vars[0].name, name);
    }
    let long_ok = "あ".repeat(100);
    assert!(
        db.env_save(&with_vars(e.clone(), vec![plain(&long_ok, "v")]))
            .is_ok(),
        "100 文字（マルチバイト）は通る"
    );
    let bad = [
        "",
        " ",
        "\t",
        "$uuid",
        "$x",
        "a b",
        "a\tb",
        "a\nb",
        "a\u{3000}b",
        "a\u{a0}b",
        "{a}",
        "a{",
        "a}",
        "{{a}}",
        &"a".repeat(101),
        &"あ".repeat(101),
    ];
    for name in bad {
        let r = db.env_save(&with_vars(e.clone(), vec![plain(name, "v")]));
        assert!(
            r.as_ref().err().is_some_and(is_invalid),
            "{name:?} は拒否されるはず: {r:?}"
        );
    }
}

#[test]
fn names_are_trimmed_case_sensitive_and_duplicates_are_detected_after_trimming() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let e = db.env_create(&p.id, "e").unwrap();
    let saved = db
        .env_save(&with_vars(
            e.clone(),
            vec![plain("  Token  ", "1"), plain("token", "2")],
        ))
        .unwrap();
    assert_eq!(
        saved
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>(),
        ["Token", "token"]
    );

    let before = db.env_get(&e.id).unwrap().unwrap();
    let r = db.env_save(&with_vars(
        e.clone(),
        vec![plain("x", "1"), plain(" x", "2")],
    ));
    assert!(r.as_ref().err().is_some_and(is_invalid), "{r:?}");
    assert_eq!(
        db.env_get(&e.id).unwrap().unwrap().vars,
        before.vars,
        "失敗したら何も変わらない"
    );

    // 通常の変数とシークレットでも同じ名前は重複
    let r = db.env_save(&with_vars(e.clone(), vec![plain("x", "1"), secret("x")]));
    assert!(r.as_ref().err().is_some_and(is_invalid), "{r:?}");
}

#[test]
fn a_name_valid_for_storage_is_always_a_variable_name_core_can_expand() {
    // storage が通す名前は `{{名前}}` として core が読める（trim 後に同じ名前になる）
    use nekoreq_core::{expand, DynamicSource, VarLayer, VarScope};
    struct NoDyn;
    impl DynamicSource for NoDyn {
        fn eval(&self, _: &str) -> Option<String> {
            None
        }
    }
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let e = db.env_create(&p.id, "e").unwrap();
    for name in [
        "a",
        "a.b",
        "日本語",
        "a:b",
        "a/b",
        "x$y",
        "a%b",
        "a'b",
        "a\"b",
        "a=b",
        "a,b",
    ] {
        let saved = db
            .env_save(&with_vars(e.clone(), vec![plain(name, "VAL")]))
            .unwrap();
        let mut l = VarLayer::new();
        l.insert(saved.vars[0].name.clone(), "VAL", false);
        let mut s = VarScope::new();
        s.push_layer(l);
        let out = expand(&format!("{{{{{name}}}}}"), &s, &NoDyn);
        assert_eq!(out.text, "VAL", "{name:?}");
    }
}

// ---- 値の往復 ----

#[test]
fn plain_values_round_trip_exactly() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let e = db.env_create(&p.id, "e").unwrap();
    let big = "x".repeat(1_000_000);
    let values = [
        "",
        " ",
        "  leading and trailing  ",
        "line1\nline2\r\n",
        "'; DROP TABLE env_vars; --",
        "{{other}}",
        "日本語🐱",
        "a\u{0}b",
        "\"quoted\"",
        "\\backslash",
        &big,
    ];
    let vars: Vec<EnvVar> = values
        .iter()
        .enumerate()
        .map(|(i, v)| plain(&format!("v{i}"), v))
        .collect();
    db.env_save(&with_vars(e.clone(), vars.clone())).unwrap();
    let got = db.env_get(&e.id).unwrap().unwrap().vars;
    assert_eq!(got.len(), vars.len());
    for (want, have) in vars.iter().zip(&got) {
        assert_eq!(want.value, have.value, "{}", want.name);
    }
}

#[test]
fn a_secret_is_stored_without_a_value_and_converting_to_plain_and_back_keeps_no_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let db = Database::open(&path).unwrap();
    let p = db.project_create("P").unwrap();
    let e = db.env_create(&p.id, "e").unwrap();
    db.env_save(&with_vars(e.clone(), vec![secret("pw")]))
        .unwrap();
    db.env_save(&with_vars(e.clone(), vec![plain("pw", "now-plain")]))
        .unwrap();
    let saved = db
        .env_save(&with_vars(e.clone(), vec![secret("pw")]))
        .unwrap();
    assert_eq!(saved.vars[0].value, VarValue::Secret);
    let conn = raw(&path);
    let v: Option<String> = conn
        .query_row("SELECT value FROM env_vars WHERE name = 'pw'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(v, None, "平文だった値は消えている");
}

fn all_bytes_in_dir(dir: &Path) -> Vec<(String, Vec<u8>)> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn a_plain_value_turned_into_a_secret_does_not_remain_in_the_sqlite_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let needle = "ONCE-PLAIN-VALUE-hunter2-xyz";
    {
        let db = Database::open(&path).unwrap();
        let p = db.project_create("P").unwrap();
        let e = db.env_create(&p.id, "e").unwrap();
        db.env_save(&with_vars(e.clone(), vec![plain("pw", needle)]))
            .unwrap();
        // 画面で「シークレットにする」と、同じ変数が secret になる
        db.env_save(&with_vars(e, vec![secret("pw")])).unwrap();
        // 開いたまま（アプリの稼働中）の状態でファイルを調べる
        for (name, bytes) in all_bytes_in_dir(dir.path()) {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{name} に残っている"
            );
        }
    }
}

// ---- プロジェクトの削除 ----

#[test]
fn deleting_a_project_removes_its_environments_and_variables_but_not_other_projects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let db = Database::open(&path).unwrap();
    let p1 = db.project_create("P1").unwrap();
    let p2 = db.project_create("P2").unwrap();
    let e1 = db.env_create(&p1.id, "e1").unwrap();
    let e2 = db.env_create(&p2.id, "e2").unwrap();
    db.env_save(&with_vars(e1.clone(), vec![plain("a", "1"), secret("s")]))
        .unwrap();
    db.env_save(&with_vars(e2.clone(), vec![plain("b", "2")]))
        .unwrap();
    let g1 = db.env_global(&p1.id).unwrap();
    db.env_save(&with_vars(g1.clone(), vec![plain("g", "1")]))
        .unwrap();

    db.project_delete(&p1.id).unwrap();
    assert!(db.env_get(&e1.id).unwrap().is_none());
    assert!(db.env_get(&g1.id).unwrap().is_none());
    assert!(matches!(
        db.env_list(&p1.id),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.env_global(&p1.id),
        Err(StorageError::NotFound { .. })
    ));
    assert_eq!(db.env_get(&e2.id).unwrap().unwrap().vars.len(), 1);

    let conn = raw(&path);
    let orphans: i64 = conn
        .query_row("SELECT count(*) FROM env_vars WHERE environment_id NOT IN (SELECT id FROM environments)", [], |r| r.get(0))
        .unwrap();
    assert_eq!(orphans, 0);
    let n: i64 = conn
        .query_row("SELECT count(*) FROM environments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2, "P2 のグローバルと e2");
}

#[test]
fn unknown_ids_are_not_found_for_every_environment_operation() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    assert!(matches!(
        db.env_create("nope", "x"),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.env_delete("nope"),
        Err(StorageError::NotFound { .. })
    ));
    let ghost = Environment {
        id: "nope".into(),
        project_id: p.id.clone(),
        name: "x".into(),
        is_global: false,
        vars: vec![],
        created_at: String::new(),
        updated_at: String::new(),
    };
    assert!(matches!(
        db.env_save(&ghost),
        Err(StorageError::NotFound { .. })
    ));
    assert!(db.env_get("nope").unwrap().is_none());
}

#[test]
fn environment_names_are_trimmed_and_limited() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    assert_eq!(db.env_create(&p.id, "  dev  ").unwrap().name, "dev");
    for bad in ["", "   ", &"a".repeat(101), &"あ".repeat(101)] {
        assert!(db.env_create(&p.id, bad).is_err(), "{bad:?}");
    }
    assert!(db.env_create(&p.id, &"あ".repeat(100)).is_ok());
    // 同じ名前の環境は作れる（ID が別）
    let a = db.env_create(&p.id, "same").unwrap();
    let b = db.env_create(&p.id, "same").unwrap();
    assert_ne!(a.id, b.id);
}

// ---- 並行アクセス ----

#[test]
fn concurrent_environment_writes_from_many_threads_do_not_deadlock_or_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("data.sqlite")).unwrap());
    let p = db.project_create("P").unwrap();
    let mut handles = Vec::new();
    for t in 0..8 {
        let db = db.clone();
        let pid = p.id.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..10 {
                let e = db.env_create(&pid, &format!("t{t}-{i}")).unwrap();
                db.env_save(&with_vars(
                    e.clone(),
                    vec![plain("n", &format!("{t}-{i}")), secret("s")],
                ))
                .unwrap();
                if i % 3 == 0 {
                    db.env_duplicate(&e.id).unwrap();
                }
                if i % 4 == 0 {
                    db.env_delete(&e.id).unwrap();
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let envs = db.env_list(&p.id).unwrap();
    assert_eq!(envs.iter().filter(|e| e.is_global).count(), 1);
    let sel = selected(&db, &p.id);
    assert!(
        sel.is_some_and(|id| envs.iter().any(|e| e.id == id)),
        "選んでいる環境は一覧にある"
    );
    let ids: std::collections::HashSet<_> = envs.iter().map(|e| e.id.clone()).collect();
    assert_eq!(ids.len(), envs.len());
}

#[test]
fn a_thousand_variables_keep_their_order() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let e = db.env_create(&p.id, "e").unwrap();
    let vars: Vec<EnvVar> = (0..1000)
        .map(|i| plain(&format!("v{:04}", (i * 7919) % 1000), &i.to_string()))
        .collect();
    let saved = db.env_save(&with_vars(e, vars.clone())).unwrap();
    assert_eq!(saved.vars, vars);
}
