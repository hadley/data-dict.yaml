//! `render-spec --live` and `render-report --live`: serve the page from memory
//! and reload the browser whenever anything it was built from changes.
//!
//! The server is written on `std::net` rather than pulled from a crate: the
//! only client is a browser on loopback and the surface is three GETs — the
//! page, the event stream it subscribes to, and the diagnostics it fetches.
//! Changes are found by polling modification times, because the files a build
//! reads are few and known: the dictionary, each table's source data, and (for
//! [`Assets::Dir`]) the page's own CSS and JS.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use data_dict::{Level, RenderStyle, Run};

use crate::assets::{Assets, LivePage, embed_json, escape_embedded};

/// Where the browser is sent first. Fixed, so a pinned tab survives a restart.
const DEFAULT_PORT: u16 = 7590;
/// How many ports past [`DEFAULT_PORT`] to try, so a second `--live` on the
/// same machine still starts.
const PORT_TRIES: u16 = 10;
/// How often the watched files are checked for a new modification time.
const POLL: Duration = Duration::from_millis(200);
/// How long to wait after spotting a change, so a burst of writes rebuilds
/// once rather than once per file.
const SETTLE: Duration = Duration::from_millis(60);
/// How often a comment frame goes out on an idle stream, so a browser that has
/// gone away is dropped rather than held forever.
const HEARTBEAT: Duration = Duration::from_secs(15);
/// How long a client has to send its request line before it is abandoned.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A comment frame: ignored by the browser, but a write that fails identifies
/// a stream whose other end is gone.
const HEARTBEAT_FRAME: &str = ":\n\n";

/// What the server is currently serving.
struct Page {
    html: String,
    /// The same export the page was built around, served on its own so the
    /// browser can take a new dictionary without reloading.
    json: String,
    /// The page's stylesheet, served on its own so a CSS edit can be swapped
    /// in without reloading.
    css: String,
    /// Whether the last build failed, leaving `html` as the last good page.
    failed: bool,
    /// The last build's diagnostics, rendered for display.
    text: Vec<String>,
}

/// Everything the accept loop and the watch loop share.
struct Session {
    dict: PathBuf,
    assets: Assets,
    /// Which page the server builds and serves.
    kind: LivePage,
    page: Mutex<Page>,
    clients: Mutex<Vec<TcpStream>>,
}

impl Session {
    /// Every file whose modification time is worth watching. `sources` are the
    /// current table sources, which are only known from a build that worked.
    fn watch_list(&self, sources: &[PathBuf]) -> Vec<PathBuf> {
        let mut paths = vec![self.dict.clone()];
        paths.extend(sources.iter().cloned());
        paths.extend(self.assets.files());
        paths
    }

    /// Hold a connection open as an event stream. The read timeout is cleared
    /// because nothing is ever read back from it.
    fn subscribe(&self, mut stream: TcpStream) -> std::io::Result<()> {
        stream.set_read_timeout(None)?;
        stream.write_all(
            b"HTTP/1.1 200 OK\r\n\
              Content-Type: text/event-stream\r\n\
              Cache-Control: no-store\r\n\
              Connection: keep-alive\r\n\r\n",
        )?;
        stream.flush()?;
        self.clients.lock().unwrap().push(stream);
        Ok(())
    }

    /// Write one frame to every open stream, dropping those that have gone.
    fn broadcast(&self, frame: &str) {
        self.clients.lock().unwrap().retain_mut(|client| {
            client
                .write_all(frame.as_bytes())
                .and_then(|()| client.flush())
                .is_ok()
        });
    }
}

/// One build's outcome.
struct Build {
    /// The page and the export it was built around. `None` when the dictionary
    /// stopped validating or the page's assets couldn't be read; the last good
    /// page keeps serving.
    page: Option<(String, String, String)>,
    /// Each table's source file, resolved against the dictionary's directory.
    /// `None` when the export failed, where the previous list stands.
    sources: Option<Vec<PathBuf>>,
    failed: bool,
    text: Vec<String>,
}

/// Export the dictionary and render the page around it.
fn build(dict: &Path, assets: &Assets, kind: &LivePage) -> Build {
    match kind {
        LivePage::Dict => build_dict(dict, assets),
        LivePage::Report { level, table } => build_report(dict, assets, *level, table.as_deref()),
    }
}

fn build_dict(dict: &Path, assets: &Assets) -> Build {
    let (problems, export) = data_dict::export_auto(dict);
    let mut text = problems.render(RenderStyle::default());
    let Some(export) = export else {
        return Build {
            page: None,
            sources: None,
            failed: true,
            text,
        };
    };
    let sources = source_paths(dict, Some(&export));
    match assets.render_dict_page(&embed_json(&export), true) {
        Ok(html) => Build {
            // The page's own copy is escaped for its `<script>` block; the one
            // served on its own is the plain document.
            page: Some((
                html,
                serde_json::to_string(&export).expect("an export always serializes"),
                assets.css(&LivePage::Dict).unwrap_or_default(),
            )),
            sources: Some(sources),
            failed: problems.status().failed(),
            text,
        },
        Err(err) => {
            text.push(format!("could not read the page's assets: {err}"));
            Build {
                page: None,
                sources: Some(sources),
                failed: true,
                text,
            }
        }
    }
}

/// Validate the dictionary's data and render the report around the run. A
/// run that couldn't start has no report to give, so — like a dictionary that
/// stopped validating — it keeps the last good page and reports over it.
fn build_report(dict: &Path, assets: &Assets, level: Level, table: Option<&str>) -> Build {
    let problems = match level {
        Level::Meta => data_dict::validate_meta(dict, table),
        _ => data_dict::validate_data(dict, table),
    };
    let mut text = problems.render(RenderStyle::default());
    if problems.preflight().is_some() {
        return Build {
            page: None,
            sources: None,
            failed: true,
            text,
        };
    }
    let run = Run::new(dict, level, table);
    let json = serde_json::to_string(&problems.report(run)).expect("a report always serializes");
    let source = embed_json(&problems.source_text().unwrap_or_default());
    let kind = LivePage::Report {
        level,
        table: table.map(str::to_string),
    };
    match assets.render_report_page(&escape_embedded(&json), &source, true) {
        Ok(html) => Build {
            page: Some((html, json, assets.css(&kind).unwrap_or_default())),
            sources: Some(source_paths(dict, None)),
            failed: problems.status().failed(),
            text,
        },
        Err(err) => {
            text.push(format!("could not read the page's assets: {err}"));
            Build {
                page: None,
                sources: Some(source_paths(dict, None)),
                failed: true,
                text,
            }
        }
    }
}

/// Each table's source file, resolved against the dictionary's directory.
/// `export` reuses one already in hand; otherwise the paths are read with a
/// spec-only export, which never touches the data files themselves.
fn source_paths(dict: &Path, export: Option<&data_dict::Export>) -> Vec<PathBuf> {
    let base = dict.parent().unwrap_or_else(|| Path::new(""));
    let owned;
    let export = match export {
        Some(export) => export,
        None => {
            owned = data_dict::export_spec(dict).1;
            match &owned {
                Some(export) => export,
                None => return Vec::new(),
            }
        }
    };
    export
        .source_paths()
        .into_iter()
        .map(|path| base.join(path))
        .collect()
}

/// Serve the dictionary at `dict` until interrupted.
pub fn run(dict: &Path, port: Option<u16>, assets: Assets) -> ExitCode {
    serve(dict, LivePage::Dict, port, assets)
}

/// Serve the validation report for the dictionary at `dict` until interrupted.
pub fn run_report(
    dict: &Path,
    level: Level,
    table: Option<String>,
    port: Option<u16>,
    assets: Assets,
) -> ExitCode {
    serve(dict, LivePage::Report { level, table }, port, assets)
}

fn serve(dict: &Path, kind: LivePage, port: Option<u16>, assets: Assets) -> ExitCode {
    let first = build(dict, &assets, &kind);
    for line in &first.text {
        eprintln!("{line}");
    }
    // A page that never built has nothing to serve, so this fails like `render`
    // rather than starting up empty; a later failure keeps the last good page.
    let Some((html, json, css)) = first.page else {
        return ExitCode::FAILURE;
    };
    let listener = match bind(port) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("could not listen on port {}: {err}", port_label(port));
            return ExitCode::FAILURE;
        }
    };
    let url = match listener.local_addr() {
        Ok(addr) => format!("http://{addr}"),
        Err(err) => {
            eprintln!("could not read the server's address: {err}");
            return ExitCode::FAILURE;
        }
    };

    let session = Arc::new(Session {
        dict: dict.to_path_buf(),
        assets,
        kind,
        page: Mutex::new(Page {
            html,
            json,
            css,
            failed: first.failed,
            text: first.text,
        }),
        clients: Mutex::new(Vec::new()),
    });

    println!("serving {} at {url}", dict.display());
    println!("watching for changes — press ctrl-c to stop");
    let accepting = Arc::clone(&session);
    thread::spawn(move || accept(&accepting, listener));
    open_browser(&url);
    watch(&session, first.sources.unwrap_or_default())
}

fn accept(session: &Arc<Session>, listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        // A browser closing a tab mid-request is ordinary, not worth reporting.
        let _ = handle(session, stream);
    }
}

fn handle(session: &Arc<Session>, mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let mut request = String::new();
    BufReader::new(&stream).read_line(&mut request)?;
    match request.split_whitespace().nth(1).unwrap_or("/") {
        "/" => {
            let html = session.page.lock().unwrap().html.clone();
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", &html)
        }
        "/dict.json" => {
            let json = session.page.lock().unwrap().json.clone();
            respond(&mut stream, "200 OK", "application/json", &json)
        }
        "/style.css" => {
            let css = session.page.lock().unwrap().css.clone();
            respond(&mut stream, "200 OK", "text/css; charset=utf-8", &css)
        }
        "/problems" => {
            let body = {
                let page = session.page.lock().unwrap();
                serde_json::json!({ "failed": page.failed, "text": page.text }).to_string()
            };
            respond(&mut stream, "200 OK", "application/json", &body)
        }
        "/events" => session.subscribe(stream),
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n",
        ),
    }
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    )?;
    stream.flush()
}

/// An SSE frame. The `data` line is required: the browser won't dispatch an
/// event whose data buffer is empty, even when the name carries the message.
fn event(name: &str) -> String {
    format!("event: {name}\ndata: 1\n\n")
}

/// Listen on `port`, or on the first free port from [`DEFAULT_PORT`].
fn bind(port: Option<u16>) -> std::io::Result<TcpListener> {
    match port {
        Some(port) => TcpListener::bind((Ipv4Addr::LOCALHOST, port)),
        None => bind_from(DEFAULT_PORT, PORT_TRIES),
    }
}

fn bind_from(start: u16, tries: u16) -> std::io::Result<TcpListener> {
    let mut last = None;
    for port in start..start.saturating_add(tries) {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(listener) => return Ok(listener),
            Err(err) => last = Some(err),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no port was tried")))
}

fn port_label(port: Option<u16>) -> String {
    match port {
        Some(port) => port.to_string(),
        None => format!("{DEFAULT_PORT}–{}", DEFAULT_PORT + PORT_TRIES - 1),
    }
}

/// The modification time of every watched file. A file that isn't there is
/// recorded as `None`, so creating it registers as a change.
fn stamps(paths: &[PathBuf]) -> Vec<Option<SystemTime>> {
    paths
        .iter()
        .map(|path| {
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
        })
        .collect()
}

/// Rebuild and notify the browser whenever a watched file changes.
fn watch(session: &Arc<Session>, sources: Vec<PathBuf>) -> ! {
    let mut sources = sources;
    let mut watched = session.watch_list(&sources);
    let mut seen = stamps(&watched);
    let mut beat = Instant::now();
    loop {
        thread::sleep(POLL);
        let now = stamps(&watched);
        if now != seen {
            thread::sleep(SETTLE);
            let css_only = css_only_change(&session.assets, &session.kind, &watched, &seen, &now);
            let build = build(&session.dict, &session.assets, &session.kind);
            if let Some(found) = &build.sources {
                sources = found.clone();
            }
            apply(session, build, css_only);
            watched = session.watch_list(&sources);
            seen = stamps(&watched);
            beat = Instant::now();
        } else if beat.elapsed() >= HEARTBEAT {
            session.broadcast(HEARTBEAT_FRAME);
            beat = Instant::now();
        }
    }
}

/// Whether every file that changed is a stylesheet. A CSS-only edit swaps the
/// page's styles in place, keeping the scroll position and filter state a
/// reload would throw away.
fn css_only_change(
    assets: &Assets,
    kind: &LivePage,
    watched: &[PathBuf],
    before: &[Option<SystemTime>],
    after: &[Option<SystemTime>],
) -> bool {
    let css = assets.css_files(kind);
    let mut changed = watched
        .iter()
        .zip(before)
        .zip(after)
        .filter(|((_, a), b)| a != b)
        .map(|((path, _), _)| path);
    !css.is_empty() && changed.clone().next().is_some() && changed.all(|path| css.contains(path))
}

/// Publish a build: a page that rendered reloads the browser — or, when only
/// its CSS changed, swaps the stylesheet — and one that didn't reports over
/// the page already there.
fn apply(session: &Arc<Session>, build: Build, css_only: bool) {
    for line in &build.text {
        eprintln!("{line}");
    }
    let rebuilt = build.page.is_some();
    {
        let mut page = session.page.lock().unwrap();
        if let Some((html, json, css)) = build.page {
            page.html = html;
            page.json = json;
            page.css = css;
        }
        page.failed = build.failed;
        page.text = build.text;
    }
    if rebuilt {
        println!("reloaded {}", session.dict.display());
    }
    let name = match (rebuilt, css_only) {
        (true, true) => "css",
        (true, false) => "reload",
        (false, _) => "problems",
    };
    session.broadcast(&event(name));
}

/// Show the page, best-effort: a browser that won't launch is no reason to
/// stop serving, since the URL has already been printed.
fn open_browser(url: &str) {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        Command::new("xdg-open")
    };
    let _ = command.arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn session(html: &str) -> Arc<Session> {
        Arc::new(Session {
            dict: PathBuf::from("data-dict.yaml"),
            assets: Assets::Embedded,
            kind: LivePage::Dict,
            page: Mutex::new(Page {
                html: html.to_string(),
                json: r#"{"tables":[]}"#.to_string(),
                css: "body { color: red }".to_string(),
                failed: false,
                text: Vec::new(),
            }),
            clients: Mutex::new(Vec::new()),
        })
    }

    /// Start a server on an ephemeral port and return where to reach it.
    fn start(session: &Arc<Session>) -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = Arc::clone(session);
        thread::spawn(move || accept(&session, listener));
        port
    }

    fn get(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        stream.flush().unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn the_root_serves_the_current_page() {
        let session = session("<!doctype html><p>hello");
        let response = get(start(&session), "/");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.ends_with("<!doctype html><p>hello"));
    }

    #[test]
    fn problems_are_served_as_json() {
        let session = session("page");
        {
            let mut page = session.page.lock().unwrap();
            page.failed = true;
            page.text = vec!["S07: bad".to_string()];
        }
        let response = get(start(&session), "/problems");
        assert!(response.ends_with(r#"{"failed":true,"text":["S07: bad"]}"#));
    }

    /// The export is served on its own so a rebuilt dictionary can be taken
    /// without reloading, and unescaped: it isn't inside a `<script>` here.
    #[test]
    fn the_export_is_served_on_its_own() {
        let session = session("page");
        session.page.lock().unwrap().json = r#"{"name":"a<b"}"#.to_string();
        let response = get(start(&session), "/dict.json");
        assert!(response.contains("Content-Type: application/json"));
        assert!(response.ends_with(r#"{"name":"a<b"}"#));
    }

    /// The stylesheet is served on its own so a CSS-only edit can be swapped
    /// in without reloading the page.
    #[test]
    fn the_stylesheet_is_served_on_its_own() {
        let session = session("page");
        let response = get(start(&session), "/style.css");
        assert!(response.contains("Content-Type: text/css; charset=utf-8"));
        assert!(response.ends_with("body { color: red }"));
    }

    #[test]
    fn a_change_is_css_only_when_every_changed_file_is_a_stylesheet() {
        let dir = PathBuf::from("render");
        let assets = Assets::Dir(dir.clone());
        let css = dir.join("shared/app.css");
        let js = dir.join("dict/app.js");
        let watched = vec![PathBuf::from("data-dict.yaml"), css.clone(), js.clone()];
        let before = vec![None, None, None];

        let mut after = vec![None, None, None];
        assert!(!css_only_change(
            &assets,
            &LivePage::Dict,
            &watched,
            &before,
            &after
        ));

        after[1] = Some(SystemTime::now());
        assert!(css_only_change(
            &assets,
            &LivePage::Dict,
            &watched,
            &before,
            &after
        ));

        after[2] = Some(SystemTime::now());
        assert!(!css_only_change(
            &assets,
            &LivePage::Dict,
            &watched,
            &before,
            &after
        ));

        // Embedded assets can't change, so no change is ever CSS-only.
        assert!(!css_only_change(
            &Assets::Embedded,
            &LivePage::Dict,
            &watched,
            &before,
            &after
        ));
    }

    #[test]
    fn an_unknown_path_is_not_found() {
        let response = get(start(&session("page")), "/nope");
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }

    #[test]
    fn a_subscriber_is_sent_every_frame() {
        let session = session("page");
        let port = start(&session);
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(stream, "GET /events HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        stream.flush().unwrap();

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "HTTP/1.1 200 OK\r\n");
        // Read to the end of the headers, so what follows is the first frame.
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header == "\r\n" {
                break;
            }
            assert!(!header.is_empty(), "the stream closed inside the headers");
        }

        // The subscriber is registered by the time the headers are out, but the
        // push comes from another thread; retry until it lands.
        for _ in 0..100 {
            if session.clients.lock().unwrap().len() == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        session.broadcast(&event("reload"));

        let mut frame = String::new();
        reader.read_line(&mut frame).unwrap();
        assert_eq!(frame, "event: reload\n");
    }

    #[test]
    fn a_stream_that_has_gone_away_is_dropped() {
        let session = session("page");
        let port = start(&session);
        {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            write!(stream, "GET /events HTTP/1.1\r\n\r\n").unwrap();
            stream.flush().unwrap();
            for _ in 0..100 {
                if session.clients.lock().unwrap().len() == 1 {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(session.clients.lock().unwrap().len(), 1);
        }
        // The first write after the close may still be buffered by the OS, so
        // the drop can take a second frame to be noticed.
        for _ in 0..50 {
            session.broadcast(HEARTBEAT_FRAME);
            if session.clients.lock().unwrap().is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("a closed stream was never dropped");
    }

    #[test]
    fn a_taken_port_falls_through_to_the_next() {
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        let listener = bind_from(port, PORT_TRIES).unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), port);
    }

    #[test]
    fn no_free_port_is_an_error() {
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        assert!(bind_from(port, 1).is_err());
    }

    /// A missing file is a stamp too, so writing one for the first time — a
    /// source that arrives after the dictionary naming it — is a change.
    #[test]
    fn a_file_appearing_registers_as_a_change() {
        let path = std::env::temp_dir().join("data-dict-live-stamp-test.yaml");
        let _ = std::fs::remove_file(&path);
        let watched = vec![path.clone()];
        let before = stamps(&watched);
        assert_eq!(before, vec![None]);
        std::fs::write(&path, "name: x\n").unwrap();
        assert_ne!(stamps(&watched), before);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_watch_list_covers_the_dictionary_its_sources_and_its_assets() {
        let session = session("page");
        let sources = vec![PathBuf::from("data/otters.parquet")];
        assert_eq!(
            session.watch_list(&sources),
            vec![
                PathBuf::from("data-dict.yaml"),
                PathBuf::from("data/otters.parquet")
            ],
            "embedded assets can't change while the process runs"
        );
    }
}
