use anyhow::{Result, anyhow};

use crate::pikpak::PlayOption;

const USAGE: &str = "Usage: pikpaktui play [options] <path> | --recent\n\n\
    -r, --recent             Play the newest added video on the first events page\n\
    -q, --quality <quality>  Stream name or number (default: original)\n\
    -l, --list-stream        List available streams without playing";

#[derive(Debug, PartialEq)]
struct PlayArgs<'a> {
    path: Option<&'a str>,
    recent: bool,
    quality: &'a str,
    list_stream: bool,
}

fn parse_args(args: &[String]) -> Result<PlayArgs<'_>> {
    let mut parsed = PlayArgs {
        path: None,
        recent: false,
        quality: "original",
        list_stream: false,
    };
    let mut positional = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if !positional => positional = true,
            "-r" | "--recent" if !positional => parsed.recent = true,
            "-l" | "--list-stream" if !positional => parsed.list_stream = true,
            "-q" | "--quality" if !positional => {
                parsed.quality = args
                    .next()
                    .map(String::as_str)
                    .filter(|value| !value.is_empty() && !value.starts_with('-'))
                    .ok_or_else(|| anyhow!("{arg} requires a quality\n{USAGE}"))?;
            }
            value if !positional && value.starts_with("--quality=") => {
                parsed.quality = value.strip_prefix("--quality=").unwrap();
                if parsed.quality.is_empty() {
                    return Err(anyhow!("--quality requires a quality\n{USAGE}"));
                }
            }
            flag if !positional && flag.starts_with('-') => {
                return Err(anyhow!("unknown option: {flag}\n{USAGE}"));
            }
            path => {
                if parsed.path.replace(path).is_some() {
                    return Err(anyhow!(
                        "expected one path; use -q/--quality to select a stream\n{USAGE}"
                    ));
                }
            }
        }
    }
    if parsed.recent && parsed.path.is_some() {
        return Err(anyhow!("--recent cannot be combined with a path\n{USAGE}"));
    }
    if !parsed.recent && parsed.path.is_none() {
        return Err(anyhow!(USAGE));
    }
    Ok(parsed)
}

fn select_stream<'a>(options: &'a [PlayOption], quality: &str) -> Result<&'a PlayOption> {
    let selected = if let Ok(num) = quality.parse::<usize>() {
        if num == 0 || num > options.len() {
            return Err(anyhow!(
                "invalid stream number: {}. Available: 1-{}",
                num,
                options.len()
            ));
        }
        &options[num - 1]
    } else {
        let quality_lower = quality.to_lowercase();
        let matched: Vec<&PlayOption> = options
            .iter()
            .filter(|option| option.label.to_lowercase().contains(&quality_lower))
            .collect();
        match matched.as_slice() {
            [] => {
                let available: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
                return Err(anyhow!(
                    "no stream matching '{}'\nAvailable: {}",
                    quality,
                    available.join(", ")
                ));
            }
            [option] => *option,
            _ => {
                let names: Vec<&str> = matched.iter().map(|o| o.label.as_str()).collect();
                return Err(anyhow!(
                    "'{}' matches multiple streams: {}\nBe more specific.",
                    quality,
                    names.join(", ")
                ));
            }
        }
    };
    if !selected.available {
        return Err(anyhow!(
            "stream '{}' is not available: {}",
            selected.label,
            selected.unavailable_reason.unwrap_or("unavailable")
        ));
    }
    Ok(selected)
}

pub fn run(args: &[String]) -> Result<()> {
    let args = parse_args(args)?;
    let client = super::cli_client()?;
    let (entry, parent_id) = if let Some(path) = args.path {
        let (parent_path, name) = super::split_parent_name(path)?;
        let parent_id = client.resolve_path(&parent_path)?;
        let entry = super::find_entry(&client, &parent_id, &name)?;
        (entry, parent_id)
    } else {
        client
            .recent_video()?
            .ok_or_else(|| anyhow!("no videos found on the first page of recent events"))?
    };

    let options = client.file_info(&entry.id)?.play_options();
    if options.is_empty() {
        return Err(anyhow!("no playable streams found for '{}'", entry.name));
    }
    if args.list_stream {
        println!("Available streams for '{}':", entry.name);
        for (i, opt) in options.iter().enumerate() {
            let status = opt
                .unavailable_reason
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default();
            println!("  {}. {}{}", i + 1, opt.display_label(), status);
        }
        println!();
        if let Some(path) = args.path {
            println!("Run: pikpaktui play -q <quality> -- \"{}\"", path);
        } else {
            println!("Run: pikpaktui play --recent -q <quality>");
        }
        return Ok(());
    }

    let opt = select_stream(&options, args.quality)?;
    let config = super::cli_config();
    let player = config.player.as_deref().ok_or_else(|| {
        anyhow!(
            "no player configured.\n\
             Set the top-level `player` in ~/.config/pikpaktui/config.toml, e.g.:\n\n  \
             player = \"mpv\""
        )
    })?;
    let mut command =
        crate::playback::prepare_player(&client, player, &parent_id, &entry.name, &opt.url)?;
    eprintln!(
        "Playing '{}' ({}) with {}...",
        entry.name, opt.label, player
    );
    let mut child = command
        .spawn()
        .map_err(|e| anyhow!("failed to launch {player}: {e}"))?;
    let status = child.wait().map_err(|e| anyhow!("player error: {e}"))?;
    if !status.success() {
        return Err(anyhow!("player exited with {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_playing_original_for_path_or_recent() {
        let args = ["/Movies/movie.mkv".into()];
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.path, Some("/Movies/movie.mkv"));
        assert!(!parsed.recent);
        assert_eq!(parsed.quality, "original");
        assert!(!parsed.list_stream);

        for recent in ["-r", "--recent"] {
            let args = [recent.into()];
            let parsed = parse_args(&args).unwrap();
            assert_eq!(parsed.path, None);
            assert!(parsed.recent);
            assert_eq!(parsed.quality, "original");
            assert!(!parsed.list_stream);
        }
    }

    #[test]
    fn accepts_quality_options_before_or_after_path() {
        for args in [
            vec!["-q", "1080p", "/movie.mkv"],
            vec!["/movie.mkv", "--quality", "1080p"],
            vec!["--quality=1080p", "/movie.mkv"],
        ] {
            let args: Vec<_> = args.into_iter().map(String::from).collect();
            let parsed = parse_args(&args).unwrap();
            assert_eq!(parsed.path, Some("/movie.mkv"));
            assert_eq!(parsed.quality, "1080p");
        }
        let args = ["--recent", "-q", "2"].map(String::from);
        assert_eq!(parse_args(&args).unwrap().quality, "2");
    }

    #[test]
    fn lists_streams_for_path_or_recent_with_or_without_quality() {
        for args in [
            vec!["/movie.mkv", "-l"],
            vec!["--list-stream", "/movie.mkv"],
            vec!["--recent", "-l"],
            vec!["-r", "--list-stream", "--quality", "720p"],
        ] {
            let args: Vec<_> = args.into_iter().map(String::from).collect();
            assert!(parse_args(&args).unwrap().list_stream);
        }
    }

    #[test]
    fn accepts_literal_paths_after_separator() {
        let args = ["-q", "720p", "--", "--recent"].map(String::from);
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.path, Some("--recent"));
        assert!(!parsed.recent);
    }

    #[test]
    fn rejects_missing_conflicting_and_legacy_arguments() {
        for args in [
            vec![],
            vec!["-l"],
            vec!["-q", "original"],
            vec!["--unknown"],
            vec!["/movie.mkv", "original"],
            vec!["/movie.mkv", "--recent"],
            vec!["-r", "-q"],
            vec!["/movie.mkv", "--quality", "-l"],
            vec!["-r", "--quality="],
            vec!["-r", "--quality", ""],
        ] {
            let args: Vec<_> = args.into_iter().map(String::from).collect();
            assert!(parse_args(&args).is_err(), "{args:?}");
        }
    }

    fn streams() -> Vec<PlayOption> {
        [
            ("original (1.0 GB)", true),
            ("1080p", true),
            ("720p", false),
        ]
        .into_iter()
        .map(|(label, available)| PlayOption {
            label: label.into(),
            url: format!("https://example.com/{label}"),
            available,
            unavailable_reason: (!available).then_some("additional playback quota required"),
            is_original: label.starts_with("original"),
            height: None,
        })
        .collect()
    }

    #[test]
    fn selects_default_original_stream_names_and_numbers() {
        let options = streams();
        let args = ["-r".into()];
        assert_eq!(
            select_stream(&options, parse_args(&args).unwrap().quality)
                .unwrap()
                .label,
            "original (1.0 GB)"
        );
        for quality in ["1080p", "1080P", "2"] {
            assert_eq!(select_stream(&options, quality).unwrap().label, "1080p");
        }
    }

    #[test]
    fn rejects_unavailable_missing_ambiguous_and_out_of_range_streams() {
        let options = streams();
        for quality in ["720p", "3"] {
            assert!(
                select_stream(&options, quality)
                    .unwrap_err()
                    .to_string()
                    .contains("additional playback quota required")
            );
        }
        for (quality, error) in [
            ("4k", "no stream matching"),
            ("p", "matches multiple streams"),
            ("0", "invalid stream number"),
            ("4", "invalid stream number"),
        ] {
            assert!(
                select_stream(&options, quality)
                    .unwrap_err()
                    .to_string()
                    .contains(error)
            );
        }
    }

    #[test]
    fn uses_shared_media_visibility_and_availability_for_cli_selection() {
        let info: crate::pikpak::FileInfoResponse = serde_json::from_value(serde_json::json!({
            "name": "movie.mkv",
            "web_content_link": "https://example.com/download",
            "medias": [
                {
                    "media_name": "720p",
                    "is_visible": true,
                    "link": { "url": "https://example.com/720p" }
                },
                {
                    "media_name": "source",
                    "is_origin": true,
                    "is_visible": true,
                    "need_more_quota": true,
                    "link": { "url": "https://example.com/original" }
                },
                {
                    "media_name": "1080p",
                    "is_visible": false,
                    "link": { "url": "https://example.com/hidden" }
                },
                { "media_name": "4k", "is_visible": true }
            ]
        }))
        .unwrap();
        let options = info.play_options();

        assert_eq!(options.len(), 3);
        assert_eq!(options[0].label, "Original");
        assert_eq!(
            select_stream(&options, "720p").unwrap().url,
            "https://example.com/720p"
        );
        assert_eq!(select_stream(&options, "2").unwrap().label, "720p");
        for quality in ["original", "1"] {
            assert!(
                select_stream(&options, quality)
                    .unwrap_err()
                    .to_string()
                    .contains("additional playback quota required")
            );
        }
        assert!(
            select_stream(&options, "1080p")
                .unwrap_err()
                .to_string()
                .contains("no stream matching")
        );
        assert!(
            select_stream(&options, "4k")
                .unwrap_err()
                .to_string()
                .contains("media link not ready; refresh to retry")
        );
    }
}
