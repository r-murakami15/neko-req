//! シークレットの保存先（`MemorySecretStore`）の結合テスト。キーチェーン本体（`KeyringSecretStore`）は
//! コンテナでは使えないので対象外（実機で確かめる）。

use std::sync::Arc;

use nekoreq_secrets::{MemorySecretStore, SecretKey, SecretStore};

fn key(p: &str, e: &str, n: &str) -> SecretKey {
    SecretKey::new(p, e, n)
}

#[test]
fn unusual_values_round_trip_exactly() {
    let s = MemorySecretStore::new();
    let big = "x".repeat(1_000_000);
    for (i, v) in [
        "",
        " ",
        "  padded  ",
        "line1\nline2\r\n",
        "日本語🐱",
        "a\u{0}b",
        "{{other}}",
        "\"'\\",
        big.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let k = key("p", "e", &format!("n{i}"));
        s.set(&k, v).unwrap();
        assert_eq!(s.get(&k).unwrap().as_deref(), Some(*v), "{i}");
    }
}

#[test]
fn names_with_unusual_characters_are_independent_keys() {
    let s = MemorySecretStore::new();
    for (i, name) in ["a b", "日本語", "a/b", "a:b", "A", "a", "a "]
        .iter()
        .enumerate()
    {
        s.set(&key("p", "e", name), &i.to_string()).unwrap();
    }
    for (i, name) in ["a b", "日本語", "a/b", "a:b", "A", "a", "a "]
        .iter()
        .enumerate()
    {
        assert_eq!(
            s.get(&key("p", "e", name)).unwrap(),
            Some(i.to_string()),
            "{name:?}"
        );
    }
    assert_eq!(s.len(), 7);
}

#[test]
fn deleting_one_key_leaves_siblings_in_the_same_environment_and_project() {
    let s = MemorySecretStore::new();
    s.set(&key("p", "e", "a"), "1").unwrap();
    s.set(&key("p", "e", "b"), "2").unwrap();
    s.set(&key("p", "e2", "a"), "3").unwrap();
    s.delete(&key("p", "e", "a")).unwrap();
    assert_eq!(s.get(&key("p", "e", "a")).unwrap(), None);
    assert_eq!(s.get(&key("p", "e", "b")).unwrap().as_deref(), Some("2"));
    assert_eq!(s.get(&key("p", "e2", "a")).unwrap().as_deref(), Some("3"));
}

#[test]
fn many_threads_writing_and_reading_do_not_lose_values() {
    let s: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::new());
    let mut hs = Vec::new();
    for t in 0..8 {
        let s = s.clone();
        hs.push(std::thread::spawn(move || {
            for i in 0..200 {
                let k = key("p", &format!("e{t}"), &format!("n{i}"));
                s.set(&k, &format!("{t}-{i}")).unwrap();
                assert_eq!(s.get(&k).unwrap(), Some(format!("{t}-{i}")));
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    for t in 0..8 {
        for i in 0..200 {
            assert_eq!(
                s.get(&key("p", &format!("e{t}"), &format!("n{i}")))
                    .unwrap(),
                Some(format!("{t}-{i}"))
            );
        }
    }
}

#[test]
fn a_poisoned_lock_does_not_make_the_store_unusable() {
    let s = Arc::new(MemorySecretStore::new());
    let s2 = s.clone();
    let _ = std::thread::spawn(move || {
        s2.set(&key("p", "e", "a"), "1").unwrap();
        panic!("別のスレッドが落ちる");
    })
    .join();
    s.set(&key("p", "e", "b"), "2").unwrap();
    assert_eq!(s.get(&key("p", "e", "a")).unwrap().as_deref(), Some("1"));
}

#[test]
fn key_debug_has_ids_and_names_but_no_values() {
    let k = key("proj", "env", "password");
    let d = format!("{k:?}");
    assert!(d.contains("password") && !d.contains("VALUE"));
}

#[test]
fn memory_store_debug_does_not_print_values() {
    let s = MemorySecretStore::new();
    s.set(&key("p", "e", "password"), "TOP-SECRET-VALUE")
        .unwrap();
    assert!(!format!("{s:?}").contains("TOP-SECRET-VALUE"), "{s:?}");
}
