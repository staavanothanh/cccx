use std::path::PathBuf;

use anyhow::Result;
use anyhow::ensure;
use codex_secrets::SecretName;
use codex_secrets::SecretScope;
use codex_secrets::SecretsBackendKind;
use codex_secrets::SecretsManager;
use codex_model_provider_info::ModelProviderInfo;
use std::path::Path;

pub const CCCX_API_KEY: &str = "CCCX_API_KEY";

#[derive(Clone)]
pub struct CccxSecretsStore {
    manager: SecretsManager,
}

impl CccxSecretsStore {
    pub fn new(cccx_home: PathBuf) -> Self {
        Self {
            manager: SecretsManager::new(cccx_home, SecretsBackendKind::Local),
        }
    }

    pub fn from_manager(manager: SecretsManager) -> Self {
        Self { manager }
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        let name = cccx_secret_name(key)?;
        ensure!(!value.is_empty(), "CCCX secret value must not be empty");
        self.manager.set(&SecretScope::Global, &name, value)
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let name = cccx_secret_name(key)?;
        self.manager.get(&SecretScope::Global, &name)
    }

    pub fn delete(&self, key: &str) -> Result<bool> {
        let name = cccx_secret_name(key)?;
        self.manager.delete(&SecretScope::Global, &name)
    }
}

/// Applies the persisted CCCX key only to the 9router provider, and only when
/// the conventional environment variable is absent. This preserves explicit
/// process-level configuration while making the secure setup flow survive a
/// restart without writing a plaintext value to config.toml.
pub fn inject_persisted_9router_key(
    provider: &mut ModelProviderInfo,
    cccx_home: &Path,
) -> Result<bool> {
    let store = CccxSecretsStore::new(cccx_home.to_path_buf());
    inject_persisted_9router_key_from_store(provider, &store)
}

fn inject_persisted_9router_key_from_store(
    provider: &mut ModelProviderInfo,
    store: &CccxSecretsStore,
) -> Result<bool> {
    if !provider.is_9router()
        || provider
            .env_key
            .as_deref()
            .is_some_and(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()))
    {
        return Ok(false);
    }

    let Some(value) = store.get(CCCX_API_KEY)? else {
        return Ok(false);
    };

    // `ModelProviderInfo::api_key()` errors when env_key is set but absent.
    // The persisted key is intentionally represented as the in-memory bearer
    // token for this session rather than copied to the environment.
    provider.env_key = None;
    provider.experimental_bearer_token = Some(value);
    Ok(true)
}

fn cccx_secret_name(key: &str) -> Result<SecretName> {
    ensure!(
        key == CCCX_API_KEY,
        "unsupported CCCX secret name; only CCCX_API_KEY is allowed"
    );
    SecretName::new(CCCX_API_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    use codex_keyring_store::tests::MockKeyringStore;
    use codex_secrets::LocalSecretsNamespace;

    fn test_store(home: &tempfile::TempDir) -> CccxSecretsStore {
        let manager = SecretsManager::new_with_keyring_store_and_namespace(
            home.path().to_path_buf(),
            SecretsBackendKind::Local,
            Arc::new(MockKeyringStore::default()),
            LocalSecretsNamespace::ManagedSecrets,
        );
        CccxSecretsStore::from_manager(manager)
    }

    #[test]
    fn cccx_api_key_round_trips_through_secrets_manager() -> Result<()> {
        let home = tempfile::tempdir()?;
        let store = test_store(&home);

        store.set(CCCX_API_KEY, "router-key-1")?;

        assert_eq!(store.get(CCCX_API_KEY)?, Some("router-key-1".to_string()));
        assert!(store.delete(CCCX_API_KEY)?);
        assert_eq!(store.get(CCCX_API_KEY)?, None);
        Ok(())
    }

    #[test]
    fn rejects_non_cccx_api_key_names_without_leaking_value() {
        let home = tempfile::tempdir().expect("tempdir");
        let store = test_store(&home);

        let err = store
            .set("OPENAI_API_KEY", "secret-value-must-not-appear")
            .expect_err("unsupported key should fail");
        let rendered = err.to_string();

        assert!(rendered.contains("only CCCX_API_KEY is allowed"));
        assert!(!rendered.contains("OPENAI_API_KEY"));
        assert!(!rendered.contains("secret-value-must-not-appear"));
    }

    #[test]
    fn rejects_empty_value_without_persisting_plaintext() -> Result<()> {
        let home = tempfile::tempdir()?;
        let store = test_store(&home);

        let err = store
            .set(CCCX_API_KEY, "")
            .expect_err("empty API key should fail");
        assert!(err.to_string().contains("must not be empty"));
        assert_eq!(store.get(CCCX_API_KEY)?, None);
        Ok(())
    }

    #[test]
    fn stored_key_is_not_written_as_plaintext_under_cccx_home() -> Result<()> {
        let home = tempfile::tempdir()?;
        let store = test_store(&home);
        let raw_value = "router-key-never-plaintext";

        store.set(CCCX_API_KEY, raw_value)?;

        for entry in walkdir::WalkDir::new(home.path()) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let bytes = fs::read(entry.path())?;
            let contents = String::from_utf8_lossy(&bytes);
            assert!(
                !contents.contains(raw_value),
                "secret value was written as plaintext to {}",
                entry.path().display()
            );
        }
        Ok(())
    }

    #[test]
    fn injects_persisted_key_only_for_9router_without_an_environment_override() -> Result<()> {
        let home = tempfile::tempdir()?;
        let store = test_store(&home);
        store.set(CCCX_API_KEY, "stored-router-key")?;

        // The public constructor deliberately carries an env_key; model-provider
        // auth would otherwise fail before trying experimental_bearer_token.
        let mut provider = ModelProviderInfo::create_9router_provider();
        let inserted = inject_persisted_9router_key_from_store(&mut provider, &store)?;

        assert!(inserted);
        assert_eq!(provider.env_key, None);
        assert_eq!(provider.experimental_bearer_token.as_deref(), Some("stored-router-key"));
        Ok(())
    }

    #[test]
    fn never_injects_a_cccx_key_into_a_non_9router_provider() -> Result<()> {
        let home = tempfile::tempdir()?;
        let store = test_store(&home);
        store.set(CCCX_API_KEY, "stored-router-key")?;
        let mut provider = ModelProviderInfo::create_openai_provider(None);

        assert!(!inject_persisted_9router_key_from_store(&mut provider, &store)?);
        assert!(provider.experimental_bearer_token.is_none());
        Ok(())
    }
}
