//! Disk cache of track cover images.
//!
//! Artwork lives outside the playback-audio index. Audio eviction never walks
//! this directory, and this cache never deletes audio files.
use crate::cache::config_dir;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct ArtCache {
    inner: Arc<Inner>,
}
struct Inner {
    directory: PathBuf,
    canonical: PathBuf,
    state: Mutex<State>,
    http: reqwest::Client,
}
struct State {
    index: Index,
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
    url: String,
    name: String,
    bytes: u64,
    used: u64,
}
pub fn default_covers_dir() -> PathBuf {
    config_dir().join("covers")
}
pub fn allowed_artwork_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    parsed.scheme() == "https"
        && artwork_host(host)
        && parsed.port().is_none_or(|port| port == 443)
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.as_str().len() <= 2048
}
fn artwork_host(host: &str) -> bool {
    host == "i.ytimg.com"
        || host.ends_with(".ytimg.com")
        || host.ends_with(".googleusercontent.com")
        || host.ends_with(".ggpht.com")
}
pub fn image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}
impl ArtCache {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory).context("Не удалось создать каталог обложек")?;
        let canonical = directory
            .canonicalize()
            .context("Не удалось создать каталог обложек")?;
        let index = load_index(&canonical);
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 3 {
                    return attempt.error(std::io::Error::other("too many redirects"));
                }
                if allowed_artwork_url(attempt.url().as_str()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .build()
            .context("Не удалось подготовить загрузку обложек")?;
        Ok(Self {
            inner: Arc::new(Inner {
                directory,
                canonical,
                state: Mutex::new(State { index }),
                http,
            }),
        })
    }
    pub fn directory(&self) -> PathBuf {
        self.inner.directory.clone()
    }
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        if !allowed_artwork_url(url) {
            return None;
        }
        let mut state = lock(&self.inner.state);
        let position = state
            .index
            .entries
            .iter()
            .position(|entry| entry.url == url)?;
        let path = self
            .inner
            .canonical
            .join(&state.index.entries[position].name);
        let fresh = fs::read(&path).ok().filter(|bytes| {
            bytes.len() as u64 == state.index.entries[position].bytes && image_kind(bytes).is_some()
        });
        let Some(bytes) = fresh else {
            let entry = state.index.entries.remove(position);
            let _ = fs::remove_file(self.inner.canonical.join(entry.name));
            let _ = save_index(&self.inner.canonical, &state.index);
            return None;
        };
        state.index.clock = state.index.clock.saturating_add(1);
        state.index.entries[position].used = state.index.clock;
        let _ = save_index(&self.inner.canonical, &state.index);
        Some(bytes)
    }
    /// Network fetch on a miss. A hit returns the stored bytes and does not
    /// open a connection.
    pub async fn load(&self, url: &str) -> Result<Vec<u8>> {
        if let Some(bytes) = self.get(url) {
            return Ok(bytes);
        }
        if !allowed_artwork_url(url) {
            bail!("Некорректный адрес обложки");
        }
        let response = self
            .inner
            .http
            .get(url)
            .send()
            .await
            .context("Не удалось загрузить обложку")?;
        if !response.status().is_success() {
            bail!("Не удалось загрузить обложку");
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_IMAGE_BYTES)
        {
            bail!("Обложка слишком большая");
        }
        let bytes = response
            .bytes()
            .await
            .context("Не удалось загрузить обложку")?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES || image_kind(&bytes).is_none() {
            bail!("Обложка слишком большая");
        }
        self.store(url, &bytes)?;
        Ok(bytes.to_vec())
    }
    pub fn forget(&self, url: &str) {
        let mut state = lock(&self.inner.state);
        if let Some(position) = state
            .index
            .entries
            .iter()
            .position(|entry| entry.url == url)
        {
            let entry = state.index.entries.remove(position);
            let _ = fs::remove_file(self.inner.canonical.join(entry.name));
            let _ = save_index(&self.inner.canonical, &state.index);
        }
    }
    fn store(&self, url: &str, bytes: &[u8]) -> Result<()> {
        if !allowed_artwork_url(url)
            || bytes.is_empty()
            || bytes.len() as u64 > MAX_IMAGE_BYTES
            || image_kind(bytes).is_none()
        {
            bail!("Некорректная обложка");
        }
        let mut state = lock(&self.inner.state);
        let name = file_name(url, &state.index.entries);
        let partial = self.inner.canonical.join(format!("{name}.partial"));
        fs::write(&partial, bytes)?;
        let final_path = self.inner.canonical.join(&name);
        replace_file(&partial, &final_path)?;
        state.index.entries.retain(|entry| entry.url != url);
        state.index.clock = state.index.clock.saturating_add(1);
        let used = state.index.clock;
        state.index.entries.push(Entry {
            url: url.to_owned(),
            name,
            bytes: bytes.len() as u64,
            used,
        });
        evict(&self.inner.canonical, &mut state.index, LIMIT_BYTES);
        save_index(&self.inner.canonical, &state.index)?;
        Ok(())
    }
}
fn file_name(url: &str, entries: &[Entry]) -> String {
    let hash = format!("{:016x}", fnv64(url.as_bytes()));
    let mut name = format!("{hash}.img");
    let mut suffix = 1u32;
    while entries
        .iter()
        .any(|entry| entry.name == name && entry.url != url)
    {
        name = format!("{hash}-{suffix}.img");
        suffix = suffix.saturating_add(1);
    }
    name
}
fn fnv64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
fn load_index(directory: &Path) -> Index {
    let mut index = fs::read(directory.join("index.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
        .unwrap_or_default();
    index.entries.retain(|entry| {
        let path = directory.join(&entry.name);
        entry.name.ends_with(".img")
            && !entry.name.contains(['/', '\\'])
            && fs::metadata(&path)
                .ok()
                .is_some_and(|meta| meta.is_file() && meta.len() == entry.bytes)
    });
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.ends_with(".partial") || name.ends_with(".tmp") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    index
}
fn evict(directory: &Path, index: &mut Index, limit: u64) {
    loop {
        let total: u64 = index.entries.iter().map(|entry| entry.bytes).sum();
        if total <= limit {
            return;
        }
        let Some(position) = index
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.used)
            .map(|(position, _)| position)
        else {
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
    fn open() -> (tempfile::TempDir, ArtCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = ArtCache::open(dir.path()).unwrap();
        (dir, cache)
    }
    #[test]
    fn allows_only_youtube_image_hosts() {
        assert!(allowed_artwork_url(
            "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg"
        ));
        assert!(allowed_artwork_url(
            "https://lh3.googleusercontent.com/a/cover=w120"
        ));
        assert!(allowed_artwork_url("https://yt3.ggpht.com/cover"));
        assert!(!allowed_artwork_url("http://i.ytimg.com/vi/x.jpg"));
        assert!(!allowed_artwork_url("https://ytimg.com.evil.test/x.jpg"));
        assert!(!allowed_artwork_url("https://user@i.ytimg.com/x.jpg"));
        assert!(!allowed_artwork_url("https://127.0.0.1/x.jpg"));
    }
    #[test]
    fn stores_and_reloads_without_a_second_write() {
        let (dir, cache) = open();
        let url = "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg";
        let bytes = b"\xff\xd8\xff\x00cover".to_vec();
        cache.store(url, &bytes).unwrap();
        assert_eq!(cache.get(url).unwrap(), bytes);
        assert!(dir.path().join("index.json").is_file());
        let reopened = ArtCache::open(dir.path()).unwrap();
        assert_eq!(reopened.get(url).unwrap(), bytes);
    }
    #[test]
    fn evicts_least_recent_cover_and_leaves_foreign_files() {
        let (dir, cache) = open();
        let neighbor = dir.path().join("not-a-cover.txt");
        fs::write(&neighbor, b"keep").unwrap();
        let url = |name: &str| format!("https://i.ytimg.com/vi/{name}/hqdefault.jpg");
        let payload = |byte: u8| {
            let mut bytes = b"\xff\xd8\xff".to_vec();
            bytes.extend(std::iter::repeat_n(byte, 40));
            bytes
        };
        cache.store(&url("aaaaaaaaaaa"), &payload(1)).unwrap();
        cache.store(&url("bbbbbbbbbbb"), &payload(2)).unwrap();
        assert!(cache.get(&url("aaaaaaaaaaa")).is_some());
        let mut state = lock(&cache.inner.state);
        evict(&cache.inner.canonical, &mut state.index, 50);
        drop(state);
        assert!(cache.get(&url("bbbbbbbbbbb")).is_none());
        assert!(cache.get(&url("aaaaaaaaaaa")).is_some());
        assert_eq!(fs::read(&neighbor).unwrap(), b"keep");
    }
    #[test]
    fn rejects_non_images_and_oversized_payloads() {
        let (_dir, cache) = open();
        let url = "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg";
        assert!(cache.store(url, b"not-an-image").is_err());
        assert!(cache.get(url).is_none());
        let mut huge = b"\xff\xd8\xff".to_vec();
        huge.resize(MAX_IMAGE_BYTES as usize + 8, 1);
        assert!(cache.store(url, &huge).is_err());
    }
}
