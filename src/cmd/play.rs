use anyhow::{Result, anyhow};

use crate::{
    config::TuiConfig,
    pikpak::{EventsResponse, PlayOption},
    player::{self, PlayContext},
    theme::{self, FileCategory},
};

const USAGE: &str = concat!(
    "Usage: pikpaktui play [-l|--list-qualities] <path> [quality]\n",
    "       pikpaktui play --latest [-l|--list-qualities] [quality]\n\n",
    "quality: original (default), a stream name/resolution like 1080p, or a stream index\n",
    "--list-qualities lists streams without launching a player\n",
    "--latest selects the newest added video in the first 20 events",
);

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Path(String),
    Latest,
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    target: Target,
    quality: Option<String>,
    list_qualities: bool,
}

fn parse_args(args: &[String]) -> Result<Args> {
    let mut latest = false;
    let mut list_qualities = false;
    let mut positional = Vec::new();
    let mut options = true;
    for arg in args {
        match arg.as_str() {
            "--" if options => options = false,
            "--latest" if options => latest = true,
            "-l" | "--list-qualities" if options => list_qualities = true,
            value if options && value.starts_with('-') => {
                return Err(anyhow!("unknown play option: {value}\n{USAGE}"));
            }
            _ => positional.push(arg.clone()),
        }
    }

    let (target, quality) = match (latest, positional.as_slice()) {
        (false, [path]) => (Target::Path(path.clone()), None),
        (false, [path, quality]) => (Target::Path(path.clone()), Some(quality.clone())),
        (true, []) => (Target::Latest, None),
        (true, [quality]) if !quality.contains(['/', '\\']) => {
            (Target::Latest, Some(quality.clone()))
        }
        _ => return Err(anyhow!("{USAGE}")),
    };
    if list_qualities && quality.is_some() {
        return Err(anyhow!(
            "--list-qualities cannot be combined with a quality selection\n{USAGE}"
        ));
    }
    Ok(Args {
        target,
        quality,
        list_qualities,
    })
}

pub fn run(args: &[String]) -> Result<()> {
    let args = parse_args(args)?;
    let config = super::cli_config();
    if !args.list_qualities && config.player.is_none() {
        return Err(anyhow!(
            "no player configured.\n\
             Set `player` in config.toml at the top level, e.g.:\n\n  \
             player = \"mpv\""
        ));
    }
    let client = super::cli_client()?;
    let mut context = match &args.target {
        Target::Path(path) => {
            let (parent_path, name) = super::split_parent_name(path)?;
            let parent_id = client.resolve_path(&parent_path)?;
            let entry = super::find_entry(&client, &parent_id, &name)?;
            playback_context(&parent_path, &entry.name, &entry.id)
        }
        Target::Latest => latest_video_context(&client.events(super::events::DEFAULT_LIMIT)?)?,
    };
    let info = client.file_info(&context.id)?;
    // Events can contain an older filename after a rename. Use the current name.
    if matches!(args.target, Target::Latest) {
        context.name = info.name.clone();
    }
    let options = info.play_options();
    if options.is_empty() {
        return Err(anyhow!("no playable streams found for '{}'", context.name));
    }

    if args.list_qualities {
        println!("Available qualities for '{}':", context.name);
        for (i, opt) in options.iter().enumerate() {
            let status = opt
                .unavailable_reason
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            println!("  {}. {}{}", i + 1, opt.display_label(), status);
        }
        return Ok(());
    }

    let option = select_quality(&options, args.quality.as_deref().unwrap_or("original"))?;
    launch_player(&config, &option.url, &option.label, &context)
}

fn select_quality<'a>(options: &'a [PlayOption], quality: &str) -> Result<&'a PlayOption> {
    if quality.parse::<usize>() == Ok(0) {
        return Err(anyhow!(
            "stream indices start at 1; use --list-qualities to inspect streams"
        ));
    }
    let quality_lower = quality.to_ascii_lowercase();
    let indexed = quality
        .parse::<usize>()
        .ok()
        .and_then(|index| index.checked_sub(1).and_then(|index| options.get(index)));
    let option = if let Some(option) = indexed {
        option
    } else {
        let height = if quality_lower == "4k" {
            Some(2160)
        } else {
            quality_lower
                .strip_suffix('p')
                .unwrap_or(&quality_lower)
                .parse::<u32>()
                .ok()
                .filter(|height| *height > 0)
        };
        let matched: Vec<_> = options
            .iter()
            .filter(|option| {
                if quality_lower == "original" {
                    option.is_original
                } else {
                    !quality_lower.is_empty()
                        && (option.label.to_ascii_lowercase().contains(&quality_lower)
                            || (!option.is_original && height.is_some() && option.height == height))
                }
            })
            .collect();
        match matched.as_slice() {
            [option] => *option,
            [] => {
                let available: Vec<_> =
                    options.iter().map(|option| option.label.as_str()).collect();
                return Err(anyhow!(
                    "no stream matching '{}'\nAvailable: {}\nUse --list-qualities to inspect streams.",
                    quality,
                    available.join(", ")
                ));
            }
            _ => {
                let names: Vec<_> = matched.iter().map(|option| option.label.as_str()).collect();
                return Err(anyhow!(
                    "'{}' matches multiple streams: {}\nBe more specific or use a stream index.",
                    quality,
                    names.join(", ")
                ));
            }
        }
    };

    if !option.available {
        return Err(anyhow!(
            "stream '{}' is not available: {}",
            option.label,
            option.unavailable_reason.unwrap_or("unavailable")
        ));
    }
    Ok(option)
}

fn latest_video_context(response: &EventsResponse) -> Result<PlayContext> {
    // The events API is newest first. Only inspect its first page, just as
    // `events` does, and ignore playback/deletion/rename activity.
    response
        .events
        .iter()
        .find_map(|event| {
            if !matches!(
                event.event_type.as_deref()?.to_ascii_uppercase().as_str(),
                "TYPE_UPLOAD"
                    | "TYPE_RESTORE"
                    | "TYPE_CREATE"
                    | "TYPE_COPY"
                    | "FILE_CREATED"
                    | "FILE_UPLOADED"
                    | "FILE_RESTORED"
                    | "FILE_COPIED"
            ) {
                return None;
            }
            let resource = event.reference_resource.as_ref();
            if resource.and_then(|resource| resource.kind.as_deref()) == Some("drive#folder") {
                return None;
            }
            let id = resource
                .and_then(|resource| resource.id.as_deref())
                .filter(|id| !id.is_empty())
                .or_else(|| event.file_id.as_deref().filter(|id| !id.is_empty()))?;
            let name = resource
                .and_then(|resource| resource.name.as_deref())
                .filter(|name| !name.is_empty())
                .or_else(|| event.file_name.as_deref().filter(|name| !name.is_empty()))
                .unwrap_or("");
            let video_mime = resource
                .and_then(|resource| resource.mime_type.as_deref())
                .into_iter()
                .chain(event.mime_type.as_deref())
                .any(|mime| mime.starts_with("video/"));
            (video_mime || theme::categorize_name(name) == FileCategory::Video).then(|| {
                PlayContext {
                    name: name.into(),
                    path: String::new(),
                    id: id.into(),
                }
            })
        })
        .ok_or_else(|| {
            anyhow!(
                "no recently added video found in the first {} events",
                super::events::DEFAULT_LIMIT
            )
        })
}

fn playback_context(parent_path: &str, name: &str, id: &str) -> PlayContext {
    let mut components: Vec<&str> = parent_path
        .trim()
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    components.push(name);
    PlayContext {
        name: name.into(),
        path: format!("/{}", components.join("/")),
        id: id.into(),
    }
}

pub(crate) fn player_command(
    config: &TuiConfig,
    url: &str,
    context: &PlayContext,
) -> Result<std::process::Command> {
    let player = config
        .player
        .as_deref()
        .ok_or_else(|| anyhow!("no player configured"))?;
    player::build_command(player, &config.player_args, url, context)
}

fn launch_player(config: &TuiConfig, url: &str, label: &str, context: &PlayContext) -> Result<()> {
    let mut command = player_command(config, url, context)?;
    let program = command.get_program().to_string_lossy().into_owned();
    eprintln!("Playing '{}' ({}) with {}...", context.name, label, program);
    let mut child = command
        .spawn()
        .map_err(|e| anyhow!("failed to launch {}: {}", program, e))?;
    child.wait().map_err(|e| anyhow!("player error: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args> {
        parse_args(&args.iter().map(|arg| (*arg).into()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_default_quality_listing_and_latest_targets() {
        assert_eq!(
            parse(&["/film.mkv"]).unwrap(),
            Args {
                target: Target::Path("/film.mkv".into()),
                quality: None,
                list_qualities: false,
            }
        );
        assert_eq!(
            parse(&["/film.mkv", "1080"]).unwrap().quality.as_deref(),
            Some("1080")
        );
        for args in [
            vec!["-l", "/film.mkv"],
            vec!["/film.mkv", "--list-qualities"],
        ] {
            assert!(parse(&args).unwrap().list_qualities);
        }
        assert_eq!(
            parse(&["--latest"]).unwrap(),
            Args {
                target: Target::Latest,
                quality: None,
                list_qualities: false,
            }
        );
        assert_eq!(
            parse(&["--latest", "720p"]).unwrap().quality.as_deref(),
            Some("720p")
        );
        assert!(parse(&["--latest", "-l"]).unwrap().list_qualities);
        assert_eq!(
            parse(&["--", "--latest"]).unwrap().target,
            Target::Path("--latest".into())
        );
    }

    #[test]
    fn rejects_invalid_arguments_before_loading_a_client() {
        for args in [
            vec![],
            vec!["--list-qualities"],
            vec!["--quality", "1080", "/film.mkv"],
            vec!["--latest", "/film.mkv"],
            vec!["--latest", "1080", "extra"],
            vec!["/film.mkv", "1080", "extra"],
            vec!["-l", "/film.mkv", "1080"],
            vec!["--latest", "-l", "1080"],
        ] {
            assert!(parse(&args).is_err(), "unexpectedly accepted {args:?}");
        }
    }

    fn options() -> Vec<PlayOption> {
        serde_json::from_value::<crate::pikpak::FileInfoResponse>(serde_json::json!({
            "name":"film.mkv", "medias":[
                {"is_origin":true,"link":{"url":"original"},"video":{"height":1080}},
                {"media_name":"Full HD","link":{"url":"1080"},"video":{"height":1080}},
                {"media_name":"720p","link":{"url":"720"}},
                {"media_name":"4K","link":{"url":"4k"},"need_more_quota":true}
            ]
        }))
        .unwrap()
        .play_options()
    }

    #[test]
    fn selects_original_resolution_names_and_one_based_indices() {
        let options = options();
        for quality in ["original", "ORIGINAL", "1"] {
            assert!(select_quality(&options, quality).unwrap().is_original);
        }
        for quality in ["1080", "1080p", "full hd", "2"] {
            assert_eq!(select_quality(&options, quality).unwrap().url, "1080");
        }
        assert_eq!(select_quality(&options, "720").unwrap().url, "720");
        for quality in ["0", "999", "", "4K", "2160p", "4"] {
            assert!(
                select_quality(&options, quality).is_err(),
                "unexpectedly accepted {quality}"
            );
        }
    }

    #[test]
    fn unavailable_or_missing_original_never_silently_plays_a_transcode() {
        let mut options = options();
        options[0].available = false;
        options[0].unavailable_reason = Some("additional playback quota required");
        let error = select_quality(&options, "original")
            .unwrap_err()
            .to_string();
        assert!(error.contains("additional playback quota required"));
        options.remove(0);
        assert!(select_quality(&options, "original").is_err());
        options.push(options[1].clone());
        assert!(
            select_quality(&options, "720")
                .unwrap_err()
                .to_string()
                .contains("multiple streams")
        );
    }

    #[test]
    fn latest_uses_newest_addition_video_and_its_file_id_not_event_id() {
        let events = serde_json::from_value::<EventsResponse>(serde_json::json!({"events":[
            {"type":"TYPE_PLAY","file_id":"played","file_name":"played.mp4"},
            {"type":"TYPE_DELETE","file_id":"deleted","file_name":"deleted.mp4"},
            {"type":"TYPE_RENAME","file_id":"renamed","file_name":"renamed.mp4"},
            {"type":"TYPE_UPLOAD","reference_resource":{"id":"folder","kind":"drive#folder","name":"folder.mp4"}},
            {"type":"TYPE_UPLOAD","file_id":"document","file_name":"document.pdf"},
            {"type":"TYPE_RESTORE","id":"event-id","file_id":"file-id","file_name":"old name.mkv",
             "reference_resource":{"id":"file-id","name":"actual name","mime_type":"video/mp4"}},
            {"type":"TYPE_UPLOAD","file_id":"older","file_name":"older.mp4"}
        ]})).unwrap();
        assert_eq!(
            latest_video_context(&events).unwrap(),
            PlayContext {
                id: "file-id".into(),
                name: "actual name".into(),
                path: String::new(),
            }
        );
    }

    #[test]
    fn latest_handles_missing_resource_metadata_and_empty_pages() {
        for event in [
            serde_json::json!({"type":"TYPE_UPLOAD","file_id":"id","file_name":"FILM.MKV"}),
            serde_json::json!({"type":"TYPE_UPLOAD","file_id":"id","file_name":"film","mime_type":"video/mp4"}),
            serde_json::json!({"type":"TYPE_UPLOAD","file_id":"id","mime_type":"video/mp4"}),
            serde_json::json!({"type":"file_created","file_id":"id","file_name":"film.mp4","reference_resource":{"id":"","name":""}}),
        ] {
            let events =
                serde_json::from_value::<EventsResponse>(serde_json::json!({"events":[event]}))
                    .unwrap();
            assert_eq!(latest_video_context(&events).unwrap().id, "id");
        }
        for events in [
            serde_json::json!([]),
            serde_json::json!([
                {"type":"TYPE_UPLOAD","id":"only-event-id","file_name":"film.mp4"},
                {"type":"TYPE_UPLOAD","file_id":"document","file_name":"document.pdf"}
            ]),
        ] {
            let events =
                serde_json::from_value::<EventsResponse>(serde_json::json!({"events":events}))
                    .unwrap();
            assert!(
                latest_video_context(&events)
                    .unwrap_err()
                    .to_string()
                    .contains("first 20 events")
            );
        }
    }

    #[test]
    fn cli_context_matches_resolved_cloud_path_and_stable_id() {
        let context = playback_context(" //TV Shows///Season 1/ ", "--film {id}.mkv", "file-id");
        assert_eq!(context.path, "/TV Shows/Season 1/--film {id}.mkv");
        assert_eq!(context.name, "--film {id}.mkv");
        assert_eq!(context.id, "file-id");
        assert_eq!(playback_context("/", "film.mkv", "id").path, "/film.mkv");
    }
}
