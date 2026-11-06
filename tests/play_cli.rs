use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    base: String,
    requests: Arc<Mutex<Vec<reqwest::Url>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

fn file_info() -> Value {
    json!({"id":"file-id", "name":"film.mkv", "medias":[
        {"media_name":"720p", "link":{"url":"https://example.invalid/720"}},
        {"is_origin":true, "link":{"url":"https://example.invalid/original"}, "video":{"height":2160}},
        {"media_name":"Full HD", "video":{"height":1080}, "link":{"url":"https://example.invalid/1080"}},
        {"media_name":"4K", "need_more_quota":true, "link":{"url":"https://example.invalid/blocked"}}
    ]})
}

impl Fixture {
    fn new(info: Value, events: Value, player: bool) -> Self {
        static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);
        let root = loop {
            let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("pikpaktui-play-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!(
                    "cannot allocate test directory '{}': {error}",
                    path.display()
                ),
            }
        };
        let mut settings = json!({"update_check":"off", "playback_quality":"720p"});
        if player {
            // A harmless child prints its arguments, allowing selection and
            // title expansion to be checked without a media player or network.
            settings["player"] = json!(if cfg!(windows) {
                "cmd.exe /d /c echo"
            } else {
                "echo"
            });
            settings["player_args"] = json!([
                "--force-media-title={name}",
                "--cloud-path={path}",
                "--cloud-id={id}"
            ]);
        }
        std::fs::write(
            root.join("config.toml"),
            toml::to_string(&settings).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("session.json"), json!({
            "access_token":"test-access", "refresh_token":"test-refresh", "expires_at_unix":4102444800_i64
        }).to_string()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let captured = Arc::clone(&requests);
        let stopped = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut buffer = [0; 4096];
                    let length = stream.read(&mut buffer).unwrap();
                    assert_ne!(length, 0, "request ended before headers");
                    bytes.extend_from_slice(&buffer[..length]);
                }
                let headers = String::from_utf8_lossy(&bytes);
                let mut first = headers.lines().next().unwrap().split_whitespace();
                assert_eq!(first.next(), Some("GET"));
                let url = reqwest::Url::parse(&format!("http://fixture{}", first.next().unwrap()))
                    .unwrap();
                let body = match url.path() {
                    "/drive/v1/events" => events.clone(),
                    "/drive/v1/files" => {
                        json!({"files":[{"id":"file-id","name":"film.mkv","kind":"drive#file"}]})
                    }
                    "/drive/v1/files/file-id" | "/drive/v1/files/latest-id" => info.clone(),
                    path => panic!("unexpected API request: {path}"),
                }
                .to_string();
                captured.lock().unwrap().push(url);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        Self {
            root,
            base,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pikpaktui"));
        command
            .arg("play")
            .args(args)
            .env_clear()
            .env("PIKPAKTUI_CONFIG_DIR", &self.root)
            .env("HOME", &self.root)
            .env("USERPROFILE", &self.root)
            .env("APPDATA", &self.root)
            .env("LOCALAPPDATA", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join(".config"))
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("PIKPAK_DRIVE_BASE_URL", &self.base)
            .env("PIKPAK_AUTH_BASE_URL", &self.base);
        #[cfg(windows)]
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.output().unwrap()
    }

    fn success(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let joined = self.handle.take().unwrap().join();
        std::fs::remove_dir_all(&self.root).unwrap();
        if !thread::panicking() {
            joined.unwrap();
        }
    }
}

#[test]
fn default_plays_original_despite_tui_preference_and_positional_quality_still_works() {
    let fixture = Fixture::new(file_info(), json!({"events":[]}), true);
    for (args, url) in [
        (vec!["/film.mkv"], "https://example.invalid/original"),
        (vec!["/film.mkv", "1080"], "https://example.invalid/1080"),
        (vec!["/film.mkv", "1080p"], "https://example.invalid/1080"),
        (vec!["/film.mkv", "2"], "https://example.invalid/720"),
    ] {
        let output = fixture.success(&args);
        assert!(output.contains(url), "{output}");
        assert!(output.contains("--force-media-title=film.mkv"), "{output}");
        assert!(output.contains("--cloud-path=/film.mkv"), "{output}");
        assert!(output.contains("--cloud-id=file-id"), "{output}");
    }
}

#[test]
fn listing_does_not_require_a_player_and_includes_unavailable_streams() {
    let fixture = Fixture::new(file_info(), json!({"events":[]}), false);
    for flag in ["--list-qualities", "-l"] {
        let output = fixture.success(&["/film.mkv", flag]);
        assert!(output.contains("1. Original"), "{output}");
        assert!(output.contains("2. 720p"), "{output}");
        assert!(output.contains("Full HD"), "{output}");
        assert!(
            output.contains("additional playback quota required"),
            "{output}"
        );
        assert!(!output.contains("https://"), "{output}");
    }
}

#[test]
fn latest_searches_only_the_events_first_page_and_preserves_file_context() {
    let fixture = Fixture::new(
        file_info(),
        json!({"next_page_token":"unused", "events":[
            {"type":"TYPE_PLAY", "file_id":"not-an-addition", "file_name":"played.mp4"},
            {"type":"TYPE_UPLOAD", "file_id":"document", "file_name":"notes.txt"},
            {"type":"TYPE_RESTORE", "id":"event-id", "file_id":"latest-id", "file_name":"old name.mkv",
             "reference_resource":{"id":"latest-id", "kind":"drive#file", "name":"old name.mkv", "mime_type":"video/x-matroska"}},
            {"type":"TYPE_UPLOAD", "file_id":"older-id", "file_name":"older.mp4"}
        ]}),
        true,
    );
    for (args, url) in [
        (vec!["--latest"], "https://example.invalid/original"),
        (vec!["--latest", "720p"], "https://example.invalid/720"),
    ] {
        let output = fixture.success(&args);
        assert!(output.contains(url), "{output}");
        assert!(output.contains("--force-media-title=film.mkv"), "{output}");
        assert!(
            output.contains("--cloud-path= --cloud-id=latest-id"),
            "{output}"
        );
    }
    assert!(
        fixture
            .success(&["--latest", "-l"])
            .contains("Available qualities for 'film.mkv'")
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    for pair in requests.as_chunks::<2>().0 {
        assert_eq!(pair[0].path(), "/drive/v1/events");
        assert!(
            pair[0]
                .query_pairs()
                .any(|(key, value)| key == "limit" && value == "20")
        );
        assert!(!pair[0].query_pairs().any(|(key, _)| key == "page_token"));
        assert_eq!(pair[1].path(), "/drive/v1/files/latest-id");
    }
}

#[test]
fn latest_listing_without_a_player_and_no_video_errors_are_explicit() {
    let fixture = Fixture::new(
        file_info(),
        json!({"events":[
            {"type":"TYPE_UPLOAD", "file_id":"latest-id", "file_name":"film.mkv"}
        ]}),
        false,
    );
    assert!(
        fixture
            .success(&["--latest", "--list-qualities"])
            .contains("Original")
    );

    let empty = Fixture::new(
        file_info(),
        json!({"events":[], "next_page_token":"do-not-follow"}),
        true,
    );
    let output = empty.run(&["--latest"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("first 20 events"));
    assert_eq!(empty.requests.lock().unwrap().len(), 1);
}

#[test]
fn original_is_required_by_default_and_restrictions_are_not_bypassed() {
    let mut blocked = file_info();
    blocked["medias"][1]["need_more_quota"] = json!(true);
    let mut missing = file_info();
    missing["medias"].as_array_mut().unwrap().remove(1);
    for (info, error) in [
        (blocked, "additional playback quota required"),
        (missing, "no stream matching 'original'"),
    ] {
        let fixture = Fixture::new(info, json!({"events":[]}), true);
        let output = fixture.run(&["/film.mkv"]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
        assert!(
            fixture
                .success(&["/film.mkv", "720p"])
                .contains("https://example.invalid/720")
        );
    }
}

#[test]
fn help_and_invalid_arguments_do_not_access_the_api() {
    let fixture = Fixture::new(file_info(), json!({"events":[]}), false);
    let help = fixture.success(&["--help"]);
    assert!(help.contains("--latest"));
    assert!(help.contains("--list-qualities"));
    assert!(help.contains("original (default)"));
    assert!(!help.contains("--quality"));
    for args in [
        vec![],
        vec!["--quality", "1080"],
        vec!["--latest", "/film.mkv"],
        vec!["/film.mkv", "-l", "1080"],
    ] {
        let output = fixture.run(&args);
        assert!(!output.status.success(), "accepted {args:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("no player configured"));
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
}
