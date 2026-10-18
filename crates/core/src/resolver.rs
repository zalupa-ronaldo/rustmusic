use crate::{Session, valid_video_id};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, Semaphore};

pub fn tool_path(name: &str) -> PathBuf {
    locate_tool(name, &ToolLookup::discover(name)).unwrap_or_else(|| PathBuf::from(name))
}
struct ToolLookup {
    override_path: Option<PathBuf>,
    bundle_dirs: Vec<PathBuf>,
    path_dirs: Vec<PathBuf>,
    known_dirs: Vec<PathBuf>,
}
impl ToolLookup {
    fn discover(name: &str) -> Self {
        let variable = format!("RUSTMUSIC_{}", name.replace('-', "_").to_uppercase());
        Self {
            override_path: std::env::var_os(variable).map(PathBuf::from),
            bundle_dirs: bundle_dirs(),
            path_dirs: path_dirs(&std::env::var("PATH").unwrap_or_default()),
            known_dirs: known_tool_dirs(),
        }
    }
}
fn tool_file_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}
fn locate_tool(name: &str, lookup: &ToolLookup) -> Option<PathBuf> {
    if let Some(path) = &lookup.override_path {
        return Some(executable_path(path).unwrap_or_else(|| path.clone()));
    }
    let file_name = tool_file_name(name);
    lookup
        .bundle_dirs
        .iter()
        .chain(&lookup.path_dirs)
        .chain(&lookup.known_dirs)
        .map(|directory| directory.join(&file_name))
        .find_map(|path| executable_path(&path))
}
fn executable_path(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode();
        if mode & 0o111 == 0 {
            return None;
        }
    }
    Some(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}
fn bundle_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("../Resources/bin"));
        dirs.push(parent.join("bin"));
    }
    dirs
}
fn path_dirs(path: &str) -> Vec<PathBuf> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    path.split(separator)
        .filter(|part| !part.is_empty())
        .map(PathBuf::from)
        .collect()
}
fn known_tool_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        let home = PathBuf::from(home);
        dirs.push(home.join(".deno/bin"));
        dirs.push(home.join(".local/bin"));
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}
fn enrich_path(current: &str, prepend: &[PathBuf]) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let mut seen = std::collections::HashSet::new();
    let mut parts = Vec::new();
    for directory in prepend {
        let text = directory.to_string_lossy().into_owned();
        if text.is_empty() || !seen.insert(text.clone()) {
            continue;
        }
        parts.push(text);
    }
    for part in current.split(separator) {
        if part.is_empty() || !seen.insert(part.to_owned()) {
            continue;
        }
        parts.push(part.to_owned());
    }
    parts.join(&separator.to_string())
}
fn child_path(program: &Path, deno: Option<&Path>, inherited: &str) -> String {
    let mut prefix = Vec::new();
    if let Some(parent) = program.parent()
        && !parent.as_os_str().is_empty()
    {
        prefix.push(parent.to_path_buf());
    }
    if let Some(deno) = deno
        && let Some(parent) = deno.parent()
        && !parent.as_os_str().is_empty()
    {
        prefix.push(parent.to_path_buf());
    }
    prefix.extend(bundle_dirs());
    prefix.extend(known_tool_dirs());
    enrich_path(inherited, &prefix)
}
fn js_runtime_args(deno: Option<&Path>) -> Vec<String> {
    match deno {
        Some(path) => vec!["--js-runtimes".into(), format!("deno:{}", path.display())],
        None => Vec::new(),
    }
}
fn format_selector(browser: bool) -> &'static str {
    if browser {
        "bestaudio[protocol^=http][ext=m4a]/bestaudio[protocol^=http][ext=webm]/bestaudio[protocol^=http]/bestaudio[ext=m4a]/bestaudio"
    } else {
        "bestaudio[protocol^=http]/bestaudio"
    }
}
#[derive(Clone)]
pub struct Stream {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub mime: String,
    pub expires: Instant,
}
impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("mime", &self.mime)
            .finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub struct Resolver {
    session: Option<Session>,
    cache: Arc<Mutex<HashMap<String, Stream>>>,
    gate: Arc<Semaphore>,
}
impl Resolver {
    pub fn new(session: Option<Session>) -> Self {
        Self {
            session,
            cache: Arc::new(Mutex::new(HashMap::new())),
            gate: Arc::new(Semaphore::new(1)),
        }
    }
    pub async fn resolve(&self, id: &str, browser: bool) -> Result<Stream> {
        if !valid_video_id(id) {
            bail!("Некорректный идентификатор трека");
        }
        let key = format!("{id}:{browser}");
        if let Some(stream) = self
            .cache
            .lock()
            .await
            .get(&key)
            .filter(|s| s.expires > Instant::now())
            .cloned()
        {
            return Ok(stream);
        }
        let _permit = self.gate.acquire().await?;
        if let Some(stream) = self
            .cache
            .lock()
            .await
            .get(&key)
            .filter(|s| s.expires > Instant::now())
            .cloned()
        {
            return Ok(stream);
        }
        let cookies = self
            .session
            .as_ref()
            .map(Session::cookie_file)
            .transpose()?;
        let program = tool_path("yt-dlp");
        let deno = locate_tool("deno", &ToolLookup::discover("deno"));
        let mut command = tokio::process::Command::new(&program);
        command
            .env(
                "PATH",
                child_path(
                    &program,
                    deno.as_deref(),
                    &std::env::var("PATH").unwrap_or_default(),
                ),
            )
            .args([
                "--ignore-config",
                "--no-playlist",
                "--dump-single-json",
                "--skip-download",
                "--socket-timeout",
                "20",
                "--retries",
                "2",
                "--format",
                format_selector(browser),
            ]);
        if let Some(file) = &cookies {
            command.arg("--cookies").arg(file.path());
        }
        command.args(js_runtime_args(deno.as_deref()));
        command
            .arg(format!("https://www.youtube.com/watch?v={id}"))
            .kill_on_drop(true);
        let output =
            tokio::time::timeout(Duration::from_secs(70), crate::helper::output(&mut command))
                .await
                .context("Время получения потока истекло")?
                .context("Не найден yt-dlp. Используйте полную сборку RustMusic.")?;
        if !output.status.success() || runtime_missing(&output.stderr) {
            bail!(public_tool_error(&output.stderr));
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| anyhow::anyhow!(public_tool_error(&output.stderr)))?;
        let url = value["url"]
            .as_str()
            .context("Аудиопоток отсутствует")?
            .to_owned();
        validate_stream_url(&url)?;
        let mut headers = HashMap::new();
        if let Some(object) = value["http_headers"].as_object() {
            for (key, value) in object {
                if let Some(text) = value.as_str()
                    && forward_header(key)
                {
                    headers.insert(key.clone(), text.into());
                }
            }
        }
        let mime = match value["ext"].as_str().unwrap_or("") {
            "m4a" => "audio/mp4",
            "webm" => "audio/webm",
            _ => "application/octet-stream",
        }
        .into();
        let ttl = stream_ttl(&url);
        let stream = Stream {
            url,
            headers,
            mime,
            expires: Instant::now() + ttl,
        };
        if !ttl.is_zero() {
            let mut cache = self.cache.lock().await;
            remember_stream(&mut cache, key, stream.clone());
        }
        Ok(stream)
    }
    pub async fn invalidate(&self, id: &str) {
        let mut cache = self.cache.lock().await;
        drop_cached(&mut cache, id);
    }
}
const MEMORY_CACHE_LIMIT: usize = 16;
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
fn stream_ttl(url: &str) -> Duration {
    stream_ttl_at(url, unix_now())
}
fn stream_ttl_at(url: &str, now: u64) -> Duration {
    let Ok(parsed) = url::Url::parse(url) else {
        return Duration::from_secs(240);
    };
    let Some(expire) = parsed
        .query_pairs()
        .find(|(key, _)| key == "expire")
        .and_then(|(_, value)| value.parse::<u64>().ok())
    else {
        return Duration::from_secs(240);
    };
    let remain = expire.saturating_sub(30).saturating_sub(now);
    if remain == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(remain.min(15 * 60))
    }
}
fn remember_stream(cache: &mut HashMap<String, Stream>, key: String, stream: Stream) {
    if stream.expires <= Instant::now() {
        return;
    }
    cache.retain(|_, cached| cached.expires > Instant::now());
    while cache.len() >= MEMORY_CACHE_LIMIT {
        let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, cached)| cached.expires)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&oldest);
    }
    cache.insert(key, stream);
}
fn drop_cached(cache: &mut HashMap<String, Stream>, id: &str) {
    if !valid_video_id(id) {
        return;
    }
    cache.remove(&format!("{id}:false"));
    cache.remove(&format!("{id}:true"));
}
fn forward_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "user-agent" | "referer" | "origin" | "accept" | "accept-language"
    )
}
fn public_tool_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let mut kept = Vec::new();
    for line in text.lines() {
        if line_has_secret(line) {
            continue;
        }
        let redacted = redact_query(line);
        let redacted = redacted.trim();
        if redacted.is_empty() {
            continue;
        }
        kept.push(redacted.to_owned());
        if kept.len() == 3 {
            break;
        }
    }
    let detail: String = kept.join(" ").chars().take(280).collect();
    if detail.is_empty() {
        "Не удалось получить аудиопоток. Трек может быть недоступен; проверьте сессию или обновите yt-dlp.".into()
    } else {
        format!("Не удалось получить аудиопоток. {detail}")
    }
}
fn runtime_missing(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr)
        .to_ascii_lowercase()
        .contains("no supported javascript runtime")
}
fn line_has_secret(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("cookie")
        || lower.contains("sapisid")
        || lower.contains("authorization")
        || lower.contains("__secure-")
        || lower.contains("login_info")
}
fn redact_query(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        out.push(ch);
        if ch == '?' {
            out.push('…');
            while chars.peek().is_some_and(|next| !next.is_whitespace()) {
                chars.next();
            }
        }
    }
    out
}
pub fn validate_stream_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).context("Некорректный адрес потока")?;
    let host = parsed.host_str().unwrap_or("");
    if parsed.scheme() != "https"
        || !host.ends_with(".googlevideo.com")
        || parsed.port().is_some_and(|p| p != 443)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        bail!("Сервис вернул недопустимый адрес потока");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prevents_arbitrary_proxy_targets() {
        for u in [
            "http://r1.googlevideo.com/x",
            "https://googlevideo.com.evil.test/x",
            "https://127.0.0.1/x",
            "https://user@r1.googlevideo.com/x",
        ] {
            assert!(validate_stream_url(u).is_err());
        }
        assert!(validate_stream_url("https://rr1---sn.googlevideo.com/videoplayback?x=1").is_ok());
    }
    #[test]
    fn deno_on_path_is_used_when_working_directory_has_none() {
        let directory = tempfile::tempdir().unwrap();
        let deno = directory.path().join(tool_file_name("deno"));
        std::fs::write(&deno, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&deno, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = locate_tool(
            "deno",
            &ToolLookup {
                override_path: None,
                bundle_dirs: vec![directory.path().join("missing-bundle")],
                path_dirs: vec![directory.path().to_path_buf()],
                known_dirs: vec![],
            },
        )
        .unwrap();
        assert!(found.is_absolute());
        assert_eq!(
            found.file_name().unwrap(),
            std::ffi::OsStr::new(&tool_file_name("deno"))
        );
        let args = js_runtime_args(Some(&found));
        assert_eq!(args[0], "--js-runtimes");
        assert!(args[1].starts_with("deno:"));
        assert!(args[1].contains(&found.display().to_string()));
        assert!(!args[1].contains(".."));
    }
    #[cfg(unix)]
    #[test]
    fn non_executable_deno_is_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("blocked");
        let allowed = directory.path().join("allowed");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::create_dir(&allowed).unwrap();
        std::fs::write(blocked.join("deno"), b"nope").unwrap();
        std::fs::write(allowed.join("deno"), b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(allowed.join("deno"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let found = locate_tool(
            "deno",
            &ToolLookup {
                override_path: None,
                bundle_dirs: vec![],
                path_dirs: vec![blocked, allowed.clone()],
                known_dirs: vec![],
            },
        )
        .unwrap();
        assert!(found.starts_with(std::fs::canonicalize(&allowed).unwrap()));
    }
    #[test]
    fn finder_path_gains_bundled_and_known_directories() {
        let separator = if cfg!(windows) { ';' } else { ':' };
        let inherited = format!("/usr/bin{separator}/bin");
        let path = enrich_path(
            &inherited,
            &[
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/Applications/RustMusic.app/Contents/Resources/bin"),
                PathBuf::from("/usr/bin"),
            ],
        );
        assert!(path.starts_with(&format!("/opt/homebrew/bin{separator}")));
        assert!(path.contains("Resources/bin"));
        assert_eq!(path.matches("/usr/bin").count(), 1);
        assert!(path.ends_with(&format!("{separator}/bin")));
    }
    #[test]
    fn stream_ttl_follows_expire_and_dead_urls_are_not_cached() {
        assert_eq!(
            stream_ttl_at(
                "https://rr1---sn.googlevideo.com/videoplayback?expire=100",
                90
            ),
            Duration::ZERO
        );
        assert_eq!(
            stream_ttl_at(
                "https://rr1---sn.googlevideo.com/videoplayback?expire=100",
                50
            ),
            Duration::from_secs(20)
        );
        assert_eq!(
            stream_ttl_at(
                "https://rr1---sn.googlevideo.com/videoplayback?expire=9999999999",
                1_000
            ),
            Duration::from_secs(15 * 60)
        );
        assert_eq!(
            stream_ttl_at("https://rr1---sn.googlevideo.com/videoplayback?x=1", 0),
            Duration::from_secs(240)
        );
        let mut cache = HashMap::new();
        remember_stream(
            &mut cache,
            "abcdefghijk:false".into(),
            Stream {
                url: "https://rr1---sn.googlevideo.com/videoplayback?expire=1".into(),
                headers: HashMap::new(),
                mime: "audio/mp4".into(),
                expires: Instant::now(),
            },
        );
        assert!(cache.is_empty());
    }
    #[test]
    fn invalidate_drops_only_that_track() {
        let mut cache = HashMap::new();
        for id in ["aaaaaaaaaaa", "bbbbbbbbbbb"] {
            remember_stream(
                &mut cache,
                format!("{id}:false"),
                Stream {
                    url: "https://rr1---sn.googlevideo.com/videoplayback?x=1".into(),
                    headers: HashMap::new(),
                    mime: "audio/mp4".into(),
                    expires: Instant::now() + Duration::from_secs(60),
                },
            );
        }
        drop_cached(&mut cache, "aaaaaaaaaaa");
        assert!(!cache.contains_key("aaaaaaaaaaa:false"));
        assert!(cache.contains_key("bbbbbbbbbbb:false"));
        drop_cached(&mut cache, "../etc/passwd");
        assert!(cache.contains_key("bbbbbbbbbbb:false"));
    }
    #[test]
    fn resolver_failure_shows_the_reason_without_cookies() {
        let stderr = b"ERROR: No supported JavaScript runtime could be found\n__Secure-3PAPISID=fixture-secret\nCookie: SID=secret\nhttps://rr.googlevideo.com/videoplayback?expire=1&sig=secret\n";
        let message = public_tool_error(stderr);
        assert!(message.contains("JavaScript runtime"));
        assert!(message.contains("googlevideo.com/videoplayback"));
        assert!(!message.contains("fixture-secret"));
        assert!(!message.contains("SID=secret"));
        assert!(!message.contains("sig=secret"));
        assert!(!message.contains("SAPISID"));
        assert!(runtime_missing(
            b"WARNING: [youtube] No supported JavaScript runtime could be found."
        ));
        assert!(!runtime_missing(b"ERROR: video unavailable"));
    }
    #[test]
    fn direct_http_audio_is_preferred() {
        assert!(format_selector(false).starts_with("bestaudio[protocol^=http]"));
        assert!(format_selector(true).contains("ext=m4a"));
        assert!(forward_header("User-Agent"));
        assert!(!forward_header("Cookie"));
    }
}
