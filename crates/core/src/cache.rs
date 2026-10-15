//! Disk cache of tracks that have already been played.
use crate::{Stream, resolver::validate_stream_url, valid_video_id};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

const DEFAULT_LIMIT_MB: u64 = 1024;
const MAX_TRACK_BYTES: u64 = 80 * 1024 * 1024;
const ALLOWED_EXT: &[&str] = &["m4a", "webm", "ogg", "mp3", "bin"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheConfig {
    pub directory: PathBuf,
    pub limit_bytes: u64,
}
impl CacheConfig {
    pub fn load() -> Self {
        let stored = fs::read(config_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<StoredConfig>(&bytes).ok());
        let (file_dir, file_mb) = match stored {
            Some(stored) => (stored.cache_dir, stored.cache_limit_mb),
            None => (None, None),
        };
        resolve_config(
            default_cache_dir(),
            DEFAULT_LIMIT_MB,
            file_dir,
            file_mb,
            std::env::var_os("RUSTMUSIC_CACHE_DIR").map(PathBuf::from),
            std::env::var("RUSTMUSIC_CACHE_LIMIT_MB")
                .ok()
                .and_then(|value| value.trim().parse().ok()),
        )
    }
    pub fn save(&self) -> Result<()> {
        let directory = config_dir();
        fs::create_dir_all(&directory).context("Не удалось сохранить настройки кэша")?;
        let stored = StoredConfig {
            cache_dir: Some(self.directory.clone()),
            cache_limit_mb: Some(self.limit_bytes / (1024 * 1024)),
        };
        let bytes = serde_json::to_vec_pretty(&stored)?;
        let temporary = directory.join("config.json.tmp");
        fs::write(&temporary, bytes).context("Не удалось сохранить настройки кэша")?;
        replace_file(&temporary, &directory.join("config.json"))
            .context("Не удалось сохранить настройки кэша")?;
        Ok(())
    }
    pub fn limit_mb(&self) -> u64 {
        self.limit_bytes / (1024 * 1024)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredConfig {
    #[serde(default)]
    cache_dir: Option<PathBuf>,
    #[serde(default)]
    cache_limit_mb: Option<u64>,
}
pub fn resolve_config(
    default_dir: PathBuf,
    default_mb: u64,
    file_dir: Option<PathBuf>,
    file_mb: Option<u64>,
    env_dir: Option<PathBuf>,
    env_mb: Option<u64>,
) -> CacheConfig {
    let directory = env_dir
        .filter(|path| !path.as_os_str().is_empty())
        .or(file_dir.filter(|path| !path.as_os_str().is_empty()))
        .unwrap_or(default_dir);
    let limit_mb = env_mb.or(file_mb).unwrap_or(default_mb);
    CacheConfig {
        directory,
        limit_bytes: limit_mb.saturating_mul(1024 * 1024),
    }
}
pub fn default_cache_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir().join("Library/Caches/rustmusic")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData/Local"))
            .join("rustmusic")
            .join("Cache")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".cache"))
            .join("rustmusic")
    }
}
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir().join("Library/Application Support/rustmusic")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData/Roaming"))
            .join("rustmusic")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("rustmusic")
    }
}
fn config_path() -> PathBuf {
    config_dir().join("config.json")
}
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}
#[derive(Clone)]
pub struct TrackCache {
    inner: Arc<CacheInner>,
}
struct CacheInner {
    directory: PathBuf,
    canonical: PathBuf,
    limit_bytes: u64,
    state: Mutex<State>,
    http: reqwest::Client,
}
struct State {
    index: Index,
    active: HashSet<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    clock: u64,
    #[serde(default)]
    entries: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    name: String,
    bytes: u64,
    used: u64,
}
impl TrackCache {
    pub fn open(config: CacheConfig) -> Result<Self> {
        fs::create_dir_all(&config.directory).context("Не удалось создать каталог кэша")?;
        let canonical = config
            .directory
            .canonicalize()
            .context("Не удалось создать каталог кэша")?;
        let index = load_index(&canonical);
        let mut state = State {
            index,
            active: HashSet::new(),
        };
        evict(&canonical, &mut state.index, config.limit_bytes, "");
        save_index(&canonical, &state.index)?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 3 {
                    return attempt.error(std::io::Error::other("too many redirects"));
                }
                if validate_stream_url(attempt.url().as_str()).is_ok() {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .build()
            .context("Не удалось подготовить загрузку кэша")?;
        Ok(Self {
            inner: Arc::new(CacheInner {
                directory: config.directory,
                canonical,
                limit_bytes: config.limit_bytes,
                state: Mutex::new(state),
                http,
            }),
        })
    }
    pub fn directory(&self) -> PathBuf {
        self.inner.directory.clone()
    }
    pub fn limit_bytes(&self) -> u64 {
        self.inner.limit_bytes
    }
    pub fn usage_bytes(&self) -> u64 {
        lock(&self.inner.state)
            .index
            .entries
            .iter()
            .map(|entry| entry.bytes)
            .sum()
    }
    /// Returns a cached file and marks it as recently used.
    /// A hit means playback can skip network resolution entirely.
    pub fn lookup(&self, id: &str) -> Option<PathBuf> {
        if !valid_video_id(id) {
            return None;
        }
        let mut state = lock(&self.inner.state);
        let position = state
            .index
            .entries
            .iter()
            .position(|entry| entry.id == id)?;
        let path = self
            .inner
            .canonical
            .join(&state.index.entries[position].name);
        let fresh = fs::metadata(&path).ok().is_some_and(|meta| {
            meta.is_file() && meta.len() == state.index.entries[position].bytes
        }) && contained(&self.inner.canonical, &path);
        if !fresh {
            let entry = state.index.entries.remove(position);
            let _ = fs::remove_file(self.inner.canonical.join(entry.name));
            let _ = save_index(&self.inner.canonical, &state.index);
            return None;
        }
        state.index.clock = state.index.clock.saturating_add(1);
        state.index.entries[position].used = state.index.clock;
        let _ = save_index(&self.inner.canonical, &state.index);
        path.canonicalize().ok()
    }
    /// Download a played stream after playback has already started.
    pub fn remember(&self, id: &str, stream: Stream) {
        if self.inner.limit_bytes == 0
            || !valid_video_id(id)
            || validate_stream_url(&stream.url).is_err()
        {
            return;
        }
        let ext = extension_for(&stream.mime);
        {
            let mut state = lock(&self.inner.state);
            if state.index.entries.iter().any(|entry| entry.id == id)
                || !state.active.insert(id.to_owned())
            {
                return;
            }
        }
        let cache = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let partial = cache.partial_path(&id, ext);
            let outcome = cache
                .download_url(&stream.url, &stream.headers, &partial, cache.max_track())
                .await;
            let stored = match outcome {
                Ok(bytes) => cache.commit_file(&id, ext, &partial, bytes).is_ok(),
                Err(_) => false,
            };
            if !stored {
                let _ = fs::remove_file(partial);
            }
            lock(&cache.inner.state).active.remove(&id);
        });
    }
    fn max_track(&self) -> u64 {
        self.inner.limit_bytes.min(MAX_TRACK_BYTES)
    }
    fn partial_path(&self, id: &str, ext: &str) -> PathBuf {
        self.inner.canonical.join(format!("{id}.{ext}.partial"))
    }
    async fn download_url(
        &self,
        url: &str,
        headers: &std::collections::HashMap<String, String>,
        destination: &Path,
        limit: u64,
    ) -> Result<u64> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut request = self.inner.http.get(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let mut response = request
            .send()
            .await
            .context("Не удалось скачать трек в кэш")?;
        if response.status() != reqwest::StatusCode::OK {
            bail!("Кэш не сохранил трек");
        }
        let mut file = tokio::fs::File::create(destination).await?;
        let mut total = 0u64;
        while let Some(chunk) = response.chunk().await? {
            total = total.saturating_add(chunk.len() as u64);
            if total > limit {
                bail!("Трек больше лимита кэша");
            }
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        Ok(total)
    }
    fn commit_file(&self, id: &str, ext: &str, partial: &Path, bytes: u64) -> Result<()> {
        if bytes == 0 || bytes > self.max_track() || self.inner.limit_bytes == 0 {
            let _ = fs::remove_file(partial);
            return Ok(());
        }
        let name = format!("{id}.{ext}");
        let final_path = self.inner.canonical.join(&name);
        replace_file(partial, &final_path)?;
        let mut state = lock(&self.inner.state);
        state.index.entries.retain(|entry| entry.id != id);
        state.index.clock = state.index.clock.saturating_add(1);
        let used = state.index.clock;
        state.index.entries.push(Entry {
            id: id.to_owned(),
            name,
            bytes,
            used,
        });
        evict(
            &self.inner.canonical,
            &mut state.index,
            self.inner.limit_bytes,
            id,
        );
        save_index(&self.inner.canonical, &state.index)?;
        Ok(())
    }
    #[cfg(test)]
    fn store_bytes(&self, id: &str, ext: &str, data: &[u8]) -> Result<()> {
        if !valid_video_id(id) {
            bail!("Некорректный идентификатор трека");
        }
        if !ALLOWED_EXT.contains(&ext) {
            bail!("Некорректное расширение кэша");
        }
        if self.inner.limit_bytes == 0 || data.is_empty() || data.len() as u64 > self.max_track() {
            return Ok(());
        }
        let partial = self.partial_path(id, ext);
        fs::write(&partial, data)?;
        self.commit_file(id, ext, &partial, data.len() as u64)
    }
}
fn extension_for(mime: &str) -> &'static str {
    match mime {
        "audio/mp4" | "audio/aac" => "m4a",
        "audio/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        _ => "bin",
    }
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
fn contained(directory: &Path, file: &Path) -> bool {
    file.canonicalize()
        .is_ok_and(|file| file.starts_with(directory))
}
fn parse_cached_name(name: &str) -> Option<(&str, &str)> {
    let (id, ext) = name.split_once('.')?;
    if name.matches('.').count() == 1 && valid_video_id(id) && ALLOWED_EXT.contains(&ext) {
        Some((id, ext))
    } else {
        None
    }
}
fn load_index(directory: &Path) -> Index {
    let parsed = fs::read(directory.join("index.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
        .unwrap_or_default();
    reconcile(directory, parsed)
}
fn reconcile(directory: &Path, mut index: Index) -> Index {
    let files = read_audio(directory);
    index.entries.retain(|entry| {
        parse_cached_name(&entry.name).is_some()
            && files.iter().any(|(id, name, len)| {
                id == &entry.id && name == &entry.name && *len == entry.bytes
            })
    });
    for (id, name, len) in files {
        if index.entries.iter().any(|entry| entry.id == id) {
            continue;
        }
        index.entries.push(Entry {
            id,
            name,
            bytes: len,
            used: 0,
        });
    }
    index
}
fn read_audio(directory: &Path) -> Vec<(String, String, u64)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let os_name = entry.file_name();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if name.ends_with(".partial") || name.ends_with(".tmp") {
            let _ = fs::remove_file(entry.path());
            continue;
        }
        if name == "index.json" {
            continue;
        }
        let Some((id, _)) = parse_cached_name(name) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_file() && meta.len() > 0 {
            files.push((id.to_owned(), name.to_owned(), meta.len()));
        }
    }
    files
}
fn evict(directory: &Path, index: &mut Index, limit: u64, protect: &str) {
    loop {
        let total: u64 = index.entries.iter().map(|entry| entry.bytes).sum();
        if total <= limit {
            return;
        }
        let victim = index
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.id != protect)
            .min_by_key(|(_, entry)| entry.used)
            .map(|(position, _)| position);
        let Some(position) = victim else {
            if let Some(position) = index.entries.iter().position(|entry| entry.id == protect) {
                let entry = index.entries.remove(position);
                let _ = fs::remove_file(directory.join(entry.name));
            }
            return;
        };
        let entry = index.entries.remove(position);
        let _ = fs::remove_file(directory.join(entry.name));
    }
}
fn save_index(directory: &Path, index: &Index) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(index)?;
    let temporary = directory.join("index.json.tmp");
    fs::write(&temporary, bytes)?;
    replace_file(&temporary, &directory.join("index.json"))?;
    Ok(())
}
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        fs::remove_file(to)?;
    }
    fs::rename(from, to)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    fn open_limited(limit: u64) -> (tempfile::TempDir, TrackCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = TrackCache::open(CacheConfig {
            directory: dir.path().to_path_buf(),
            limit_bytes: limit,
        })
        .unwrap();
        (dir, cache)
    }
    #[test]
    fn env_overrides_saved_directory_and_limit() {
        let config = resolve_config(
            PathBuf::from("/default"),
            1024,
            Some(PathBuf::from("/saved")),
            Some(256),
            Some(PathBuf::from("/env")),
            Some(32),
        );
        assert_eq!(config.directory, PathBuf::from("/env"));
        assert_eq!(config.limit_bytes, 32 * 1024 * 1024);
        let fallback = resolve_config(PathBuf::from("/default"), 1024, None, None, None, None);
        assert_eq!(fallback.directory, PathBuf::from("/default"));
        assert_eq!(fallback.limit_mb(), 1024);
    }
    #[test]
    fn default_directory_uses_platform_cache() {
        let dir = default_cache_dir();
        assert!(dir.ends_with("rustmusic") || dir.ends_with("Cache"));
    }
    #[test]
    fn stores_lookup_and_evicts_least_recently_used() {
        let (_dir, cache) = open_limited(100);
        cache.store_bytes("aaaaaaaaaaa", "m4a", &[1; 40]).unwrap();
        cache.store_bytes("bbbbbbbbbbb", "m4a", &[2; 40]).unwrap();
        assert!(cache.lookup("aaaaaaaaaaa").is_some());
        cache.store_bytes("ccccccccccc", "m4a", &[3; 40]).unwrap();
        assert!(cache.lookup("aaaaaaaaaaa").is_some());
        assert!(cache.lookup("bbbbbbbbbbb").is_none());
        assert!(cache.lookup("ccccccccccc").is_some());
        assert!(cache.usage_bytes() <= 100);
    }
    #[test]
    fn disabled_or_oversized_tracks_are_not_kept() {
        let (_dir, cache) = open_limited(0);
        cache.store_bytes("aaaaaaaaaaa", "m4a", &[1; 20]).unwrap();
        assert!(cache.lookup("aaaaaaaaaaa").is_none());
        let (_dir, cache) = open_limited(100);
        cache.store_bytes("aaaaaaaaaaa", "m4a", &[1; 500]).unwrap();
        assert!(cache.lookup("aaaaaaaaaaa").is_none());
        assert!(cache.store_bytes("../etc/passwd", "m4a", &[1; 4]).is_err());
    }
    #[test]
    fn rebuilds_index_and_drops_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("abcdefghijk.m4a"), [9; 12]).unwrap();
        fs::write(dir.path().join("abcdefghijk.m4a.partial"), [1; 4]).unwrap();
        fs::write(dir.path().join("index.json"), b"{").unwrap();
        let cache = TrackCache::open(CacheConfig {
            directory: dir.path().to_path_buf(),
            limit_bytes: 1000,
        })
        .unwrap();
        assert!(!dir.path().join("abcdefghijk.m4a.partial").exists());
        let path = cache.lookup("abcdefghijk").unwrap();
        assert_eq!(fs::read(path).unwrap(), vec![9; 12]);
    }
    #[test]
    fn repeated_cache_hits_stay_under_resolve_budget() {
        let (_dir, cache) = open_limited(10_000);
        cache.store_bytes("abcdefghijk", "m4a", b"audio").unwrap();
        let started = Instant::now();
        for _ in 0..50 {
            assert!(cache.lookup("abcdefghijk").is_some());
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "50 cache hits took {elapsed:?}; a yt-dlp resolve is orders of magnitude slower"
        );
    }
    #[tokio::test]
    async fn downloads_audio_bytes_into_the_cache() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = b"cached-audio";
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut collected = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = tokio::io::AsyncReadExt::read(&mut socket, &mut buf)
                    .await
                    .unwrap();
                if n == 0 {
                    break;
                }
                collected.extend_from_slice(&buf[..n]);
                if collected.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            tokio::io::AsyncWriteExt::write_all(&mut socket, header.as_bytes())
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut socket, body)
                .await
                .unwrap();
        });
        let (_dir, cache) = open_limited(10_000);
        let partial = cache.partial_path("abcdefghijk", "m4a");
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            cache.download_url(
                &format!("http://127.0.0.1:{port}/track"),
                &std::collections::HashMap::new(),
                &partial,
                10_000,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        cache
            .commit_file("abcdefghijk", "m4a", &partial, bytes)
            .unwrap();
        let path = cache.lookup("abcdefghijk").unwrap();
        assert_eq!(fs::read(path).unwrap(), body);
    }
    #[test]
    fn remember_ignores_non_googlevideo_targets() {
        let (_dir, cache) = open_limited(10_000);
        cache.remember(
            "abcdefghijk",
            Stream {
                url: "https://127.0.0.1/secret".into(),
                headers: std::collections::HashMap::new(),
                mime: "audio/mp4".into(),
                expires: std::time::Instant::now() + Duration::from_secs(60),
            },
        );
        assert!(cache.lookup("abcdefghijk").is_none());
        assert!(lock(&cache.inner.state).active.is_empty());
    }
}
