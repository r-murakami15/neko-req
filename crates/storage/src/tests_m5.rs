//! M5 で足したもの（プロジェクトの設定・アプリの設定・確認済みホスト・自動更新用の変数）とマイグレーション v5 のテスト。

use nekoreq_core::{
    AppSettings, EnvVar, ProjectSettings, SourceFile, SourceKind, SpecMeta, VarValue,
};
use rusqlite::Connection;

use super::*;

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

#[test]
fn migration_v4_to_v5_keeps_projects_and_defaults_both_automations_on() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    {
        let conn = Connection::open(&path).unwrap();
        for sql in &crate::migrations::MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();
        conn.execute_batch(
            "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p1', 'P', 't', 't');",
        )
        .unwrap();
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
    let p = db.project_get("p1").unwrap().unwrap();
    assert_eq!(p.name, "P");
    assert_eq!(p.settings, ProjectSettings::default());
    assert!(p.settings.auto_session && p.settings.auto_csrf);
    assert_eq!(db.app_settings_get().unwrap(), AppSettings::default());
}

#[test]
fn project_settings_round_trip_and_new_projects_are_on() {
    let db = db();
    let p = db.project_create("P").unwrap();
    assert_eq!(p.settings, ProjectSettings::default());
    let saved = db
        .project_settings_set(
            &p.id,
            &ProjectSettings {
                auto_session: false,
                auto_csrf: true,
            },
        )
        .unwrap();
    assert!(!saved.settings.auto_session && saved.settings.auto_csrf);
    // 一覧・取得にも出る
    assert!(
        !db.project_get(&p.id)
            .unwrap()
            .unwrap()
            .settings
            .auto_session
    );
    assert!(!db.project_list().unwrap()[0].settings.auto_session);
    // 別のプロジェクトには影響しない
    let q = db.project_create("Q").unwrap();
    assert_eq!(q.settings, ProjectSettings::default());
    assert!(matches!(
        db.project_settings_set("nope", &ProjectSettings::default()),
        Err(StorageError::NotFound { .. })
    ));
    // 名前の変更で設定が消えない
    db.project_rename(&p.id, "P2").unwrap();
    assert!(
        !db.project_get(&p.id)
            .unwrap()
            .unwrap()
            .settings
            .auto_session
    );
}

#[test]
fn app_settings_round_trip() {
    let db = db();
    assert_eq!(db.app_settings_get().unwrap(), AppSettings::default());
    let mut s = AppSettings {
        editor_command: Some("code -g {file}:{line}".into()),
        ..AppSettings::default()
    };
    // M6 の項目も一緒に保存・復元される
    s.request.timeout_secs = 5;
    s.history.limit = 5000;
    s.network.ca_cert_path = Some("/etc/ca.pem".into());
    db.app_settings_set(&s).unwrap();
    assert_eq!(db.app_settings_get().unwrap(), s);
    db.app_settings_set(&AppSettings::default()).unwrap();
    assert_eq!(db.app_settings_get().unwrap(), AppSettings::default());
}

#[test]
fn allowed_hosts_can_be_updated() {
    let db = db();
    let p = db.project_create("P").unwrap();
    assert!(matches!(
        db.source_set_allowed_hosts(&p.id, &[]),
        Err(StorageError::NotFound { .. })
    ));
    let source = nekoreq_core::ApiSource {
        id: String::new(),
        kind: SourceKind::Url,
        entry: "https://example.com/openapi.yaml".into(),
        files: vec![SourceFile {
            path: "openapi.yaml".into(),
            modified_at: None,
            hash: "h".into(),
        }],
        openapi_version: "3.0.3".into(),
        synced_at: None,
        has_token: false,
        allowed_hosts: vec![],
        options: Default::default(),
        meta: SpecMeta::default(),
    };
    db.source_save(&p.id, &source).unwrap();
    db.source_set_allowed_hosts(&p.id, &["https://assets.example.com".into()])
        .unwrap();
    let got = db.project_get(&p.id).unwrap().unwrap().source.unwrap();
    assert_eq!(got.allowed_hosts, vec!["https://assets.example.com"]);
}

fn var<'a>(env: &'a nekoreq_core::Environment, name: &str) -> Option<&'a EnvVar> {
    env.vars.iter().find(|v| v.name == name)
}

#[test]
fn ensure_secret_creates_converts_and_leaves_existing_alone() {
    let db = db();
    let p = db.project_create("P").unwrap();
    let env = db.env_create(&p.id, "dev").unwrap();
    let mut edited = env.clone();
    edited.vars = vec![
        EnvVar {
            name: "baseUrl".into(),
            value: VarValue::Plain("http://x".into()),
            description: String::new(),
            enabled: true,
        },
        EnvVar {
            name: "session".into(),
            value: VarValue::Plain("PLAIN-SESSION-VALUE".into()),
            description: "手で入れた".into(),
            enabled: false,
        },
    ];
    db.env_save(&edited).unwrap();

    // 通常の変数 → シークレット（無効のままにして、説明も残す）
    assert!(db
        .env_var_ensure_secret(&env.id, "session", "自動")
        .unwrap());
    let got = db.env_get(&env.id).unwrap().unwrap();
    let s = var(&got, "session").unwrap();
    assert_eq!(s.value, VarValue::Secret);
    assert!(!s.enabled);
    assert_eq!(s.description, "手で入れた");
    // ほかの変数はそのまま
    assert_eq!(
        var(&got, "baseUrl").unwrap().value,
        VarValue::Plain("http://x".into())
    );

    // なければ末尾にシークレットとして作る（有効）
    assert!(db
        .env_var_ensure_secret(&env.id, "xsrfToken", "自動で更新されます")
        .unwrap());
    let got = db.env_get(&env.id).unwrap().unwrap();
    assert_eq!(got.vars.last().unwrap().name, "xsrfToken");
    let x = var(&got, "xsrfToken").unwrap();
    assert_eq!(x.value, VarValue::Secret);
    assert!(x.enabled);
    assert_eq!(x.description, "自動で更新されます");

    // すでにシークレットなら変えない
    assert!(!db
        .env_var_ensure_secret(&env.id, "xsrfToken", "別")
        .unwrap());
    assert_eq!(
        var(&db.env_get(&env.id).unwrap().unwrap(), "xsrfToken")
            .unwrap()
            .description,
        "自動で更新されます"
    );
    assert!(matches!(
        db.env_var_ensure_secret("nope", "x", ""),
        Err(StorageError::NotFound { .. })
    ));
}

#[test]
fn ensure_secret_does_not_leave_the_old_plain_value_in_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let db = Database::open(&path).unwrap();
    let p = db.project_create("P").unwrap();
    let env = db.env_create(&p.id, "dev").unwrap();
    let mut edited = env.clone();
    edited.vars = vec![EnvVar {
        name: "session".into(),
        value: VarValue::Plain("PLAIN-SESSION-VALUE-123".into()),
        description: String::new(),
        enabled: true,
    }];
    db.env_save(&edited).unwrap();
    db.env_var_ensure_secret(&env.id, "session", "").unwrap();
    drop(db);
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes
                .windows(b"PLAIN-SESSION-VALUE-123".len())
                .any(|w| w == b"PLAIN-SESSION-VALUE-123"),
            "平文の値がファイルに残っている"
        );
    }
}
