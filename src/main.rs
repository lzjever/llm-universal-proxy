//! LLM Universal Proxy entrypoint.

#[derive(Debug, PartialEq, Eq)]
struct CliArgs {
    config_path: String,
    dashboard: bool,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<CliArgs, String> {
    let _program = args.next();
    let mut config_path = None;
    let mut dashboard = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --config".to_string())?;
                config_path = Some(value);
            }
            "--dashboard" => {
                dashboard = true;
            }
            "--help" | "-h" => {
                return Err(
                    "usage: llm-universal-proxy --config <config.yaml> [--dashboard]".to_string(),
                );
            }
            other => {
                return Err(format!("unknown argument `{}`", other));
            }
        }
    }

    Ok(CliArgs {
        config_path: config_path
            .ok_or_else(|| "missing required --config <config.yaml>".to_string())?,
        dashboard,
    })
}

#[tokio::main]
async fn main() {
    let args = match parse_args(std::env::args()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{}", message);
            std::process::exit(2);
        }
    };

    let result = if args.dashboard {
        llm_universal_proxy::run_with_config_path_and_dashboard(args.config_path).await
    } else {
        llm_universal_proxy::run_with_config_path(args.config_path).await
    };
    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_args, CliArgs};

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
                config_path: "proxy.yaml".to_string(),
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
        assert_eq!(args.config_path, "proxy.yaml");
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
