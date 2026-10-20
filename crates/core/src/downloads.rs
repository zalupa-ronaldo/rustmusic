//! Explicit track downloads.
//!
//! These files are not part of the playback cache. Nothing in this module
//! evicts a finished download, and the playback cache refuses to delete or
//! replace files in a directory marked `.rustmusic-pinned`.
use crate::{Stream, cache::config_dir, resolver::validate_stream_url, valid_video_id};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

/// Disk safety ceiling for one explicit download.
///
/// This is not the playback-cache budget. A mix or an hour-long audio file is
/// saved whole; only a runaway response past 1 GiB is refused.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
const ALLOWED_EXT: &[&str] = &["m4a", "webm", "ogg", "mp3", "bin"];
const MARKER: &str = ".rustmusic-pinned";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPhase {
    Idle,
    Active(u8),
    Ready,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadInfo {
    pub id: String,
    pub phase: DownloadPhase,
    pub progress: u8,
}
#[derive(Clone)]
pub struct DownloadStore {
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
    active: HashMap<String, Active>,
}
struct Active {
    progress: u8,
    failed: Option<String>,
    downloading: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    entries: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    name: String,
    bytes: u64,
}
pub fn default_downloads_dir() -> PathBuf {
    config_dir().join("downloads")
}
/// An HTTP Range header this store will not satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadByteRange;
/// `Ok(None)` means the whole file. `Err` is an unsatisfiable range.
pub fn http_byte_range(header: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, BadByteRange> {
    let Some(header) = header else {
        return Ok(None);
    };
    let spec = header.trim().strip_prefix("bytes=").ok_or(BadByteRange)?;
    if spec.is_empty() || spec.contains(',') {
        return Err(BadByteRange);
    }
    let (start, end) = spec.split_once('-').ok_or(BadByteRange)?;
    if len == 0 {
        return Err(BadByteRange);
    }
    if start.is_empty() {
        let suffix: u64 = end.parse().map_err(|_| BadByteRange)?;
        if suffix == 0 {
            return Err(BadByteRange);
        }
        let suffix = suffix.min(len);
        return Ok(Some((len - suffix, len - 1)));
    }
    let start: u64 = start.parse().map_err(|_| BadByteRange)?;
    if start >= len {
        return Err(BadByteRange);
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().map_err(|_| BadByteRange)?.min(len - 1)
    };
    if end < start {
        return Err(BadByteRange);
    }
    Ok(Some((start, end)))
}
impl DownloadStore {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory).context("Не удалось создать каталог загрузок")?;
        let canonical = directory
            .canonicalize()
            .context("Не удалось создать каталог загрузок")?;
        fs::write(canonical.join(MARKER), b"rustmusic-downloads\n")
            .context("Не удалось защитить каталог загрузок")?;
        let index = load_index(&canonical);
        save_index(&canonical, &index)?;
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
            .context("Не удалось подготовить загрузку трека")?;
        Ok(Self {
            inner: Arc::new(Inner {
                directory,
                canonical,
                state: Mutex::new(State {
                    index,
                    active: HashMap::new(),
                }),
                http,
            }),
        })
    }
    pub fn directory(&self) -> PathBuf {
        self.inner.directory.clone()
    }
    pub fn count(&self) -> usize {
        lock(&self.inner.state).index.entries.len()
    }
    pub fn bytes(&self) -> u64 {
        lock(&self.inner.state)
            .index
            .entries
            .iter()
            .map(|entry| entry.bytes)
            .sum()
    }
    pub fn phase(&self, id: &str) -> DownloadPhase {
        if !valid_video_id(id) {
            return DownloadPhase::Idle;
        }
        let state = lock(&self.inner.state);
        if ready_entry(&state.index, &self.inner.canonical, id).is_some() {
            return DownloadPhase::Ready;
        }
        match state.active.get(id) {
            Some(active) if active.failed.is_some() => DownloadPhase::Failed,
            Some(active) => DownloadPhase::Active(active.progress),
            None => DownloadPhase::Idle,
        }
    }
    pub fn error(&self, id: &str) -> Option<String> {
        lock(&self.inner.state)
            .active
            .get(id)
            .and_then(|active| active.failed.clone())
    }
    pub fn list(&self) -> Vec<DownloadInfo> {
        let state = lock(&self.inner.state);
        let mut items = Vec::new();
        for entry in &state.index.entries {
            if ready_entry(&state.index, &self.inner.canonical, &entry.id).is_some() {
                items.push(DownloadInfo {
                    id: entry.id.clone(),
                    phase: DownloadPhase::Ready,
                    progress: 100,
                });
            }
        }
        for (id, active) in &state.active {
            if items.iter().any(|item| item.id == *id) {
                continue;
            }
            let (phase, progress) = if active.failed.is_some() {
                (DownloadPhase::Failed, 0)
            } else {
                (DownloadPhase::Active(active.progress), active.progress)
            };
            items.push(DownloadInfo {
                id: id.clone(),
                phase,
                progress,
            });
        }
        items
    }
    pub fn mark_failed(&self, id: &str, message: impl Into<String>) {
        if valid_video_id(id) {
            self.fail(id, message.into());
        }
    }
    /// Show the download as started before the audio URL is known.
    /// A later `save_stream` continues this reservation instead of starting a second one.
    pub fn reserve(&self, id: &str) -> bool {
        if !valid_video_id(id) {
            return false;
        }
        let mut state = lock(&self.inner.state);
        if ready_entry(&state.index, &self.inner.canonical, id).is_some() {
            return false;
        }
        if state
            .active
            .get(id)
            .is_some_and(|active| active.failed.is_none())
        {
            return false;
        }
        state.active.insert(
            id.to_owned(),
            Active {
                progress: 0,
                failed: None,
                downloading: false,
            },
        );
        true
    }
    pub fn lookup(&self, id: &str) -> Option<PathBuf> {
        if !valid_video_id(id) {
            return None;
        }
        let state = lock(&self.inner.state);
        let entry = ready_entry(&state.index, &self.inner.canonical, id)?;
        let path = self.inner.canonical.join(&entry.name);
        path.canonicalize()
            .ok()
            .filter(|path| path.starts_with(&self.inner.canonical))
    }
    /// Copy an already cached playback file into the pinned store.
    pub fn pin_copy(&self, id: &str, source: &Path) -> Result<()> {
        if !valid_video_id(id) {
            bail!("Некорректный идентификатор трека");
        }
        if self.lookup(id).is_some() {
            return Ok(());
        }
        let ext = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        if !ALLOWED_EXT.contains(&ext) {
            bail!("Некорректное расширение файла");
        }
        let len = fs::metadata(source)
            .context("Не удалось прочитать файл трека")?
            .len();
        if len == 0 || len > MAX_DOWNLOAD_BYTES {
            bail!("Трек слишком большой для сохранения");
        }
        let partial = self.inner.canonical.join(format!("{id}.{ext}.partial"));
        fs::copy(source, &partial).context("Не удалось прочитать файл трека")?;
        self.commit_partial(id, ext, &partial)
    }
    pub fn store(&self, id: &str, ext: &str, data: &[u8]) -> Result<()> {
        if !valid_video_id(id) {
            bail!("Некорректный идентификатор трека");
        }
        if !ALLOWED_EXT.contains(&ext) {
            bail!("Некорректное расширение файла");
        }
        if data.is_empty() || data.len() as u64 > MAX_DOWNLOAD_BYTES {
            bail!("Трек слишком большой для сохранения");
        }
        let partial = self.inner.canonical.join(format!("{id}.{ext}.partial"));
        let mut file = fs::File::create(&partial)?;
        file.write_all(data)?;
        file.sync_all()?;
        self.commit_partial(id, ext, &partial)
    }
    fn commit_partial(&self, id: &str, ext: &str, partial: &Path) -> Result<()> {
        let bytes = fs::metadata(partial).map(|meta| meta.len()).unwrap_or(0);
        if bytes == 0 || bytes > MAX_DOWNLOAD_BYTES {
            let _ = fs::remove_file(partial);
            bail!("Трек слишком большой для сохранения");
        }
        let name = format!("{id}.{ext}");
        let final_path = self.inner.canonical.join(&name);
        replace_file(partial, &final_path)?;
        let mut state = lock(&self.inner.state);
        state.index.entries.retain(|entry| entry.id != id);
        state.index.entries.push(Entry {
            id: id.to_owned(),
            name,
            bytes,
        });
        state.active.remove(id);
        save_index(&self.inner.canonical, &state.index)?;
        Ok(())
    }
    pub async fn save_stream(&self, id: &str, stream: Stream) -> Result<()> {
        if !valid_video_id(id) {
            bail!("Некорректный идентификатор трека");
        }
        validate_stream_url(&stream.url)?;
        if self.lookup(id).is_some() {
            return Ok(());
        }
        if !self.claim(id) {
            return Ok(());
        }
        let ext = extension_for(&stream.mime);
        match self
            .download_to(id, &stream.url, &stream.headers, ext)
            .await
        {
            Ok(()) => Ok(()),
            Err(error) => {
                self.fail(id, error.to_string());
                Err(error)
            }
        }
    }
    fn claim(&self, id: &str) -> bool {
        let mut state = lock(&self.inner.state);
        if ready_entry(&state.index, &self.inner.canonical, id).is_some() {
            return false;
        }
        if let Some(active) = state.active.get_mut(id) {
            if active.failed.is_none() && active.downloading {
                return false;
            }
            active.failed = None;
            active.progress = 0;
            active.downloading = true;
            return true;
        }
        state.active.insert(
            id.to_owned(),
            Active {
                progress: 0,
                failed: None,
                downloading: true,
            },
        );
        true
    }
    fn fail(&self, id: &str, message: String) {
        for ext in ALLOWED_EXT {
            let _ = fs::remove_file(self.inner.canonical.join(format!("{id}.{ext}.partial")));
        }
        let mut state = lock(&self.inner.state);
        state.active.insert(
            id.to_owned(),
            Active {
                progress: 0,
                failed: Some(message),
                downloading: false,
            },
        );
    }
    fn set_progress(&self, id: &str, progress: u8) {
        let mut state = lock(&self.inner.state);
        if let Some(active) = state.active.get_mut(id)
            && active.failed.is_none()
        {
            active.progress = progress.min(99);
        }
    }
    async fn download_to(
        &self,
        id: &str,
        url: &str,
        headers: &HashMap<String, String>,
        ext: &str,
    ) -> Result<()> {
        let name = format!("{id}.{ext}");
        let partial = self.inner.canonical.join(format!("{name}.partial"));
        let mut request = self.inner.http.get(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let mut response = request.send().await.context("Не удалось скачать трек")?;
        if !response.status().is_success() {
            bail!("Не удалось скачать трек");
        }
        let hinted = response.content_length().filter(|len| *len > 0);
        if hinted.is_some_and(|len| len > MAX_DOWNLOAD_BYTES) {
            bail!("Трек слишком большой для сохранения");
        }
        let mut file = tokio::fs::File::create(&partial).await?;
        let mut total = 0u64;
        let mut too_big = false;
        while let Some(chunk) = response.chunk().await? {
            total = total.saturating_add(chunk.len() as u64);
            if total > MAX_DOWNLOAD_BYTES {
                too_big = true;
                break;
            }
            file.write_all(&chunk).await?;
            let progress = hinted
                .map(|len| ((total.saturating_mul(100)) / len).min(99) as u8)
                .unwrap_or(0);
            self.set_progress(id, progress);
        }
        file.sync_all().await?;
        drop(file);
        if too_big || total == 0 {
            let _ = tokio::fs::remove_file(&partial).await;
            let message = if too_big {
                "Трек слишком большой для сохранения"
            } else {
                "Пустой файл трека"
            };
            bail!("{message}");
        }
        self.commit_partial(id, ext, &partial)?;
        Ok(())
    }
}
fn ready_entry<'a>(index: &'a Index, directory: &Path, id: &str) -> Option<&'a Entry> {
    let entry = index.entries.iter().find(|entry| entry.id == id)?;
    let path = directory.join(&entry.name);
    fs::metadata(&path)
        .ok()
        .is_some_and(|meta| meta.is_file() && meta.len() == entry.bytes)
        .then_some(entry)
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
fn load_index(directory: &Path) -> Index {
    let mut index = fs::read(directory.join("index.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
        .unwrap_or_default();
    index.entries.retain(|entry| {
        parse_name(&entry.name).is_some_and(|(id, _)| id == entry.id)
            && fs::metadata(directory.join(&entry.name))
                .ok()
                .is_some_and(|meta| meta.is_file() && meta.len() == entry.bytes)
    });
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let os_name = entry.file_name();
            let Some(name) = os_name.to_str() else {
                continue;
            };
            if name.ends_with(".partial") || name.ends_with(".tmp") {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            if name == "index.json" || name == MARKER {
                continue;
            }
            let Some((id, _)) = parse_name(name) else {
                continue;
            };
            if index.entries.iter().any(|stored| stored.id == id) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_file() && meta.len() > 0 {
                index.entries.push(Entry {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    bytes: meta.len(),
                });
            }
        }
    }
    index
}
fn parse_name(name: &str) -> Option<(&str, &str)> {
    let (id, ext) = name.split_once('.')?;
    if name.matches('.').count() == 1 && valid_video_id(id) && ALLOWED_EXT.contains(&ext) {
        Some((id, ext))
    } else {
        None
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
    fn open() -> (tempfile::TempDir, DownloadStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = DownloadStore::open(dir.path()).unwrap();
        (dir, store)
    }
    #[test]
    fn keeps_every_download_and_rebuilds_a_missing_index() {
        let (dir, store) = open();
        store.store("aaaaaaaaaaa", "m4a", &[1; 20]).unwrap();
        store.store("bbbbbbbbbbb", "webm", &[2; 20]).unwrap();
        store.store("ccccccccccc", "mp3", &[3; 20]).unwrap();
        assert_eq!(store.count(), 3);
        assert!(store.bytes() >= 60);
        assert_eq!(store.phase("aaaaaaaaaaa"), DownloadPhase::Ready);
        fs::remove_file(dir.path().join("index.json")).unwrap();
        let reopened = DownloadStore::open(dir.path()).unwrap();
        assert_eq!(
            fs::read(reopened.lookup("bbbbbbbbbbb").unwrap()).unwrap(),
            vec![2; 20]
        );
        assert_eq!(reopened.count(), 3);
        assert!(dir.path().join(MARKER).is_file());
    }
    #[test]
    fn copy_survives_deletion_of_the_source_file() {
        let (dir, store) = open();
        let source = dir.path().join("source.m4a");
        fs::write(&source, [4; 12]).unwrap();
        store.pin_copy("abcdefghijk", &source).unwrap();
        fs::remove_file(&source).unwrap();
        assert_eq!(
            fs::read(store.lookup("abcdefghijk").unwrap()).unwrap(),
            vec![4; 12]
        );
        store
            .pin_copy("abcdefghijk", &store.lookup("abcdefghijk").unwrap())
            .unwrap();
        assert_eq!(store.count(), 1);
    }
    #[test]
    fn mixes_and_hour_long_audio_fit_under_the_download_ceiling() {
        let hour_at_256_kbps = 256 * 1000 / 8 * 60 * 60;
        assert!(hour_at_256_kbps > 80 * 1024 * 1024);
        assert!(hour_at_256_kbps <= MAX_DOWNLOAD_BYTES);
        assert!(4 * hour_at_256_kbps <= MAX_DOWNLOAD_BYTES);
    }
    #[test]
    fn rejects_bad_ids_and_oversized_payloads() {
        let (_dir, store) = open();
        assert!(store.store("../etc/passwd", "m4a", &[1; 4]).is_err());
        assert!(store.store("abcdefghijk", "exe", &[1; 4]).is_err());
        assert!(store.store("abcdefghijk", "m4a", &[]).is_err());
        assert!(store.lookup("../etc/passwd").is_none());
    }
    #[test]
    fn parses_single_byte_ranges() {
        assert_eq!(http_byte_range(None, 10).unwrap(), None);
        assert_eq!(
            http_byte_range(Some("bytes=0-3"), 10).unwrap(),
            Some((0, 3))
        );
        assert_eq!(http_byte_range(Some("bytes=8-"), 10).unwrap(), Some((8, 9)));
        assert_eq!(http_byte_range(Some("bytes=-2"), 10).unwrap(), Some((8, 9)));
        assert!(http_byte_range(Some("bytes=10-11"), 10).is_err());
        assert!(http_byte_range(Some("bytes=0-1,2-3"), 10).is_err());
        assert!(http_byte_range(Some("bytes=0-1"), 0).is_err());
    }
    #[tokio::test]
    async fn downloads_bytes_and_reports_completion() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = b"pinned-audio-bytes";
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
        let (_dir, store) = open();
        assert!(store.reserve("abcdefghijk"));
        store
            .download_to(
                "abcdefghijk",
                &format!("http://127.0.0.1:{port}/track"),
                &HashMap::new(),
                "m4a",
            )
            .await
            .unwrap();
        assert_eq!(store.phase("abcdefghijk"), DownloadPhase::Ready);
        assert_eq!(
            fs::read(store.lookup("abcdefghijk").unwrap()).unwrap(),
            body
        );
    }
    #[tokio::test]
    async fn content_length_past_the_ceiling_is_refused_before_a_file_is_kept() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
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
                MAX_DOWNLOAD_BYTES + 1
            );
            tokio::io::AsyncWriteExt::write_all(&mut socket, header.as_bytes())
                .await
                .unwrap();
        });
        let (dir, store) = open();
        let error = store
            .download_to(
                "abcdefghijk",
                &format!("http://127.0.0.1:{port}/track"),
                &HashMap::new(),
                "m4a",
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("слишком большой"));
        assert!(store.lookup("abcdefghijk").is_none());
        assert!(!dir.path().join("abcdefghijk.m4a").exists());
    }
    #[tokio::test]
    async fn save_stream_rejects_non_googlevideo_targets() {
        let (dir, store) = open();
        let error = store
            .save_stream(
                "abcdefghijk",
                Stream {
                    url: "https://127.0.0.1/secret".into(),
                    headers: HashMap::new(),
                    mime: "audio/mp4".into(),
                    expires: std::time::Instant::now() + Duration::from_secs(60),
                },
            )
            .await;
        assert!(error.is_err());
        assert_eq!(store.phase("abcdefghijk"), DownloadPhase::Idle);
        assert!(!dir.path().join("abcdefghijk.m4a").exists());
    }
}
