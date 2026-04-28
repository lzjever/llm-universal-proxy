//! LLM Universal Proxy entrypoint.

#[derive(Debug, PartialEq, Eq)]
struct CliArgs {
    config_path: Option<String>,
    admin_bootstrap: bool,
    dashboard: bool,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<CliArgs, String> {
    let _program = args.next();
    let mut config_path = None;
    let mut admin_bootstrap = false;
    let mut dashboard = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --config".to_string())?;
                config_path = Some(value);
            }
            "--admin-bootstrap" => {
                admin_bootstrap = true;
            }
            "--dashboard" => {
                dashboard = true;
            }
            "--help" | "-h" => {
                return Err("usage: llm-universal-proxy (--config <config.yaml> | --admin-bootstrap) [--dashboard]".to_string());
            }
            other => {
                return Err(format!("unknown argument `{other}`"));
            }
        }
    }

    if config_path.is_some() && admin_bootstrap {
        return Err("--config and --admin-bootstrap are mutually exclusive".to_string());
    }
    if config_path.is_none() && !admin_bootstrap {
        return Err("missing required --config <config.yaml> or --admin-bootstrap".to_string());
    }

    Ok(CliArgs {
        config_path,
        admin_bootstrap,
        dashboard,
    })
}

fn validate_admin_bootstrap_env() -> Result<(), String> {
    match std::env::var("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN") {
        Ok(token) if token.trim().is_empty() => Err(
            "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN must not be empty when --admin-bootstrap is used"
                .to_string(),
        ),
        Ok(_) => Ok(()),
        Err(std::env::VarError::NotPresent) => Err(
            "LLM_UNIVERSAL_PROXY_ADMIN_TOKEN is required when --admin-bootstrap is used"
                .to_string(),
        ),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN must be valid UTF-8".to_string())
        }
    }
}

#[tokio::main]
async fn main() {
    let args = match parse_args(std::env::args()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let result = if args.admin_bootstrap {
        if let Err(message) = validate_admin_bootstrap_env() {
            eprintln!("{message}");
            std::process::exit(2);
        }
        if args.dashboard {
            llm_universal_proxy::run_with_config_and_dashboard(
                llm_universal_proxy::Config::default(),
            )
            .await
        } else {
            llm_universal_proxy::run_with_config(llm_universal_proxy::Config::default()).await
        }
    } else {
        let config_path = args
            .config_path
            .expect("config_path is required unless bootstrap");
        if args.dashboard {
            llm_universal_proxy::run_with_config_path_and_dashboard(config_path).await
        } else {
            llm_universal_proxy::run_with_config_path(config_path).await
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::{parse_args, validate_admin_bootstrap_env, CliArgs};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn parse_args_accepts_long_flag() {
        let args = parse_args(
            vec![
                "llm-universal-proxy".to_string(),
                "--config".to_string(),
                "proxy.yaml".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(
            args,
            CliArgs {
                config_path: Some("proxy.yaml".to_string()),
                admin_bootstrap: false,
                dashboard: false,
            }
        );
    }

    #[test]
    fn parse_args_accepts_short_flag() {
        let args = parse_args(
            vec![
                "llm-universal-proxy".to_string(),
                "-c".to_string(),
                "proxy.yaml".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(args.config_path.as_deref(), Some("proxy.yaml"));
    }

    #[test]
    fn parse_args_accepts_dashboard_flag() {
        let args = parse_args(
            vec![
                "llm-universal-proxy".to_string(),
                "--config".to_string(),
                "proxy.yaml".to_string(),
                "--dashboard".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert!(args.dashboard);
        assert_eq!(args.config_path.as_deref(), Some("proxy.yaml"));
        assert!(!args.admin_bootstrap);
    }

    #[test]
    fn parse_args_accepts_admin_bootstrap_flag() {
        let args = parse_args(
            vec![
                "llm-universal-proxy".to_string(),
                "--admin-bootstrap".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert!(args.admin_bootstrap);
        assert_eq!(args.config_path, None);
    }

    #[test]
    fn parse_args_rejects_config_with_admin_bootstrap() {
        let err = parse_args(
            vec![
                "llm-universal-proxy".to_string(),
                "--config".to_string(),
                "proxy.yaml".to_string(),
                "--admin-bootstrap".to_string(),
            ]
            .into_iter(),
        )
        .unwrap_err();
        assert!(err.contains("--config and --admin-bootstrap are mutually exclusive"));
    }

    #[test]
    fn admin_bootstrap_requires_non_empty_admin_token() {
        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _admin_token = EnvGuard::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");

        let missing = validate_admin_bootstrap_env().unwrap_err();
        assert!(missing.contains("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN is required"));

        let _admin_token = EnvGuard::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", " ");
        let empty = validate_admin_bootstrap_env().unwrap_err();
        assert!(empty.contains("must not be empty"));
    }

    #[test]
    fn admin_bootstrap_accepts_non_empty_admin_token() {
        let _lock = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let _admin_token = EnvGuard::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "admin-secret");

        validate_admin_bootstrap_env().unwrap();
    }

    #[test]
    fn parse_args_requires_value() {
        let err =
            parse_args(vec!["llm-universal-proxy".to_string(), "--config".to_string()].into_iter())
                .unwrap_err();
        assert!(err.contains("missing value"));
    }

    #[test]
    fn parse_args_requires_flag() {
        let err = parse_args(vec!["llm-universal-proxy".to_string()].into_iter()).unwrap_err();
        assert!(err.contains("missing required --config"));
    }
}
