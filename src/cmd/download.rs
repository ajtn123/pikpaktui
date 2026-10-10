use crate::pikpak::{Entry, EntryKind, PikPak};
use anyhow::{Result, anyhow};

const USAGE: &str = "Usage: pikpaktui download [-n] [-j <n>] [-o <output>] <path> [output]\n\
    Usage: pikpaktui download [-n] [-j <n>] -t <local_dir> <path...>\n\
    Usage: pikpaktui download [-n] [-j <n>] [-o <output> | -t <local_dir>] --recent\n\n\
    If <path> is a folder, the entire directory tree is downloaded recursively.\n\
    -r / --recent  download the newest added file or folder on the first events page\n\
    -j / --jobs <n>  concurrent file downloads (default: 1)";

#[derive(Debug, PartialEq)]
struct DownloadArgs<'a> {
    output: Option<&'a str>,
    target_dir: Option<&'a str>,
    dry_run: bool,
    jobs: usize,
    paths: Vec<&'a str>,
    recent: bool,
}

fn destination_path(
    output: Option<&str>,
    positional_output: Option<&str>,
    remote_name: &str,
) -> std::path::PathBuf {
    match output.or(positional_output) {
        Some(explicit) => std::path::PathBuf::from(explicit),
        None => std::path::PathBuf::from(crate::pikpak::sanitize_filename(remote_name)),
    }
}

fn target_destination(target_dir: &std::path::Path, remote_name: &str) -> std::path::PathBuf {
    target_dir.join(crate::pikpak::sanitize_filename(remote_name))
}

fn parse_args(args: &[String]) -> Result<DownloadArgs<'_>> {
    if args.is_empty() {
        return Err(anyhow!(USAGE));
    }

    let mut output: Option<&str> = None;
    let mut target_dir: Option<&str> = None;
    let mut dry_run = false;
    let mut jobs: usize = 1;
    let mut paths: Vec<&str> = Vec::new();
    let mut recent = false;
    let mut positional = false;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" if !positional => positional = true,
            "-r" | "--recent" if !positional => recent = true,
            "-n" | "--dry-run" if !positional => dry_run = true,
            "-j" | "--jobs" if !positional => {
                let val = iter.next().ok_or_else(|| anyhow!("-j requires a number"))?;
                jobs = val
                    .parse::<usize>()
                    .map_err(|_| anyhow!("-j requires a positive integer"))?;
                if jobs == 0 {
                    return Err(anyhow!("-j must be at least 1"));
                }
                if jobs > 16 {
                    return Err(anyhow!("-j must be at most 16"));
                }
            }
            "-o" | "--output" if !positional => {
                output = Some(
                    iter.next()
                        .ok_or_else(|| anyhow!("-o requires an output path"))?
                        .as_str(),
                );
            }
            "-t" if !positional => {
                target_dir = Some(
                    iter.next()
                        .ok_or_else(|| anyhow!("-t requires a directory path"))?
                        .as_str(),
                );
            }
            s if !positional && s.starts_with('-') && s != "-" => {
                return Err(anyhow!("unknown option: {s}"));
            }
            _ => paths.push(arg),
        }
    }

    if recent && !paths.is_empty() {
        return Err(anyhow!(
            "--recent cannot be combined with a path; use -o or -t for the destination\n{USAGE}"
        ));
    }
    if !recent && paths.is_empty() {
        return Err(anyhow!("no file path specified"));
    }
    if target_dir.is_none() {
        // Single-file form takes `<path> [output]`; extra positionals were
        // silently dropped, and a positional output was silently beaten by -o.
        if paths.len() > 2 {
            return Err(anyhow!(
                "too many arguments: expected <path> [output] (use -t <dir> for multiple sources)"
            ));
        }
        if output.is_some() && paths.len() > 1 {
            return Err(anyhow!("both -o and a positional output were given"));
        }
    }

    Ok(DownloadArgs {
        output,
        target_dir,
        dry_run,
        jobs,
        paths,
        recent,
    })
}

pub fn run(args: &[String]) -> Result<()> {
    let args = parse_args(args)?;
    let client = super::cli_client()?;

    if args.recent {
        let (entry, _) = client.recent_entry()?.ok_or_else(|| {
            anyhow!("no files or folders found on the first page of recent events")
        })?;
        let dest = if let Some(dir) = args.target_dir {
            target_destination(std::path::Path::new(dir), &entry.name)
        } else {
            destination_path(args.output, None, &entry.name)
        };
        download_entry(&client, &entry, &dest, args.jobs, args.dry_run)?;
    } else {
        let sources = if args.target_dir.is_some() {
            &args.paths[..]
        } else {
            &args.paths[..1]
        };
        for path in sources {
            let (parent, name) = super::split_parent_name(path)?;
            let parent_id = client.resolve_path(&parent)?;
            let entry = super::find_entry(&client, &parent_id, &name)?;
            let dest = if let Some(dir) = args.target_dir {
                target_destination(std::path::Path::new(dir), &entry.name)
            } else {
                destination_path(args.output, args.paths.get(1).copied(), &entry.name)
            };
            download_entry(&client, &entry, &dest, args.jobs, args.dry_run)?;
        }
    }
    Ok(())
}

fn download_entry(
    client: &PikPak,
    entry: &Entry,
    dest: &std::path::Path,
    jobs: usize,
    dry_run: bool,
) -> Result<()> {
    let name = &entry.name;
    if dry_run {
        let kind_tag = if entry.kind == EntryKind::Folder {
            "folder".to_string()
        } else {
            super::format_size(entry.size)
        };
        println!(
            "[dry-run] Would download '{}' ({}) -> '{}'",
            name,
            kind_tag,
            dest.display()
        );
        return Ok(());
    }

    if entry.kind == EntryKind::Folder {
        let parent_dest = dest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let folder_name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        println!(
            "Downloading folder '{}' -> '{}'{}",
            name,
            dest.display(),
            if jobs > 1 {
                format!(" ({jobs} concurrent)")
            } else {
                String::new()
            }
        );
        let (ok, failed) = client.download_dir(&entry.id, &folder_name, parent_dest, jobs)?;
        println!(
            "Folder '{}' done: {} file(s) ok, {} failed",
            name, ok, failed
        );
        if failed > 0 {
            return Err(anyhow!("{} file(s) failed in '{}'", failed, name));
        }
    } else {
        if let Some(parent) = dest.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        eprintln!(
            "{} ({}) downloading...",
            name,
            super::format_size(entry.size)
        );
        let total = client.download_to(&entry.id, dest)?;
        println!(
            "Downloaded '{}' -> '{}' ({})",
            name,
            dest.display(),
            super::format_size(total)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_recent_aliases_without_a_source_path() {
        for recent in ["-r", "--recent"] {
            let args = [recent.into()];
            let parsed = parse_args(&args).unwrap();
            assert!(parsed.recent);
            assert!(parsed.paths.is_empty());
            assert_eq!(parsed.output, None);
            assert_eq!(parsed.target_dir, None);
            assert_eq!(parsed.jobs, 1);
            assert!(!parsed.dry_run);
        }
    }

    #[test]
    fn recent_supports_output_target_dry_run_and_jobs() {
        for args in [
            vec!["--recent", "-o", "local-name", "-n", "-j", "4"],
            vec!["--output", "local-name", "--dry-run", "--jobs", "4", "-r"],
        ] {
            let args: Vec<_> = args.into_iter().map(String::from).collect();
            let parsed = parse_args(&args).unwrap();
            assert!(parsed.recent);
            assert_eq!(parsed.output, Some("local-name"));
            assert!(parsed.dry_run);
            assert_eq!(parsed.jobs, 4);
        }
        let args = ["-r", "-t", "./downloads", "-j", "16"].map(String::from);
        let parsed = parse_args(&args).unwrap();
        assert!(parsed.recent);
        assert_eq!(parsed.target_dir, Some("./downloads"));
        assert_eq!(parsed.jobs, 16);
    }

    #[test]
    fn rejects_recent_with_positional_paths_before_client_setup() {
        for args in [
            vec!["--recent", "/movie.mkv"],
            vec!["/movie.mkv", "-r"],
            vec!["-r", "-o", "local.mkv", "/movie.mkv"],
            vec!["-r", "-t", "./downloads", "/Movies", "/Notes"],
            vec!["-r", "--", "--recent"],
        ] {
            let args: Vec<_> = args.into_iter().map(String::from).collect();
            let error = run(&args).unwrap_err();
            assert!(error.to_string().contains("cannot be combined with a path"));
        }
    }

    #[test]
    fn retains_explicit_single_and_multiple_source_forms() {
        let args = ["/movie.mkv", "local.mkv"].map(String::from);
        let parsed = parse_args(&args).unwrap();
        assert!(!parsed.recent);
        assert_eq!(parsed.paths, ["/movie.mkv", "local.mkv"]);

        let args = ["-t", "./downloads", "/Movies", "/Notes"].map(String::from);
        let parsed = parse_args(&args).unwrap();
        assert!(!parsed.recent);
        assert_eq!(parsed.paths, ["/Movies", "/Notes"]);
        assert_eq!(parsed.target_dir, Some("./downloads"));

        let args = ["--", "--recent"].map(String::from);
        let parsed = parse_args(&args).unwrap();
        assert!(!parsed.recent);
        assert_eq!(parsed.paths, ["--recent"]);
    }

    #[test]
    fn rejects_missing_sources_bad_options_and_conflicting_outputs() {
        for args in [
            vec![],
            vec!["-n"],
            vec!["-o", "local.mkv"],
            vec!["-t", "./downloads"],
            vec!["--recent", "--unknown"],
            vec!["-r", "-o"],
            vec!["-r", "-t"],
            vec!["-r", "-j"],
            vec!["-r", "-j", "0"],
            vec!["-r", "-j", "many"],
            vec!["/movie.mkv", "one.mkv", "two.mkv"],
            vec!["/movie.mkv", "local.mkv", "-o", "other.mkv"],
        ] {
            let args: Vec<_> = args.into_iter().map(String::from).collect();
            assert!(parse_args(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn jobs_above_sixteen_are_rejected_before_client_setup() {
        let err = run(&["-j".into(), "17".into()]).unwrap_err();
        assert!(
            err.to_string().contains("at most 16"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn implicit_destination_sanitizes_remote_path_components() {
        assert_eq!(
            destination_path(None, None, r"..\outside/file.txt"),
            std::path::PathBuf::from("__outside_file.txt")
        );
        assert_eq!(
            target_destination(std::path::Path::new("/safe"), r"..\outside/file.txt"),
            std::path::PathBuf::from("/safe/__outside_file.txt")
        );
    }

    #[test]
    fn explicit_destination_is_preserved_verbatim() {
        assert_eq!(
            destination_path(Some("../chosen/name"), None, "remote.txt"),
            std::path::PathBuf::from("../chosen/name")
        );
        assert_eq!(
            destination_path(None, Some("../positional/name"), "remote.txt"),
            std::path::PathBuf::from("../positional/name")
        );
    }
}
