use crate::{Session, valid_video_id};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, Semaphore};

pub fn tool_path(name: &str) -> PathBuf {
    let variable = format!("RUSTMUSIC_{}", name.replace('-', "_").to_uppercase());
    if let Some(path) = std::env::var_os(variable) {
        return path.into();
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        for folder in [parent.join("../Resources/bin"), parent.join("bin")] {
            let path = folder.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_owned()
            });
            if path.is_file() {
                return path;
            }
        }
    }
    name.into()
}
const MEMORY_CACHE_LIMIT: usize = 16;
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
        let mut command = tokio::process::Command::new(tool_path("yt-dlp"));
        command.args([
            "--ignore-config",
            "--no-playlist",
            "--no-warnings",
            "--dump-single-json",
            "--skip-download",
            "--socket-timeout",
            "20",
            "--retries",
            "1",
            "--format",
            if browser {
                "bestaudio[ext=m4a]/bestaudio[ext=webm]/bestaudio"
            } else {
                "bestaudio"
            },
        ]);
        if let Some(file) = &cookies {
            command.arg("--cookies").arg(file.path());
        }
        let deno = tool_path("deno");
        if deno.is_file() {
            command
                .arg("--js-runtimes")
                .arg(format!("deno:{}", deno.display()));
        }
        command
            .arg(format!("https://www.youtube.com/watch?v={id}"))
            .kill_on_drop(true);
        let output =
            tokio::time::timeout(Duration::from_secs(70), crate::helper::output(&mut command))
                .await
                .context("Время получения потока истекло")?
                .context("Не найден yt-dlp. Используйте полную сборку RustMusic.")?;
        if !output.status.success() {
            bail!(
                "Не удалось получить аудиопоток. Трек может быть недоступен; проверьте сессию или обновите yt-dlp."
            );
        }
        let value: Value =
            serde_json::from_slice(&output.stdout).context("Неподдерживаемый ответ yt-dlp")?;
        let url = value["url"]
            .as_str()
            .context("Аудиопоток отсутствует")?
            .to_owned();
        validate_stream_url(&url)?;
        let mut headers = HashMap::new();
        if let Some(object) = value["http_headers"].as_object() {
            for (key, value) in object {
                if let Some(s) = value.as_str()
                    && matches!(
                        key.to_ascii_lowercase().as_str(),
                        "user-agent" | "referer" | "origin"
                    )
                {
                    headers.insert(key.clone(), s.into());
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
}
fn stream_ttl(url: &str) -> Duration {
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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
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
    fn stream_ttl_follows_expire_and_stays_bounded() {
        assert_eq!(
            stream_ttl("https://rr1---sn.googlevideo.com/videoplayback?expire=1"),
            Duration::ZERO
        );
        assert_eq!(
            stream_ttl("https://rr1---sn.googlevideo.com/videoplayback?expire=9999999999"),
            Duration::from_secs(15 * 60)
        );
        assert_eq!(
            stream_ttl("https://rr1---sn.googlevideo.com/videoplayback?x=1"),
            Duration::from_secs(240)
        );
    }
    #[test]
    fn memory_cache_drops_the_soonest_expiry() {
        let mut cache = HashMap::new();
        for index in 0..MEMORY_CACHE_LIMIT {
            remember_stream(
                &mut cache,
                index.to_string(),
                Stream {
                    url: "https://rr1---sn.googlevideo.com/x".into(),
                    headers: HashMap::new(),
                    mime: "audio/mp4".into(),
                    expires: Instant::now() + Duration::from_secs(60 + index as u64),
                },
            );
        }
        assert_eq!(cache.len(), MEMORY_CACHE_LIMIT);
        remember_stream(
            &mut cache,
            "fresh".into(),
            Stream {
                url: "https://rr1---sn.googlevideo.com/x".into(),
                headers: HashMap::new(),
                mime: "audio/mp4".into(),
                expires: Instant::now() + Duration::from_secs(10_000),
            },
        );
        assert_eq!(cache.len(), MEMORY_CACHE_LIMIT);
        assert!(!cache.contains_key("0"));
        assert!(cache.contains_key("fresh"));
        assert!(cache.contains_key("1"));
    }
}
