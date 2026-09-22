use anyhow::{Context, Result, anyhow};
use std::process::Command;

use crate::pikpak::{EntryKind, PikPak};

fn video_title(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn matching_subtitle(video: &str, subtitle: &str) -> bool {
    let Some((stem, extension)) = subtitle.rsplit_once('.') else {
        return false;
    };
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "srt" | "ass" | "ssa" | "vtt"
    ) {
        return false;
    }
    let title = video_title(video);
    !title.is_empty()
        && (stem == title
            || stem
                .strip_prefix(title)
                .is_some_and(|suffix| suffix.starts_with('.')))
}

#[derive(Debug, PartialEq)]
enum Part {
    Literal(String),
    Title,
    Url,
    Subtitle,
}

/// Parse once, before inserting any untrusted filenames or signed URLs.
fn parse_argument(value: &str) -> Result<Vec<Part>> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' | '}' if chars.peek() == Some(&ch) => {
                chars.next();
                literal.push(ch);
            }
            '{' => {
                parts.push(Part::Literal(std::mem::take(&mut literal)));
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => name.push(ch),
                        None => {
                            return Err(anyhow!(
                                "unclosed player placeholder; use '{{{{' for a literal '{{'"
                            ));
                        }
                    }
                }
                parts.push(match name.as_str() {
                    "title" => Part::Title,
                    "url" => Part::Url,
                    "subtitle" => Part::Subtitle,
                    _ => return Err(anyhow!("unknown player placeholder '{{{name}}}'; expected {{title}}, {{url}}, or {{subtitle}}")),
                });
            }
            '}' => {
                return Err(anyhow!(
                    "unmatched '}}' in player template; use '}}}}' for a literal '}}'"
                ));
            }
            _ => literal.push(ch),
        }
    }
    parts.push(Part::Literal(literal));
    Ok(parts)
}

struct PlayerTemplate {
    program: String,
    arguments: Vec<Vec<Part>>,
    has_url: bool,
    has_subtitles: bool,
}

impl PlayerTemplate {
    fn parse(player: &str) -> Result<Self> {
        if player.contains('\0') {
            return Err(anyhow!("player command contains a NUL character"));
        }
        let words = shlex::split(player)
            .ok_or_else(|| anyhow!("invalid quoting or trailing escape in player command"))?;
        let (program, arguments) = words
            .split_first()
            .filter(|(program, _)| !program.is_empty())
            .ok_or_else(|| anyhow!("player command is empty"))?;
        let program = parse_argument(program)?
            .into_iter()
            .map(|part| match part {
                Part::Literal(text) => Ok(text),
                _ => Err(anyhow!("player executable cannot contain placeholders")),
            })
            .collect::<Result<String>>()?;
        let arguments: Vec<_> = arguments
            .iter()
            .map(|arg| parse_argument(arg))
            .collect::<Result<_>>()?;
        let has_url = arguments.iter().any(|arg| arg.contains(&Part::Url));
        let has_subtitles = arguments.iter().any(|arg| arg.contains(&Part::Subtitle));
        if arguments
            .iter()
            .any(|arg| arg.contains(&Part::Subtitle) && arg.contains(&Part::Url))
        {
            return Err(anyhow!(
                "{{url}} and subtitle placeholders must be in separate player arguments"
            ));
        }
        Ok(Self {
            program,
            arguments,
            has_url,
            has_subtitles,
        })
    }

    fn command(&self, url: &str, title: &str, subtitles: &[String]) -> Command {
        let mut command = Command::new(&self.program);
        for argument in &self.arguments {
            let expand = |subtitle: &str| -> String {
                argument
                    .iter()
                    .map(|part| match part {
                        Part::Literal(text) => text.as_str(),
                        Part::Title => title,
                        Part::Url => url,
                        Part::Subtitle => subtitle,
                    })
                    .collect()
            };
            if argument.contains(&Part::Subtitle) {
                for subtitle in subtitles {
                    command.arg(expand(subtitle));
                }
            } else {
                command.arg(expand(""));
            }
        }
        if !self.has_url {
            // Preserve the existing launch convention for plain player commands.
            if command.get_args().last().is_none_or(|arg| arg != "--") {
                command.arg("--");
            }
            command.arg(url);
        }
        command
    }
}

/// Shared by CLI and TUI. Only a subtitle placeholder makes extra API requests.
pub fn prepare_player(
    client: &PikPak,
    player: &str,
    parent_id: &str,
    name: &str,
    url: &str,
) -> Result<Command> {
    let template = PlayerTemplate::parse(player)?;
    let mut subtitles = Vec::new();
    if template.has_subtitles {
        let mut entries = client.ls(parent_id).context("cannot list subtitle files")?;
        entries
            .retain(|entry| entry.kind == EntryKind::File && matching_subtitle(name, &entry.name));
        entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        for entry in entries {
            let info = client
                .file_info(&entry.id)
                .with_context(|| format!("cannot load subtitle '{}'", entry.name))?;
            let link = info
                .download_url()
                .filter(|url| !url.is_empty())
                .ok_or_else(|| anyhow!("no download URL for subtitle '{}'", entry.name))?;
            subtitles.push(link.to_owned());
        }
    }
    Ok(template.command(url, video_title(name), &subtitles))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_same_video_subtitles() {
        for name in [
            "VideoX.ass",
            "VideoX.Y.srt",
            "VideoX.en.forced.SRT",
            "VideoX.vtt",
            "VideoX.ssa",
        ] {
            assert!(matching_subtitle("VideoX.mkv", name), "{name}");
        }
        for name in [
            "VideoXY.srt",
            "Video.srt",
            "VideoX2.ass",
            "VideoX.mkv",
            "VideoX.srt.bak",
        ] {
            assert!(!matching_subtitle("VideoX.mkv", name), "{name}");
        }
        assert!(matching_subtitle("电影 part.1.mkv", "电影 part.1.zh.ass"));
        assert!(matching_subtitle("VideoX", "VideoX.srt"));
    }

    #[test]
    fn passes_each_title_and_url_as_one_argument_before_separator() {
        let command =
            PlayerTemplate::parse("mpv --fullscreen --title={title} --sub-file={subtitle}")
                .unwrap()
                .command(
                    "https://example/video?a=1&b=2",
                    "A movie; $(title)",
                    &[
                        "https://example/sub 1.ass?a=1&b=2".into(),
                        "https://example/sub2.srt".into(),
                    ],
                );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "--fullscreen",
                "--title=A movie; $(title)",
                "--sub-file=https://example/sub 1.ass?a=1&b=2",
                "--sub-file=https://example/sub2.srt",
                "--",
                "https://example/video?a=1&b=2"
            ]
        );
    }

    #[test]
    fn default_playback_preserves_other_players() {
        let command = PlayerTemplate::parse("vlc --fullscreen").unwrap().command(
            "https://example/video",
            "Video",
            &[],
        );
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(args, ["--fullscreen", "--", "https://example/video"]);
        assert!(PlayerTemplate::parse("  ").is_err());
    }

    #[test]
    fn quoted_program_and_arguments_support_other_player_syntax() {
        let template = PlayerTemplate::parse(r#""C:/Program Files/Player/player.exe" --caption "Now playing: {title}" {url} --subtitle={subtitle}"#).unwrap();
        let command = template.command(
            "https://example/video",
            "电影 part 1",
            &["https://example/sub".into()],
        );
        assert_eq!(command.get_program(), "C:/Program Files/Player/player.exe");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--caption",
                "Now playing: 电影 part 1",
                "https://example/video",
                "--subtitle=https://example/sub"
            ]
        );
        let template =
            PlayerTemplate::parse(r"'C:\Program Files\Player\player.exe' {url}").unwrap();
        assert_eq!(template.program, r"C:\Program Files\Player\player.exe");
    }

    #[test]
    fn substituted_values_are_never_reparsed_or_recursively_expanded() {
        let template =
            PlayerTemplate::parse("mpv --title={title} --sub-file={subtitle} -- {url}").unwrap();
        let title = "{url} {subtitle} \"quoted\"; $(echo injected) & | >";
        let subtitle = "https://example/{title}?a=1&b=two words";
        let command = template.command("https://example/{subtitle}", title, &[subtitle.into()]);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                format!("--title={title}"),
                format!("--sub-file={subtitle}"),
                "--".into(),
                "https://example/{subtitle}".into()
            ]
        );
    }

    #[test]
    fn absent_subtitles_remove_only_their_template_arguments() {
        let template =
            PlayerTemplate::parse("mpv --sub-file={subtitle} --title={title} --").unwrap();
        let command = template.command("url", "Video", &[]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--title=Video", "--", "url"]
        );
    }

    #[test]
    fn standalone_subtitle_placeholder_expands_to_separate_arguments() {
        let command = PlayerTemplate::parse("wrapper {url} {subtitle}")
            .unwrap()
            .command("url", "Video", &["sub one".into(), "sub two".into()]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["url", "sub one", "sub two"]
        );
    }

    #[test]
    fn escaped_placeholders_are_literal_and_do_not_request_subtitles() {
        let template = PlayerTemplate::parse(
            "mpv --label={{subtitle}} --title='${{media-title}}' --empty='' -- {url}",
        )
        .unwrap();
        assert!(!template.has_subtitles);
        let command = template.command("url", "Video", &[]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--label={subtitle}",
                "--title=${media-title}",
                "--empty=",
                "--",
                "url"
            ]
        );
    }

    #[test]
    fn rejects_malformed_templates_before_launch_or_network() {
        for template in [
            "",
            "''",
            "mpv '",
            "mpv \\",
            "mpv {unknown}",
            "mpv {title",
            "mpv title}",
            "{url}",
            "mpv {url}{subtitle}",
            "mpv --sub-files={subtitles}",
            "mpv\0",
        ] {
            assert!(PlayerTemplate::parse(template).is_err(), "{template:?}");
        }
    }

    #[test]
    fn template_roundtrips_through_config() {
        use crate::config::TuiConfig;
        let config: TuiConfig =
            toml::from_str(r#"player = "mpv --title={title} --sub-file={subtitle} -- {url}""#)
                .unwrap();
        let restored: TuiConfig = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(config.player, restored.player);
    }
}
