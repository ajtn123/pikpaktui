use anyhow::{Result, anyhow};
use std::{
    collections::HashSet,
    process::Command,
    time::{Duration, Instant},
};

use crate::pikpak::{Entry, EntryKind, PikPak, interactive};

const SUBTITLE_URLS: &str = "{subtitle_urls}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlayContext {
    pub name: String,
    pub path: String,
    pub id: String,
    pub subtitle_urls: Vec<String>,
}

pub(crate) fn needs_subtitles(args: &[String]) -> bool {
    args.iter().any(|arg| arg.contains(SUBTITLE_URLS))
}

/// Inspect an existing directory snapshot; this function never lists a folder.
pub(crate) fn matching_subtitle_ids(name: &str, entries: &[Entry]) -> Vec<String> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if stem.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            let (subtitle_stem, extension) = entry.name.rsplit_once('.')?;
            let extension = extension.to_ascii_lowercase();
            (entry.kind == EntryKind::File
                && !entry.id.is_empty()
                && matches!(
                    extension.as_str(),
                    "ass" | "ssa" | "srt" | "vtt" | "sub" | "ttml" | "smi" | "sami"
                )
                && (subtitle_stem == stem
                    || subtitle_stem
                        .strip_prefix(stem)
                        .is_some_and(|suffix| suffix.starts_with('.') && suffix.len() > 1)))
            .then_some((
                subtitle_stem != stem,
                entry.name.as_str(),
                entry.id.as_str(),
            ))
        })
        .collect();
    matches.sort_unstable();
    let mut seen = HashSet::new();
    matches
        .into_iter()
        .filter(|&(_, _, id)| seen.insert(id))
        .map(|(_, _, id)| id.to_owned())
        .collect()
}

/// Fetch only file details for matching subtitles, never a directory listing.
/// Optional subtitles must not prevent video playback when a link is unavailable.
pub(crate) fn resolve_subtitle_urls(client: &PikPak, ids: &[String]) -> Vec<String> {
    let timeout = Duration::from_secs(5);
    let started = Instant::now();
    interactive::with_deadline(timeout, || {
        let mut urls = Vec::new();
        let mut seen = HashSet::new();
        for id in ids {
            if started.elapsed() >= timeout {
                break;
            }
            if let Ok(info) = client.file_info(id)
                && let Some(url) = info.download_url()
                && seen.insert(url.to_owned())
            {
                urls.push(url.to_owned());
            }
        }
        urls
    })
}

fn expand_argument(arg: &str, context: &PlayContext, subtitle_url: &str) -> String {
    let mut expanded = String::new();
    let mut rest = arg;
    while !rest.is_empty() {
        if let Some((token, value)) = [
            ("{name}", context.name.as_str()),
            ("{path}", context.path.as_str()),
            ("{id}", context.id.as_str()),
            (SUBTITLE_URLS, subtitle_url),
        ]
        .into_iter()
        .find(|(token, _)| rest.starts_with(token))
        {
            expanded.push_str(value);
            rest = &rest[token.len()..];
        } else {
            let ch = rest.chars().next().unwrap();
            expanded.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    expanded
}

pub(crate) fn build_command(
    player: &str,
    extra_args: &[String],
    url: &str,
    context: &PlayContext,
) -> Result<Command> {
    let mut parts = player.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| anyhow!("player command is empty"))?;
    let mut command = Command::new(program);
    command.args(parts);
    for arg in extra_args {
        if arg.contains(SUBTITLE_URLS) {
            for subtitle_url in &context.subtitle_urls {
                command.arg(expand_argument(arg, context, subtitle_url));
            }
        } else {
            command.arg(expand_argument(arg, context, ""));
        }
    }
    command
        .arg("--")
        .arg(url)
        .env("PIKPAKTUI_PLAY_NAME", &context.name)
        .env("PIKPAKTUI_PLAY_PATH", &context.path)
        .env("PIKPAKTUI_PLAY_ID", &context.id);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, ffi::OsStr};

    #[test]
    fn subtitle_expansion_repeats_arguments_without_reexpanding_inserted_values() {
        let mut context = PlayContext {
            name: "电影 {subtitle_urls}.mkv".into(),
            path: "/folder/电影.mkv".into(),
            id: "id".into(),
            subtitle_urls: vec![
                "https://example.invalid/电影 'one'.ass?x=1&y={name}".into(),
                "https://example.invalid/two.ass?x=a,b;c".into(),
            ],
        };
        let args = vec![
            "--title={name}".into(),
            "--sub-file={subtitle_urls}".into(),
            "{id}:{subtitle_urls}:{subtitle_urls}".into(),
            "--other".into(),
        ];
        assert!(needs_subtitles(&args));
        let command = build_command("mpv", &args, "video-url", &context).unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                "--title=电影 {subtitle_urls}.mkv",
                "--sub-file=https://example.invalid/电影 'one'.ass?x=1&y={name}",
                "--sub-file=https://example.invalid/two.ass?x=a,b;c",
                "id:https://example.invalid/电影 'one'.ass?x=1&y={name}:https://example.invalid/电影 'one'.ass?x=1&y={name}",
                "id:https://example.invalid/two.ass?x=a,b;c:https://example.invalid/two.ass?x=a,b;c",
                "--other",
                "--",
                "video-url",
            ]
        );
        context.subtitle_urls.clear();
        let command = build_command("mpv", &args, "video-url", &context).unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                "--title=电影 {subtitle_urls}.mkv",
                "--other",
                "--",
                "video-url",
            ]
        );
        assert!(!needs_subtitles(&["--title={name}".into()]));
    }

    fn entry(id: &str, name: &str) -> Entry {
        Entry {
            id: id.into(),
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            created_time: String::new(),
            modified_time: String::new(),
            starred: false,
            thumbnail_link: None,
            phase: None,
            audit: None,
        }
    }

    #[test]
    fn subtitles_match_the_full_video_stem_with_optional_dot_suffixes() {
        let mut folder = entry("folder", "video.ass");
        folder.kind = EntryKind::Folder;
        let entries = vec![
            entry("zh", "video.zh.ass"),
            entry("bare", "video.ass"),
            entry("en", "video.en.forced.SRT"),
            entry("vtt", "video.en.vtt"),
            entry("bare", "video.ass"),
            entry("different", "video2.ass"),
            entry("prefix", "other-video.ass"),
            entry("case", "Video.zh.ass"),
            entry("wrong-extension", "video.zh.txt"),
            entry("empty-suffix", "video..ass"),
            entry("", "video.srt"),
            folder,
        ];
        assert_eq!(
            matching_subtitle_ids("video.mp4", &entries),
            vec!["bare", "en", "vtt", "zh"]
        );
        assert!(matching_subtitle_ids("other.mp4", &entries).is_empty());
        assert!(matching_subtitle_ids(".mp4", &entries).is_empty());
        let entries = vec![
            entry("match", "节目.S01E01.zh.ass"),
            entry("other-episode", "节目.S01E010.zh.ass"),
        ];
        assert_eq!(
            matching_subtitle_ids("节目.S01E01.mkv", &entries),
            vec!["match"]
        );
    }

    #[test]
    fn safe_arguments_preserve_boundaries_and_substitute_context_once() {
        let context = PlayContext {
            name: "--movie 'one' {path}.mkv".into(),
            path: "/TV Shows/--movie 'one' {path}.mkv".into(),
            id: "file-{name}".into(),
            subtitle_urls: Vec::new(),
        };
        let command = build_command(
            "wrapper --legacy",
            &[
                "--title={name}".into(),
                "{path}".into(),
                "{id}".into(),
                "quote free two words".into(),
                "{unknown}".into(),
                "$(touch ignored)".into(),
            ],
            "https://example.invalid/video?x=1&y=2",
            &context,
        )
        .unwrap();
        assert_eq!(command.get_program(), OsStr::new("wrapper"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                "--legacy",
                "--title=--movie 'one' {path}.mkv",
                "/TV Shows/--movie 'one' {path}.mkv",
                "file-{name}",
                "quote free two words",
                "{unknown}",
                "$(touch ignored)",
                "--",
                "https://example.invalid/video?x=1&y=2",
            ]
        );
    }

    #[test]
    fn legacy_player_splitting_and_separator_remain_unchanged() {
        let context = PlayContext {
            name: String::new(),
            path: String::new(),
            id: "id".into(),
            subtitle_urls: Vec::new(),
        };
        let command = build_command("  open -a IINA  ", &[], "url", &context).unwrap();
        assert_eq!(command.get_program(), OsStr::new("open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["-a", "IINA", "--", "url"]
        );
        assert!(build_command("  ", &[], "url", &context).is_err());
    }

    #[test]
    fn context_is_set_only_on_the_child_command() {
        let context = PlayContext {
            name: "film\".mkv".into(),
            path: "/film\".mkv".into(),
            id: "id".into(),
            subtitle_urls: Vec::new(),
        };
        let before = std::env::var_os("PIKPAKTUI_PLAY_NAME");
        let command = build_command("mpv", &[], "url", &context).unwrap();
        let env: BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(OsStr::new("PIKPAKTUI_PLAY_NAME")),
            Some(&Some(OsStr::new("film\".mkv")))
        );
        assert_eq!(
            env.get(OsStr::new("PIKPAKTUI_PLAY_PATH")),
            Some(&Some(OsStr::new("/film\".mkv")))
        );
        assert_eq!(
            env.get(OsStr::new("PIKPAKTUI_PLAY_ID")),
            Some(&Some(OsStr::new("id")))
        );
        assert_eq!(std::env::var_os("PIKPAKTUI_PLAY_NAME"), before);
    }
}
