use anyhow::{Result, anyhow};
use std::path::Path;

const USAGE: &str = "Usage: pikpaktui hash [-J|--json] [--] <local_path>";

fn parse_args(args: &[String]) -> Result<(&str, bool)> {
    let mut path = None;
    let mut json = false;
    let mut positional = false;
    for arg in args {
        match arg.as_str() {
            "--" if !positional => positional = true,
            "-J" | "--json" if !positional => json = true,
            flag if !positional && flag.starts_with('-') => {
                return Err(anyhow!("unknown option: {flag}\n{USAGE}"));
            }
            value => {
                if path.replace(value).is_some() {
                    return Err(anyhow!("expected one local file\n{USAGE}"));
                }
            }
        }
    }
    Ok((path.ok_or_else(|| anyhow!(USAGE))?, json))
}

pub fn run(args: &[String]) -> Result<()> {
    let (path, json) = parse_args(args)?;
    let hash = crate::pikpak::pikpak_hash(Path::new(path))?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "path": path, "pikpak_hash": hash })
        );
    } else {
        println!("{hash}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options_and_literal_filenames() {
        let args = ["--json", "--", "-movie.mkv"].map(String::from);
        assert_eq!(parse_args(&args).unwrap(), ("-movie.mkv", true));
        let args = ["movie with spaces.mkv", "-J"].map(String::from);
        assert_eq!(parse_args(&args).unwrap(), ("movie with spaces.mkv", true));
    }

    #[test]
    fn rejects_missing_extra_and_unknown_arguments() {
        for args in [vec![], vec!["--json"], vec!["a", "b"], vec!["--invalid"]] {
            let args: Vec<_> = args.into_iter().map(String::from).collect();
            assert!(parse_args(&args).is_err());
        }
    }
}
