---
title: Configuration
section: guide
order: 3
---


All configuration files live under `~/.config/pikpaktui/`.

## Credentials — `login.toml`

Stores your PikPak account credentials. Created automatically on first login (via TUI or `pikpaktui login`).

```toml
username = "you@example.com"
password = "your-password"
```

You can also set credentials via environment variables for the `login` command:

```bash
PIKPAK_USER=you@example.com PIKPAK_PASS=yourpassword pikpaktui login
```

:::callout[warning]{kind="warn"}
Credentials are stored in plain text. Ensure `~/.config/pikpaktui/` has appropriate permissions (`chmod 700`).
:::

## TUI & CLI Settings — `config.toml`

The main settings file. Edit manually or use the in-TUI settings panel (`,` to open, `s` to save).
The scalar settings are top-level TOML keys; do not wrap them in a `[tui]` table.

```toml
# UI
nerd_font = false           # Nerd Font icons in TUI (requires a Nerd Font terminal)
border_style = "thick"      # "rounded" | "thick" | "thick-rounded" | "double"
color_scheme = "vibrant"    # "vibrant" | "classic" | "custom"
show_help_bar = true        # Bottom keybinding hint bar
quota_bar_style = "bar"     # "bar" (visual bar) | "percent" (numeric %)

# Preview
show_preview = true         # Include preview when width allows; false = browser panes only
lazy_preview = false        # Only load preview when cursor stops moving
preview_max_size = 65536    # Max bytes loaded for text preview (default: 64 KB)
thumbnail_mode = "auto"     # "auto" | "off" | "force-color" | "force-grayscale"
thumbnail_size = "medium"   # "small" | "medium" | "large"

# Sort (persisted when changed with S / R in TUI)
sort_field = "name"         # "name" | "size" | "created" | "type" | "extension" | "none"
sort_reverse = false

# Interface
move_mode = "picker"        # "picker" (two-pane GUI) | "input" (text input with tab-completion)
cli_nerd_font = false       # Nerd Font icons in CLI output

# Playback
player = "mpv"              # External video player command; set in TUI on first video play

# Downloads
download_jobs = 1           # TUI download workers (1–16 in the settings UI)
update_check = "notify"     # "notify" | "quiet" | "off"
```

### Playback title and subtitles

The `player` command supports argument templates, so you can use your player's own
option names. For mpv, set this top-level key in `config.toml`:

```toml
player = "mpv --fullscreen --title={title} --sub-file={subtitle}"
```

Templates apply to CLI playback and both TUI playback modes (Enter and `w`).

| Placeholder | Expansion |
|-------------|-----------|
| `{title}` | Video filename without its final extension, e.g. `VideoX` for `VideoX.mkv` |
| `{url}` | The selected video stream URL, at this position in the command |
| `{subtitle}` | One matching subtitle URL; the **entire argument** repeats for every match and is omitted when there are no matches |

For example, `--sub-file={subtitle}` becomes two separate `--sub-file=URL`
arguments when two subtitles match. Use your player's repeatable option with `=`;
`--sub-file {subtitle}` would only repeat the URL, not the preceding option.
A standalone `{subtitle}` passes each URL as a separate positional argument for
players or wrappers that accept separate file arguments.

Without `{url}`, pikpaktui appends `--` and the video URL, preserving plain commands
such as `player = "mpv"`. If the last argument is already `--`, it is not duplicated.
With `{url}`, the template controls the argument order and separators completely:

```toml
player = "mpv --title={title} --sub-file={subtitle} -- {url}"
```

Subtitles are files in the same cloud
folder named `VideoX.ass`, `VideoX.Y.srt`, etc. Supported extensions are `.srt`,
`.ass`, `.ssa`, and `.vtt` (case-insensitive); filename stems match exactly.
Subtitle files are looked up only when the template contains a subtitle placeholder.
No local subtitle download is required. A failed lookup reports an error instead
of starting incomplete playback.

Commands use shell-style single/double quotes and backslash escaping to group
arguments, but run directly **without a shell**. Variables, pipes, redirects, and
command substitutions are not evaluated. Arguments are parsed before placeholders
are expanded, so spaces, quotes, and metacharacters in titles or URLs remain data
inside a single argument. Substituted values are never expanded again.

Quote executable paths containing spaces. On Windows, forward slashes avoid
backslash escaping; a TOML literal string makes nested quotes easy:

```toml
player = '"C:/Program Files/mpv/mpv.exe" --title={title} --sub-file={subtitle}'
```

Use `{{` and `}}` for literal braces (e.g. `--title=${{media-title}}` to pass mpv's
own property expression). Unknown placeholders and malformed quotes produce an
error. Placeholders are allowed only in arguments, not in the executable name.

The mpv example uses its [`--title`](https://mpv.io/manual/master/#options-title)
and [`--sub-file`](https://mpv.io/manual/master/#options-sub-file) options; other
players can use different flags with the same placeholders.

### update_check

Controls update checking behavior.

- `"notify"` (default) — Check for updates on startup; show persistently in TUI status bar and CLI stderr
- `"quiet"` — Check silently; only show in TUI log
- `"off"` — Disable update checking entirely

```toml
update_check = "notify"
```

### Image Protocols

Configure the image rendering protocol per terminal emulator, keyed by the `$TERM_PROGRAM` environment variable. Detected automatically — entries are added the first time each terminal is used.

```toml
[image_protocols]
ghostty = "kitty"
"iTerm.app" = "iterm2"
WezTerm = "auto"
```

Supported values: `"auto"` (detect), `"kitty"`, `"iterm2"`, `"sixel"`.

### Custom Colors

Used when `color_scheme = "custom"`. Each value is an `[R, G, B]` array (0–255).

```toml
[custom_colors]
folder   = [92, 176, 255]   # Light blue
archive  = [255, 102, 102]  # Light red
image    = [255, 102, 255]  # Light magenta
video    = [102, 255, 255]  # Light cyan
audio    = [0, 255, 255]    # Cyan
document = [102, 255, 102]  # Light green
code     = [255, 255, 102]  # Light yellow
default  = [255, 255, 255]  # White
```

You can edit custom colors in the TUI: open Settings (`,`), select **Color Scheme**, press `Enter` to enter the custom color editor, then use `r` / `g` / `b` to edit each RGB component.

## Auto-managed Files

These are maintained automatically. Do not edit manually.

| File | Description |
|------|-------------|
| `session.json` | Access and refresh tokens (auto-refreshed) |
| `downloads.json` | Incomplete download state — survives restarts |

## Environment Variables

These override config file values. Useful for CI or per-session overrides.

| Variable | Description |
|----------|-------------|
| `PIKPAK_USER` | Account email (for `pikpaktui login`) |
| `PIKPAK_PASS` | Account password (for `pikpaktui login`) |
| `PIKPAK_DRIVE_BASE_URL` | Override PikPak drive API endpoint |
| `PIKPAK_AUTH_BASE_URL` | Override PikPak auth API endpoint |
| `PIKPAK_CLIENT_ID` | Override OAuth client ID |
| `PIKPAK_CLIENT_SECRET` | Override OAuth client secret |
| `PIKPAK_CAPTCHA_TOKEN` | CAPTCHA token if login is challenged |

:::callout[Concurrent downloads]{kind="info"}
`download_jobs` controls the TUI download queue only; CLI folder downloads use `download -j <n>`. Values between 2–4 are typical, and the TUI settings editor offers 1–16.
:::
