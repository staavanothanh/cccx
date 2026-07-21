use codex_model_provider_info::NINE_ROUTER_API_KEY_ENV;
use std::path::Path;
use tempfile::TempDir;
use wiremock::MockServer;

pub struct TestCodexExecBuilder {
    home: TempDir,
    cwd: TempDir,
}

impl TestCodexExecBuilder {
    pub fn cmd(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::new(
            codex_utils_cargo_bin::cargo_bin("codex-exec")
                .expect("should find binary for codex-exec"),
        );
        cmd.current_dir(self.cwd.path())
            // cccx resolves this ahead of the legacy CODEX_HOME alias, so every
            // child process must receive the isolated home explicitly.
            .env("CCCX_HOME", self.home.path())
            .env("CODEX_HOME", self.home.path())
            .env("CODEX_SQLITE_HOME", self.home.path())
            .env(NINE_ROUTER_API_KEY_ENV, "dummy");
        cmd
    }
    pub fn cmd_with_server(&self, server: &MockServer) -> assert_cmd::Command {
        // The production boundary accepts only providers identified as
        // 9router. Point such a provider at the per-test mock instead of
        // bypassing that boundary with the legacy OpenAI base-url override.
        let base = format!("{}/v1", server.uri());
        let config = format!(
            r#"model_provider = "9router-test"

[model_providers.9router-test]
name = "9router"
base_url = {}
env_key = "{}"
wire_api = "responses"
"#,
            toml_string_literal(&base),
            NINE_ROUTER_API_KEY_ENV,
        );
        std::fs::write(self.home.path().join("config.toml"), config)
            .expect("write isolated 9router test provider config");

        let mut cmd = self.cmd();
        cmd.arg("-c")
            .arg("model_provider=\"9router-test\"");
        cmd
    }

    pub fn cwd_path(&self) -> &Path {
        self.cwd.path()
    }
    pub fn home_path(&self) -> &Path {
        self.home.path()
    }
}

fn toml_string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("serialize TOML string literal")
}

pub fn test_codex_exec() -> TestCodexExecBuilder {
    TestCodexExecBuilder {
        home: TempDir::new().expect("create temp home"),
        cwd: TempDir::new().expect("create temp cwd"),
    }
}
