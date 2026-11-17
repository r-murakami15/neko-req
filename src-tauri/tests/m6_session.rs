//! タブの復元（`session_save` / `session_load`）。往復・シークレットを保存しないこと・50 タブの時間。

use std::sync::Arc;
use std::time::Instant;

use nekoreq_core::{
    ApiKeyIn, AuthOverride, Body, KeyValue, OpKey, ParamIn, RequestSpec, SessionState, SessionTab,
    SessionTabKind,
};
use nekoreq_lib::error::AppError;
use nekoreq_lib::state::AppState;
use nekoreq_secrets::MemorySecretStore;

fn state() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let s =
        AppState::open_with_secrets(&dir.path().join("data"), Arc::new(MemorySecretStore::new()))
            .unwrap();
    (dir, s)
}

fn kv(location: ParamIn, name: &str, value: &str) -> KeyValue {
    KeyValue {
        location,
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}

fn spec(n: usize) -> RequestSpec {
    RequestSpec {
        method: "POST".into(),
        url: format!("{{{{baseUrl}}}}/api/v1/video/clients/{n}"),
        params: vec![
            kv(ParamIn::Query, "q", &format!("検索{n}")),
            kv(ParamIn::Header, "X-Trace", "abc"),
        ],
        body: Body::Json(format!(
            r#"{{"clientName":"取引先{n}","note":"{}"}}"#,
            "あ".repeat(500)
        )),
        auth: Some(AuthOverride::Bearer {
            token: "{{token}}".into(),
        }),
    }
}

fn tab(id: &str, kind: SessionTabKind, request: Option<RequestSpec>) -> SessionTab {
    SessionTab {
        id: id.into(),
        kind,
        op_key: (kind == SessionTabKind::Operation).then(|| OpKey::Id("getClientList".into())),
        adhoc_id: None,
        title: format!("タブ {id}"),
        dirty: request.is_some(),
        request,
        view_state: Some(r#"{"panel":"headers","scroll":120}"#.into()),
        dropped_fields: vec![],
    }
}

#[test]
fn tabs_round_trip_per_project() {
    let (_d, s) = state();
    let a = s.project_create("a").unwrap();
    let b = s.project_create("b").unwrap();
    assert_eq!(s.session_load(&a.id).unwrap(), None);

    let state_a = SessionState {
        tabs: vec![
            tab("t1", SessionTabKind::Operation, Some(spec(1))),
            tab("t2", SessionTabKind::Adhoc, None),
            tab("t3", SessionTabKind::New, Some(spec(3))),
        ],
        active_tab_id: Some("t2".into()),
        saved_at: None,
    };
    let saved = s.session_save(&a.id, &state_a).unwrap();
    assert!(saved.saved_at.is_some());
    let loaded = s.session_load(&a.id).unwrap().unwrap();
    assert_eq!(loaded, saved);
    assert_eq!(loaded.tabs.len(), 3);
    assert_eq!(loaded.active_tab_id.as_deref(), Some("t2"));
    // 未保存の入力・画面の状態がそのまま戻る（`{{変数}}` の認証も残る）
    assert_eq!(loaded.tabs[0].request, Some(spec(1)));
    assert_eq!(
        loaded.tabs[0].view_state.as_deref(),
        Some(r#"{"panel":"headers","scroll":120}"#)
    );
    assert_eq!(
        loaded.tabs[0].op_key,
        Some(OpKey::Id("getClientList".into()))
    );
    assert!(loaded.tabs[0].dropped_fields.is_empty());
    // 別のプロジェクトとは別
    assert_eq!(s.session_load(&b.id).unwrap(), None);
    // 置き換え（全部閉じた状態）
    s.session_save(&a.id, &SessionState::default()).unwrap();
    assert!(s.session_load(&a.id).unwrap().unwrap().tabs.is_empty());
    // プロジェクトを消せば一緒に消える
    s.session_save(&b.id, &state_a).unwrap();
    s.project_delete(&b.id).unwrap();
    assert!(s.session_load(&b.id).unwrap().is_none());
    assert!(matches!(
        s.session_save("none", &state_a).unwrap_err(),
        AppError::NotFound(_) | AppError::Storage(_)
    ));
}

#[test]
fn hand_written_secrets_are_not_saved_but_variables_are() {
    let (dir, s) = state();
    let p = s.project_create("p").unwrap();
    let mut r = spec(1);
    r.auth = Some(AuthOverride::ApiKey {
        location: ApiKeyIn::Header,
        name: "X-Api-Key".into(),
        value: "TAB-API-KEY-1".into(),
    });
    r.params = vec![
        kv(ParamIn::Header, "Authorization", "Bearer TAB-HEADER-2"),
        kv(
            ParamIn::Header,
            "Cookie",
            "SESSION=TAB-COOKIE-3; theme={{theme}}",
        ),
        kv(ParamIn::Header, "X-XSRF-TOKEN", "TAB-XSRF-4"),
        kv(ParamIn::Cookie, "SESSION", "TAB-COOKIE-5"),
        kv(ParamIn::Header, "X-Trace", "keep"),
        kv(ParamIn::Header, "X-Ref", "{{token}}"),
    ];
    let mut t = tab("t1", SessionTabKind::New, Some(r));
    t.title = "タイトル".into();
    let saved = s
        .session_save(
            &p.id,
            &SessionState {
                tabs: vec![
                    t,
                    tab(
                        "t2",
                        SessionTabKind::Operation,
                        Some({
                            let mut r = spec(2);
                            r.auth = Some(AuthOverride::Basic {
                                username: "neko".into(),
                                password: "TAB-BASIC-PW-6".into(),
                            });
                            r
                        }),
                    ),
                ],
                active_tab_id: Some("t1".into()),
                saved_at: None,
            },
        )
        .unwrap();

    let loaded = s.session_load(&p.id).unwrap().unwrap();
    assert_eq!(loaded, saved);
    let req = loaded.tabs[0].request.as_ref().unwrap();
    // 手で書いた値は空。変数で参照したものと、ほかのヘッダーは残る
    assert_eq!(
        req.auth,
        Some(AuthOverride::ApiKey {
            location: ApiKeyIn::Header,
            name: "X-Api-Key".into(),
            value: String::new()
        })
    );
    let v = |i: usize| req.params[i].value.as_str();
    assert_eq!(v(0), "");
    assert_eq!(v(1), "theme={{theme}}");
    assert_eq!(v(2), "");
    assert_eq!(v(3), "");
    assert_eq!(v(4), "keep");
    assert_eq!(v(5), "{{token}}");
    // 何を空にしたかが残る
    let dropped = &loaded.tabs[0].dropped_fields;
    assert!(
        dropped.iter().any(|d| d.contains("API キー")),
        "{dropped:?}"
    );
    assert!(dropped.contains(&"Authorization ヘッダー".to_string()));
    assert!(dropped.contains(&"Cookie ヘッダー".to_string()));
    assert!(dropped.contains(&"X-XSRF-TOKEN ヘッダー".to_string()));
    assert_eq!(
        loaded.tabs[1].request.as_ref().unwrap().auth,
        Some(AuthOverride::Basic {
            username: "neko".into(),
            password: String::new()
        })
    );
    assert_eq!(
        loaded.tabs[1].dropped_fields,
        vec!["認証（Basic のパスワード）"]
    );

    // SQLite のファイルにも残らない
    fn all_bytes(dir: &std::path::Path) -> Vec<u8> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(all_bytes(&p));
            } else {
                out.extend(std::fs::read(&p).unwrap());
            }
        }
        out
    }
    let bytes = all_bytes(dir.path());
    for secret in [
        "TAB-API-KEY-1",
        "TAB-HEADER-2",
        "TAB-COOKIE-3",
        "TAB-XSRF-4",
        "TAB-COOKIE-5",
        "TAB-BASIC-PW-6",
    ] {
        assert!(
            !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "{secret} がファイルに残っている"
        );
    }
}

#[test]
fn too_many_tabs_are_refused() {
    let (_d, s) = state();
    let p = s.project_create("p").unwrap();
    let tabs: Vec<SessionTab> = (0..201)
        .map(|i| tab(&i.to_string(), SessionTabKind::New, None))
        .collect();
    let e = s
        .session_save(
            &p.id,
            &SessionState {
                tabs,
                ..SessionState::default()
            },
        )
        .unwrap_err();
    assert!(matches!(e, AppError::Validation(_)));
}

#[test]
fn fifty_tabs_save_and_load_well_within_the_startup_budget() {
    let (_d, s) = state();
    let p = s.project_create("p").unwrap();
    // 50 タブ。半分は 1 KB 超の未保存の入力、1 つは 1 MB の本文
    let mut tabs: Vec<SessionTab> = (0..50)
        .map(|i| {
            if i % 2 == 0 {
                tab(&format!("t{i}"), SessionTabKind::Operation, Some(spec(i)))
            } else {
                tab(&format!("t{i}"), SessionTabKind::Adhoc, None)
            }
        })
        .collect();
    let mut big = spec(1);
    big.body = Body::Json("x".repeat(1024 * 1024));
    tabs[0] = tab("t0", SessionTabKind::New, Some(big));
    let state = SessionState {
        tabs,
        active_tab_id: Some("t49".into()),
        saved_at: None,
    };

    let t = Instant::now();
    s.session_save(&p.id, &state).unwrap();
    let save = t.elapsed();
    // 起動のときの読み込みは、開き直した DB から（コンテナの SQLite は同じプロセス）
    let t = Instant::now();
    let loaded = s.session_load(&p.id).unwrap().unwrap();
    let load = t.elapsed();
    assert_eq!(loaded.tabs.len(), 50);
    assert_eq!(loaded.active_tab_id.as_deref(), Some("t49"));
    eprintln!("50 タブ（うち 1 MB 本文 1 つ）: 保存 {save:?} / 読み込み {load:?}");
    // 設計書「起動 2 秒以内に前回のタブを復元」。デバッグビルドでもその 1/10 以内
    assert!(load.as_millis() < 200, "{load:?}");
    assert!(save.as_millis() < 500, "{save:?}");
}
