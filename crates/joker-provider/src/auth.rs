//! Credential resolution chain.
//!
//! [`resolve_auth`] unifies the previously dual-track auth paths (config-time
//! env resolution vs. runtime credential store) into one priority chain:
//! in-memory value > credential store > environment variable > no credentials.

use joker::CredentialStore;

use crate::protocol::{Auth, CredentialSource};

/// Resolve an [`Auth`] bundle into concrete credentials.
///
/// Priority: [`CredentialSource::Value`] (CLI flag, config, or TUI input) >
/// credential store (consulted with the route's provider id and the
/// configured env var name as fallbacks) > environment variable > no
/// credentials. When nothing resolves, the original
/// [`CredentialSource::EnvVar`] is preserved so callers can report the
/// expected variable name in error messages.
#[must_use]
pub fn resolve_auth(provider_id: &str, auth: &Auth, store: Option<&CredentialStore>) -> Auth {
    resolve_auth_with_env(provider_id, auth, store, |name| std::env::var(name).ok())
}

/// Testable core of [`resolve_auth`] with an injectable environment lookup.
fn resolve_auth_with_env(
    provider_id: &str,
    auth: &Auth,
    store: Option<&CredentialStore>,
    get_env: impl Fn(&str) -> Option<String>,
) -> Auth {
    match &auth.credentials {
        CredentialSource::Value(_) => auth.clone(),
        CredentialSource::EnvVar(env_name) => {
            let key = match store {
                // The store chain checks the explicit env name and the
                // inferred `{ID}_API_KEY` when no credential is stored.
                Some(store) => store.get_with_env_using(provider_id, Some(env_name), &get_env),
                None => get_env(env_name),
            };
            let credentials = match key {
                Some(value) => CredentialSource::Value(value),
                None => CredentialSource::EnvVar(env_name.clone()),
            };
            Auth {
                scheme: auth.scheme.clone(),
                credentials,
            }
        }
        CredentialSource::None => auth.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AuthScheme;

    /// Fake environment lookup backed by a static map.
    fn fake_env<'a>(values: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    fn bearer(credentials: CredentialSource) -> Auth {
        Auth {
            scheme: AuthScheme::Bearer,
            credentials,
        }
    }

    #[test]
    fn value_wins_over_store_and_env() {
        let mut store = CredentialStore::new();
        store.set("deepseek", "sk-stored".into());
        let resolved = resolve_auth_with_env(
            "deepseek",
            &bearer(CredentialSource::Value("sk-cli".into())),
            Some(&store),
            fake_env(&[("DEEPSEEK_API_KEY", "sk-env")]),
        );
        assert_eq!(
            resolved.credentials,
            CredentialSource::Value("sk-cli".into())
        );
    }

    #[test]
    fn store_wins_over_env() {
        let mut store = CredentialStore::new();
        store.set("deepseek", "sk-stored".into());
        let resolved = resolve_auth_with_env(
            "deepseek",
            &bearer(CredentialSource::EnvVar("DEEPSEEK_API_KEY".into())),
            Some(&store),
            fake_env(&[("DEEPSEEK_API_KEY", "sk-env")]),
        );
        assert_eq!(
            resolved.credentials,
            CredentialSource::Value("sk-stored".into())
        );
    }

    #[test]
    fn env_used_when_store_misses() {
        let store = CredentialStore::new();
        let resolved = resolve_auth_with_env(
            "deepseek",
            &bearer(CredentialSource::EnvVar("DEEPSEEK_API_KEY".into())),
            Some(&store),
            fake_env(&[("DEEPSEEK_API_KEY", "sk-env")]),
        );
        assert_eq!(
            resolved.credentials,
            CredentialSource::Value("sk-env".into())
        );
    }

    #[test]
    fn explicit_env_name_beats_inferred() {
        let store = CredentialStore::new();
        let resolved = resolve_auth_with_env(
            "myllm",
            &bearer(CredentialSource::EnvVar("MY_CUSTOM_KEY".into())),
            Some(&store),
            fake_env(&[
                ("MY_CUSTOM_KEY", "sk-explicit"),
                ("MYLLM_API_KEY", "sk-inferred"),
            ]),
        );
        assert_eq!(
            resolved.credentials,
            CredentialSource::Value("sk-explicit".into())
        );
    }

    #[test]
    fn unresolvable_keeps_env_name_for_reporting() {
        let store = CredentialStore::new();
        let resolved = resolve_auth_with_env(
            "deepseek",
            &bearer(CredentialSource::EnvVar("DEEPSEEK_API_KEY".into())),
            Some(&store),
            fake_env(&[]),
        );
        assert_eq!(
            resolved.credentials,
            CredentialSource::EnvVar("DEEPSEEK_API_KEY".into())
        );
    }

    #[test]
    fn no_store_uses_env_directly() {
        let resolved = resolve_auth_with_env(
            "deepseek",
            &bearer(CredentialSource::EnvVar("DEEPSEEK_API_KEY".into())),
            None,
            fake_env(&[("DEEPSEEK_API_KEY", "sk-env")]),
        );
        assert_eq!(
            resolved.credentials,
            CredentialSource::Value("sk-env".into())
        );
    }

    #[test]
    fn none_credentials_pass_through() {
        let resolved = resolve_auth_with_env(
            "deepseek",
            &Auth::none(),
            Some(&CredentialStore::new()),
            fake_env(&[]),
        );
        assert_eq!(resolved.credentials, CredentialSource::None);
    }
}
