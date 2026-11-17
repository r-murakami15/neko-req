//! M2 で足したリポジトリ（環境・変数・AdHocRequest・RequestDraft）とマイグレーション v2 のテスト。

use nekoreq_core::{
    AdHocRequest, AuthOverride, Body, EnvVar, Environment, KeyValue, OpKey, ParamIn, RequestDraft,
    VarValue,
};
use rusqlite::Connection;

use super::*;

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
        description: "シークレット".into(),
        enabled: true,
    }
}

// ---- マイグレーション v1 → v2 ----

#[test]
fn migration_v1_to_v2_adds_global_environments_to_existing_projects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    {
        // v1 のデータベースを手で作る（プロジェクト 2 つ・通常の環境とシークレット変数つき）
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(crate::migrations::MIGRATIONS[0])
            .unwrap();
        conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        for id in ["p1", "p2"] {
            conn.execute(
                "INSERT INTO projects (id, name, created_at, updated_at) VALUES (?1, ?1, 't', 't')",
                [id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO environments (id, project_id, name, created_at, updated_at) VALUES ('e1', 'p1', 'ローカル', 't', 't')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO env_vars (environment_id, name, kind, value) VALUES ('e1', 'baseUrl', 'plain', 'http://localhost')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO env_vars (environment_id, name, kind, value) VALUES ('e1', 'session', 'secret', NULL)",
            [],
        )
        .unwrap();
    }

    let db = Database::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);

    for pid in ["p1", "p2"] {
        let g = db.env_global(pid).unwrap();
        assert!(g.is_global);
        assert_eq!(g.name, "グローバル");
        assert_eq!(g.project_id, pid);
        assert!(uuid_like(&g.id), "{}", g.id);
        assert!(g.created_at.ends_with('Z'), "{}", g.created_at);
    }
    // 2 つのプロジェクトの ID は別
    assert_ne!(
        db.env_global("p1").unwrap().id,
        db.env_global("p2").unwrap().id
    );

    // 既存の環境と変数はそのまま。変数は有効
    let envs = db.env_list("p1").unwrap();
    assert_eq!(envs.len(), 2);
    assert!(envs[0].is_global, "グローバルが先頭");
    assert_eq!(envs[1].id, "e1");
    assert!(!envs[1].is_global);
    assert_eq!(
        envs[1].vars,
        vec![
            plain("baseUrl", "http://localhost"),
            EnvVar {
                description: String::new(),
                ..secret("session")
            }
        ]
    );

    // 2 回目に開いても増えない
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.env_list("p1").unwrap().len(), 2);
}

fn uuid_like(s: &str) -> bool {
    let parts: Vec<_> = s.split('-').collect();
    parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && s.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
        && parts[2].starts_with('4')
}

#[test]
fn v2_keeps_the_rule_that_secrets_have_no_value() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let g = db.env_global(&p.id).unwrap();
    let conn = db.conn();
    let err = conn.execute(
        "INSERT INTO env_vars (environment_id, name, kind, value, enabled) VALUES (?1, 'pw', 'secret', 'hunter2', 1)",
        [&g.id],
    );
    assert!(err.is_err());
}

// ---- 環境 ----

#[test]
fn a_new_project_gets_one_global_environment() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let envs = db.env_list(&p.id).unwrap();
    assert_eq!(envs.len(), 1);
    assert!(envs[0].is_global);
    assert_eq!(envs[0].name, "グローバル");
    assert!(envs[0].vars.is_empty());
    assert_eq!(p.default_environment_id, None, "グローバルは選べない環境");

    // プロジェクトを消すと環境も変数も消える
    let mut g = envs[0].clone();
    g.vars = vec![plain("a", "1")];
    db.env_save(&g).unwrap();
    db.project_delete(&p.id).unwrap();
    let conn = db.conn();
    for table in ["environments", "env_vars"] {
        let n: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table}");
    }
}

#[test]
fn only_one_global_environment_per_project() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let conn = db.conn();
    let err = conn.execute(
        "INSERT INTO environments (id, project_id, name, created_at, updated_at, is_global) VALUES ('x', ?1, 'g2', 't', 't', 1)",
        [&p.id],
    );
    assert!(err.is_err());
}

#[test]
fn create_list_rename_and_first_environment_becomes_selected() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let a = db
        .env_create(&p.id, "  ローカル (Dev Container)  ")
        .unwrap();
    let b = db.env_create(&p.id, "検証").unwrap();
    assert_eq!(a.name, "ローカル (Dev Container)");
    assert!(!a.is_global);

    let names: Vec<_> = db
        .env_list(&p.id)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["グローバル", "ローカル (Dev Container)", "検証"]);

    // 最初に作った環境が「選んでいる環境」になる。2 つ目は変わらない
    let project = db.project_get(&p.id).unwrap().unwrap();
    assert_eq!(
        project.default_environment_id.as_deref(),
        Some(a.id.as_str())
    );

    let mut renamed = b.clone();
    renamed.name = "本番".into();
    let saved = db.env_save(&renamed).unwrap();
    assert_eq!(saved.name, "本番");
    assert_eq!(saved.created_at, b.created_at);
    assert!(saved.updated_at >= b.updated_at);
}

#[test]
fn environment_name_and_unknown_ids_are_validated() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    assert!(matches!(
        db.env_create(&p.id, "  "),
        Err(StorageError::InvalidName(_))
    ));
    assert!(matches!(
        db.env_create(&p.id, &"あ".repeat(101)),
        Err(StorageError::InvalidName(_))
    ));
    assert!(matches!(
        db.env_create("nope", "x"),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.env_list("nope"),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.env_delete("nope"),
        Err(StorageError::NotFound { .. })
    ));
    assert!(matches!(
        db.env_duplicate("nope"),
        Err(StorageError::NotFound { .. })
    ));
}

#[test]
fn save_replaces_vars_and_keeps_order_enabled_and_descriptions() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let mut env = db.env_create(&p.id, "ローカル").unwrap();
    env.vars = vec![
        plain("baseUrl", "http://localhost:8080"),
        EnvVar {
            description: "ログインで自動更新".into(),
            ..secret("session")
        },
        EnvVar {
            enabled: false,
            ..plain("email", "user@example.com")
        },
        plain("  clientId  ", "1"),
    ];
    let saved = db.env_save(&env).unwrap();
    let names: Vec<_> = saved.vars.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        names,
        ["baseUrl", "session", "email", "clientId"],
        "空白は除く"
    );
    assert_eq!(saved.vars[1].value, VarValue::Secret);
    assert_eq!(saved.vars[1].description, "ログインで自動更新");
    assert!(!saved.vars[2].enabled);

    // 変数を減らして保存し直すと、減ったぶんは消える
    let mut again = saved.clone();
    again.vars.truncate(1);
    again.vars[0].value = VarValue::Plain("http://localhost:9090".into());
    let saved = db.env_save(&again).unwrap();
    assert_eq!(saved.vars, again.vars);
    assert_eq!(db.env_get(&env.id).unwrap().unwrap().vars, again.vars);
}

#[test]
fn secret_values_never_reach_sqlite() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let mut env = db.env_create(&p.id, "E").unwrap();
    env.vars = vec![secret("password"), plain("email", "a@example.com")];
    db.env_save(&env).unwrap();
    let dup = db.env_duplicate(&env.id).unwrap();
    assert_eq!(dup.vars[0].value, VarValue::Secret);

    let conn = db.conn();
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM env_vars WHERE kind = 'secret' AND value IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM env_vars WHERE kind = 'secret'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2, "元と複製の 2 件");
}

#[test]
fn invalid_variable_names_are_rejected() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let env = db.env_create(&p.id, "E").unwrap();
    for bad in ["", "  ", "$uuid", "a b", "a{b", "a}", "x{{y}}"] {
        let mut e = env.clone();
        e.vars = vec![plain(bad, "v")];
        assert!(
            matches!(db.env_save(&e), Err(StorageError::Invalid(_))),
            "{bad:?}"
        );
    }
    let mut e = env.clone();
    e.vars = vec![plain("a", "1"), plain(" a ", "2")];
    assert!(matches!(db.env_save(&e), Err(StorageError::Invalid(m)) if m.contains("重複")));
    // 失敗したら中身は変わらない
    assert!(db.env_get(&env.id).unwrap().unwrap().vars.is_empty());
    // 日本語や記号の名前はよい
    let mut e = env.clone();
    e.vars = vec![plain("取引先-id.v2", "1")];
    assert!(db.env_save(&e).is_ok());
}

#[test]
fn duplicate_copies_vars_and_does_not_select_the_copy() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let mut env = db.env_create(&p.id, "ローカル").unwrap();
    env.vars = vec![
        plain("baseUrl", "http://localhost:8080"),
        secret("session"),
        EnvVar {
            enabled: false,
            ..plain("off", "x")
        },
    ];
    db.env_save(&env).unwrap();

    let copy = db.env_duplicate(&env.id).unwrap();
    assert_ne!(copy.id, env.id);
    assert_eq!(copy.name, "ローカル のコピー");
    assert_eq!(copy.vars, db.env_get(&env.id).unwrap().unwrap().vars);
    assert!(!copy.is_global);
    let project = db.project_get(&p.id).unwrap().unwrap();
    assert_eq!(
        project.default_environment_id.as_deref(),
        Some(env.id.as_str())
    );

    // 長い名前は 100 文字に収める
    let long = db.env_create(&p.id, &"長".repeat(100)).unwrap();
    let copy = db.env_duplicate(&long.id).unwrap();
    assert_eq!(copy.name.chars().count(), 100);
    assert!(copy.name.ends_with(" のコピー"));
}

#[test]
fn deleting_the_selected_environment_selects_the_next_one() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let a = db.env_create(&p.id, "A").unwrap();
    let b = db.env_create(&p.id, "B").unwrap();
    db.env_save(&Environment {
        vars: vec![plain("x", "1")],
        ..a.clone()
    })
    .unwrap();

    db.env_delete(&a.id).unwrap();
    assert!(db.env_get(&a.id).unwrap().is_none());
    let project = db.project_get(&p.id).unwrap().unwrap();
    assert_eq!(
        project.default_environment_id.as_deref(),
        Some(b.id.as_str())
    );
    // 変数も消えている
    let n: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM env_vars", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);

    db.env_delete(&b.id).unwrap();
    let project = db.project_get(&p.id).unwrap().unwrap();
    assert_eq!(project.default_environment_id, None);
    assert_eq!(db.env_list(&p.id).unwrap().len(), 1, "グローバルだけ残る");
}

#[test]
fn global_environment_cannot_be_deleted_duplicated_renamed_or_selected() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let g = db.env_global(&p.id).unwrap();
    assert!(matches!(
        db.env_delete(&g.id),
        Err(StorageError::Invalid(_))
    ));
    assert!(matches!(
        db.env_duplicate(&g.id),
        Err(StorageError::Invalid(_))
    ));
    assert!(matches!(
        db.env_select(&p.id, Some(&g.id)),
        Err(StorageError::Invalid(_))
    ));
    let mut renamed = g.clone();
    renamed.name = "別名".into();
    assert!(matches!(
        db.env_save(&renamed),
        Err(StorageError::Invalid(_))
    ));

    // 変数は保存できる
    let mut with_vars = g.clone();
    with_vars.vars = vec![plain("email", "a@example.com"), secret("password")];
    assert_eq!(db.env_save(&with_vars).unwrap().vars.len(), 2);
}

#[test]
fn select_environment_is_saved_per_project() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let other = db.project_create("Q").unwrap();
    let a = db.env_create(&p.id, "A").unwrap();
    let b = db.env_create(&p.id, "B").unwrap();
    let oe = db.env_create(&other.id, "O").unwrap();

    let updated = db.env_select(&p.id, Some(&b.id)).unwrap();
    assert_eq!(
        updated.default_environment_id.as_deref(),
        Some(b.id.as_str())
    );
    assert_eq!(
        db.project_get(&p.id)
            .unwrap()
            .unwrap()
            .default_environment_id
            .as_deref(),
        Some(b.id.as_str())
    );
    // 他のプロジェクトの環境は選べない
    assert!(matches!(
        db.env_select(&p.id, Some(&oe.id)),
        Err(StorageError::NotFound { .. })
    ));
    let cleared = db.env_select(&p.id, None).unwrap();
    assert_eq!(cleared.default_environment_id, None);
    assert_eq!(
        db.env_select(&p.id, Some(&a.id))
            .unwrap()
            .default_environment_id
            .as_deref(),
        Some(a.id.as_str())
    );
}

#[test]
fn selection_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let (pid, eid) = {
        let db = Database::open(&path).unwrap();
        let p = db.project_create("P").unwrap();
        let _a = db.env_create(&p.id, "A").unwrap();
        let b = db.env_create(&p.id, "B").unwrap();
        db.env_select(&p.id, Some(&b.id)).unwrap();
        (p.id, b.id)
    };
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.project_get(&pid)
            .unwrap()
            .unwrap()
            .default_environment_id,
        Some(eid)
    );
}

// ---- マイグレーション v2 → v3（AdHocRequest の説明） ----

#[test]
fn migration_v2_to_v3_adds_an_empty_description_to_existing_adhoc_requests() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(crate::migrations::MIGRATIONS[0])
            .unwrap();
        conn.execute_batch(crate::migrations::MIGRATIONS[1])
            .unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p1', 'p1', 't', 't')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO adhoc_requests (id, project_id, name, method, url, params_json, body_json, created_at, updated_at)
             VALUES ('a1', 'p1', '古いリクエスト', 'GET', 'http://h/', '[]', '{\"kind\":\"none\"}', 't', 't')",
            [],
        )
        .unwrap();
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
    let a = db.adhoc_get("a1").unwrap().unwrap();
    assert_eq!(a.name, "古いリクエスト");
    assert_eq!(a.description, "");
}

#[test]
fn adhoc_description_round_trips_and_defaults_to_empty_when_missing_in_json() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let mut req = adhoc(&p.id, "説明つき");
    req.description = "ログイン状態を確かめる\n2 行目".into();
    let saved = db.adhoc_save(&req).unwrap();
    assert_eq!(saved.description, "ログイン状態を確かめる\n2 行目");
    assert_eq!(db.adhoc_get(&saved.id).unwrap().unwrap(), saved);
    // 更新で書き換え・空にもできる
    let mut edit = saved.clone();
    edit.description = String::new();
    assert_eq!(db.adhoc_save(&edit).unwrap().description, "");
    // 画面から description のない JSON が来ても空として読める
    let mut v = serde_json::to_value(&req).unwrap();
    v.as_object_mut().unwrap().remove("description");
    let back: AdHocRequest = serde_json::from_value(v).unwrap();
    assert_eq!(back.description, "");
}

// ---- AdHocRequest ----

fn adhoc(project_id: &str, name: &str) -> AdHocRequest {
    AdHocRequest {
        id: String::new(),
        project_id: project_id.into(),
        name: name.into(),
        description: String::new(),
        method: "get".into(),
        url: "{{baseUrl}}/api/v1/auth/session".into(),
        params: vec![KeyValue {
            location: ParamIn::Header,
            name: "Accept".into(),
            value: "application/json".into(),
            enabled: true,
        }],
        body: Body::Json("{\"a\":\"{{x}}\"}".into()),
        auth: Some(AuthOverride::Bearer {
            token: "{{token}}".into(),
        }),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

#[test]
fn adhoc_create_update_list_delete() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let q = db.project_create("Q").unwrap();
    assert!(db.adhoc_list(&p.id).unwrap().is_empty());

    let a = db.adhoc_save(&adhoc(&p.id, "  セッション確認  ")).unwrap();
    assert!(!a.id.is_empty());
    assert_eq!(a.name, "セッション確認");
    assert_eq!(a.method, "GET", "method は大文字にそろえる");
    assert_eq!(a.body, Body::Json("{\"a\":\"{{x}}\"}".into()));
    assert_eq!(
        a.auth,
        Some(AuthOverride::Bearer {
            token: "{{token}}".into()
        })
    );
    assert_eq!(a.params.len(), 1);
    assert_eq!(a.created_at, a.updated_at);

    let b = db.adhoc_save(&adhoc(&p.id, "二つ目")).unwrap();
    db.adhoc_save(&adhoc(&q.id, "別のプロジェクト")).unwrap();
    let names: Vec<_> = db
        .adhoc_list(&p.id)
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["セッション確認", "二つ目"]);

    // 更新: 作成日時は保つ
    let mut edit = a.clone();
    edit.name = "改名".into();
    edit.url = "{{baseUrl}}/x".into();
    edit.body = Body::None;
    edit.auth = None;
    edit.params.clear();
    let updated = db.adhoc_save(&edit).unwrap();
    assert_eq!(updated.id, a.id);
    assert_eq!(updated.created_at, a.created_at);
    assert!(updated.updated_at >= a.updated_at);
    assert_eq!(updated.body, Body::None);
    assert_eq!(updated.auth, None);
    assert!(updated.params.is_empty());
    assert_eq!(db.adhoc_get(&a.id).unwrap().unwrap(), updated);
    assert_eq!(db.adhoc_list(&p.id).unwrap().len(), 2);

    db.adhoc_delete(&b.id).unwrap();
    assert!(db.adhoc_get(&b.id).unwrap().is_none());
    assert!(matches!(
        db.adhoc_delete(&b.id),
        Err(StorageError::NotFound { .. })
    ));
}

#[test]
fn adhoc_validates_and_cascades_with_the_project() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    assert!(matches!(
        db.adhoc_save(&adhoc(&p.id, "  ")),
        Err(StorageError::InvalidName(_))
    ));
    assert!(matches!(
        db.adhoc_save(&adhoc("nope", "x")),
        Err(StorageError::NotFound { .. })
    ));
    let mut no_method = adhoc(&p.id, "x");
    no_method.method = " ".into();
    assert!(matches!(
        db.adhoc_save(&no_method),
        Err(StorageError::Invalid(_))
    ));

    db.adhoc_save(&adhoc(&p.id, "x")).unwrap();
    db.project_delete(&p.id).unwrap();
    let n: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM adhoc_requests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn adhoc_body_variants_round_trip() {
    use nekoreq_core::{MultipartPart, MultipartValue};
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let kv = |n: &str| KeyValue {
        location: ParamIn::Query,
        name: n.into(),
        value: "v".into(),
        enabled: false,
    };
    for body in [
        Body::None,
        Body::Text("t".into()),
        Body::Form(vec![kv("a")]),
        Body::Multipart(vec![
            MultipartPart {
                name: "f".into(),
                value: MultipartValue::File("/tmp/x".into()),
                enabled: true,
            },
            MultipartPart {
                name: "t".into(),
                value: MultipartValue::Text("v".into()),
                enabled: false,
            },
        ]),
        Body::Binary("/tmp/y.bin".into()),
    ] {
        let mut r = adhoc(&p.id, "r");
        r.body = body.clone();
        let saved = db.adhoc_save(&r).unwrap();
        assert_eq!(db.adhoc_get(&saved.id).unwrap().unwrap().body, body);
    }
}

// ---- RequestDraft ----

fn draft(project_id: &str, key: OpKey) -> RequestDraft {
    RequestDraft {
        id: String::new(),
        project_id: project_id.into(),
        op_key: key,
        params: vec![KeyValue {
            location: ParamIn::Query,
            name: "q".into(),
            value: "Netflix".into(),
            enabled: true,
        }],
        body: Body::Json("{}".into()),
        auth: Some(AuthOverride::None),
        updated_at: String::new(),
    }
}

#[test]
fn draft_save_get_and_replace_by_op_key() {
    let db = Database::open_in_memory().unwrap();
    let p = db.project_create("P").unwrap();
    let id_key = OpKey::Id("getClientList".into());
    let mp_key = OpKey::method_path("get", "/api/v1/video/clients/{clientId}");

    assert_eq!(db.draft_get(&p.id, &id_key).unwrap(), None);

    let a = db.draft_save(&draft(&p.id, id_key.clone())).unwrap();
    assert!(!a.id.is_empty());
    assert_eq!(a.op_key, id_key);
    assert_eq!(a.auth, Some(AuthOverride::None));
    assert_eq!(db.draft_get(&p.id, &id_key).unwrap(), Some(a.clone()));

    // 同じ op_key への保存は置き換え（ID は保つ）
    let mut edit = draft(&p.id, id_key.clone());
    edit.params[0].value = "Amazon".into();
    edit.auth = None;
    let b = db.draft_save(&edit).unwrap();
    assert_eq!(b.id, a.id);
    assert_eq!(b.params[0].value, "Amazon");
    assert_eq!(b.auth, None);
    assert!(b.updated_at >= a.updated_at);

    // METHOD path のキーも別物として扱える
    let c = db.draft_save(&draft(&p.id, mp_key.clone())).unwrap();
    assert_ne!(c.id, a.id);
    assert_eq!(
        db.draft_get(&p.id, &mp_key).unwrap().unwrap().op_key,
        mp_key
    );
    // プロジェクトが違えば別
    let q = db.project_create("Q").unwrap();
    assert_eq!(db.draft_get(&q.id, &id_key).unwrap(), None);
}

#[test]
fn draft_requires_an_existing_project_and_follows_its_deletion() {
    let db = Database::open_in_memory().unwrap();
    assert!(matches!(
        db.draft_save(&draft("nope", OpKey::Id("x".into()))),
        Err(StorageError::NotFound { .. })
    ));
    let p = db.project_create("P").unwrap();
    db.draft_save(&draft(&p.id, OpKey::Id("x".into()))).unwrap();
    db.project_delete(&p.id).unwrap();
    let n: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM request_drafts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}
