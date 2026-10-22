//! Static identifier discovery: no store scans, network calls, or tool probes.
use std::io::Write;
use std::process::ExitCode;

use txcript::HarnessId;

fn render(format: &str) -> Result<String, String> {
    let names: Vec<_> = HarnessId::ALL.into_iter().map(HarnessId::as_str).collect();
    let output = match format {
        "human" => names.join("\n"),
        "json" => serde_json::to_string_pretty(&names).map_err(|e| e.to_string())?,
        _ => {
            return Err(format!(
                "unsupported harnesses format `{format}` (use `human` or `json`)"
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
        return Err(format!("writing harnesses: {error}"));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn human_and_json_list_every_canonical_identifier_once() {
        let expected: Vec<_> = HarnessId::ALL.into_iter().map(HarnessId::as_str).collect();
        let human = render("human").unwrap();
        assert_eq!(human.lines().collect::<Vec<_>>(), expected);
        let json: Vec<String> = serde_json::from_str(&render("json").unwrap()).unwrap();
        assert_eq!(json, expected);
        let unique: std::collections::HashSet<_> = json.iter().collect();
        assert_eq!(unique.len(), HarnessId::ALL.len());
        assert!(json.iter().any(|name| name == "grok_bot"));
    }

    #[test]
    fn command_defaults_to_human_and_accepts_json() {
        for (args, expected) in [
            (vec!["txcript", "harnesses"], "human"),
            (vec!["txcript", "harnesses", "--format", "json"], "json"),
        ] {
            let cli = crate::Cli::try_parse_from(args).unwrap();
            let crate::Command::Harnesses { format } = cli.command else {
                panic!("expected harnesses command");
            };
            assert_eq!(format, expected);
        }
    }

    #[test]
    fn invalid_formats_are_rejected() {
        assert!(render("yaml").is_err());
        assert!(crate::Cli::try_parse_from(["txcript", "harnesses", "--format", "yaml"]).is_err());
    }

    #[test]
    fn help_registry_matches_canonical_identifiers() {
        let help_names: Vec<_> = crate::HARNESSES
            .strip_prefix("harnesses: ")
            .unwrap()
            .split(',')
            .map(str::trim)
            .collect();
        assert_eq!(help_names.len(), HarnessId::ALL.len());
        for harness in HarnessId::ALL {
            assert!(help_names.contains(&harness.as_str()));
        }
    }
}
