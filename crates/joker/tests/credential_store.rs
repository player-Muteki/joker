use joker::CredentialStore;

#[test]
fn new_store_is_empty() {
    let store = CredentialStore::new();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn set_and_get() {
    let mut store = CredentialStore::new();
    store.set("openai", "sk-openai-123".to_string());
    assert_eq!(store.get("openai"), Some("sk-openai-123".to_string()));
}

#[test]
fn get_missing_key_returns_none() {
    let store = CredentialStore::new();
    assert_eq!(store.get("nonexistent"), None);
}

#[test]
fn set_overwrites_existing_key() {
    let mut store = CredentialStore::new();
    store.set("deepseek", "sk-v1".to_string());
    store.set("deepseek", "sk-v2".to_string());
    assert_eq!(store.get("deepseek"), Some("sk-v2".to_string()));
    assert_eq!(store.len(), 1);
}

#[test]
fn remove_deletes_key() {
    let mut store = CredentialStore::new();
    store.set("anthropic", "sk-ant-abc".to_string());
    assert!(store.has("anthropic"));
    store.remove("anthropic");
    assert!(!store.has("anthropic"));
    assert_eq!(store.get("anthropic"), None);
    assert!(store.is_empty());
}

#[test]
fn remove_missing_key_is_noop() {
    let mut store = CredentialStore::new();
    store.remove("ghost");
    assert!(store.is_empty());
}

#[test]
fn len_and_is_empty_reflect_contents() {
    let mut store = CredentialStore::new();
    assert_eq!(store.len(), 0);
    assert!(store.is_empty());

    store.set("a", "1".to_string());
    assert_eq!(store.len(), 1);
    assert!(!store.is_empty());

    store.set("b", "2".to_string());
    assert_eq!(store.len(), 2);
    assert!(!store.is_empty());

    store.remove("a");
    assert_eq!(store.len(), 1);
    assert!(!store.is_empty());

    store.remove("b");
    assert_eq!(store.len(), 0);
    assert!(store.is_empty());
}

#[test]
fn list_returns_all_keys_sorted() {
    let mut store = CredentialStore::new();
    store.set("z", "zzz".to_string());
    store.set("a", "aaa".to_string());
    store.set("m", "mmm".to_string());

    let keys = store.list();
    assert_eq!(keys, vec!["a", "m", "z"]);
}

#[test]
fn list_is_empty_when_no_credentials() {
    let store = CredentialStore::new();
    assert!(store.list().is_empty());
}
