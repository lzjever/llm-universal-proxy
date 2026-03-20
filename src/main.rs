//! LLM Universal Proxy entrypoint.

fn parse_config_path(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    let _program = args.next();
    let mut config_path = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --config".to_string())?;
                config_path = Some(value);
            }
            "--help" | "-h" => {
                return Err("usage: llm-universal-proxy --config <config.yaml>".to_string());
            }
            other => {
                return Err(format!("unknown argument `{}`", other));
            }
        }
    }

    config_path.ok_or_else(|| "missing required --config <config.yaml>".to_string())
}

#[tokio::main]
async fn main() {
    let config_path = match parse_config_path(std::env::args()) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{}", message);
            std::process::exit(2);
        }
    };

    if let Err(e) = llm_universal_proxy::run_with_config_path(config_path).await {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_config_path;

    #[test]
    fn parse_config_path_accepts_long_flag() {
        let path = parse_config_path(
            vec![
                "llm-universal-proxy".to_string(),
                "--config".to_string(),
                "proxy.yaml".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(path, "proxy.yaml");
    }

    #[test]
    fn parse_config_path_accepts_short_flag() {
        let path = parse_config_path(
            vec![
                "llm-universal-proxy".to_string(),
                "-c".to_string(),
                "proxy.yaml".to_string(),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(path, "proxy.yaml");
    }

    #[test]
    fn parse_config_path_requires_value() {
        let err = parse_config_path(
            vec!["llm-universal-proxy".to_string(), "--config".to_string()].into_iter(),
        )
        .unwrap_err();
        assert!(err.contains("missing value"));
    }

    #[test]
    fn parse_config_path_requires_flag() {
        let err =
            parse_config_path(vec!["llm-universal-proxy".to_string()].into_iter()).unwrap_err();
        assert!(err.contains("missing required --config"));
    }
}
