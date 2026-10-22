//! One-shot compatibility handshake. Contracts describe behavior, not provenance.
use std::io::Write;
use std::process::ExitCode;

use serde::Serialize;

#[derive(Serialize)]
struct VersionInfo {
    schema_version: u8,
    version: &'static str,
    distribution: &'static str,
    git_commit: Option<&'static str>,
    contracts: [&'static str; 1],
}

fn information() -> VersionInfo {
    VersionInfo {
        schema_version: 1,
        version: env!("CARGO_PKG_VERSION"),
        distribution: "gitsense",
        // A release builder may embed a verified source revision. Do not
        // guess from the working directory at runtime or label dirty builds
        // with their parent commit. Ordinary local/archive builds use null.
        git_commit: option_env!("TXCRIPT_GIT_COMMIT").filter(|value| is_full_commit(value)),
        contracts: ["gitsense.sessions.v1"],
    }
}

fn is_full_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn render(format: &str) -> Result<String, String> {
    let info = information();
    let output = match format {
        "json" => serde_json::to_string_pretty(&info).map_err(|error| error.to_string())?,
        "human" => format!(
            "txcript {}\ndistribution: {}\ngit commit: {}\ncontracts: {}",
            info.version,
            info.distribution,
            info.git_commit.unwrap_or("unknown"),
            info.contracts.join(", ")
        ),
        _ => {
            return Err(format!(
                "unsupported version format `{format}` (use `human` or `json`)"
            ));
        }
    };
    Ok(output + "\n")
}

pub(crate) fn run(format: &str) -> Result<ExitCode, String> {
    let output = render(format)?;
    if let Err(error) = std::io::stdout().lock().write_all(output.as_bytes())
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(format!("writing version: {error}"));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn handshake_has_the_documented_required_fields() {
        let info: serde_json::Value = serde_json::from_str(&render("json").unwrap()).unwrap();
        assert_eq!(info["schema_version"], 1);
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["distribution"], "gitsense");
        assert_eq!(
            info["contracts"],
            serde_json::json!(["gitsense.sessions.v1"])
        );
        assert!(info.as_object().unwrap().contains_key("git_commit"));
        assert!(
            info["git_commit"].is_null() || is_full_commit(info["git_commit"].as_str().unwrap())
        );
        assert!(render("json").unwrap().ends_with('\n'));
    }

    #[test]
    fn version_command_parses_and_rejects_invalid_formats() {
        for (args, expected) in [
            (vec!["txcript", "version"], "human"),
            (vec!["txcript", "version", "--format", "json"], "json"),
        ] {
            let cli = crate::Cli::try_parse_from(args).unwrap();
            let crate::Command::Version { format } = cli.command else {
                panic!("expected version command");
            };
            assert_eq!(format, expected);
        }
        assert!(crate::Cli::try_parse_from(["txcript", "version", "--format", "yaml"]).is_err());
        assert!(render("yaml").is_err());
    }

    #[test]
    fn human_output_includes_release_identity_and_contract() {
        let output = render("human").unwrap();
        assert!(output.starts_with(&format!("txcript {}\n", env!("CARGO_PKG_VERSION"))));
        assert!(output.contains("gitsense.sessions.v1"));
    }

    #[test]
    fn only_full_git_object_ids_are_accepted() {
        assert!(is_full_commit(&"a".repeat(40)));
        assert!(is_full_commit(&"b".repeat(64)));
        for invalid in ["", "54b95f7", &"x".repeat(40)] {
            assert!(!is_full_commit(invalid));
        }
    }
}
