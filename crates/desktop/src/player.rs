use anyhow::{Context, Result};
use rustmusic_core::{Stream, resolver::tool_path};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub position: f32,
    pub duration: f32,
    pub paused: bool,
    pub ended: bool,
    pub failed: bool,
}
#[derive(Clone, Default)]
pub struct Player(Arc<PlayerInner>);
#[derive(Default)]
struct PlayerInner {
    engine: Mutex<Option<Engine>>,
    epoch: AtomicU64,
}
impl Player {
    pub fn set_epoch(&self, epoch: u64) {
        self.0.epoch.store(epoch, Ordering::SeqCst);
    }
    pub async fn load(&self, stream: Stream, epoch: u64) -> Result<()> {
        let player = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut lock = player
                .0
                .engine
                .lock()
                .map_err(|_| anyhow::anyhow!("Плеер недоступен"))?;
            if player.0.epoch.load(Ordering::SeqCst) != epoch {
                return Ok(());
            }
            if let Some(engine) = lock.as_mut()
                && engine.child.try_wait()?.is_some()
            {
                *lock = None;
            }
            if lock.is_none() {
                *lock = Some(Engine::new()?);
            }
            let engine = lock.as_mut().unwrap();
            let headers = stream
                .headers
                .into_iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>();
            engine
                .command(json!(["set_property", "http-header-fields", headers]))
                .context("mpv: настройка заголовков")?;
            engine
                .command(json!(["loadfile", stream.url, "replace"]))
                .context("mpv: запуск потока")?;
            engine.note_load();
            engine
                .command(json!(["set_property", "pause", false]))
                .context("mpv: начало воспроизведения")?;
            Ok(())
        })
        .await?
    }
    pub async fn command(&self, command: Value) -> Result<()> {
        let player = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut lock = player
                .0
                .engine
                .lock()
                .map_err(|_| anyhow::anyhow!("Плеер недоступен"))?;
            if let Some(engine) = lock.as_mut() {
                engine.command(command)?;
            }
            Ok(())
        })
        .await?
    }
    pub async fn status(&self) -> Result<Status> {
        let player = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut lock = player
                .0
                .engine
                .lock()
                .map_err(|_| anyhow::anyhow!("Плеер недоступен"))?;
            if let Some(e) = lock.as_mut() {
                let position = e.property("time-pos")?.as_f64().unwrap_or(0.) as f32;
                let duration = e.property("duration")?.as_f64().unwrap_or(0.) as f32;
                let paused = e.property("pause")?.as_bool().unwrap_or(false);
                let ended = e.property("eof-reached")?.as_bool().unwrap_or(false);
                let idle = e.property("idle-active")?.as_bool().unwrap_or(false);
                if position > 0.05 || duration > 0. {
                    e.seen_audio = true;
                }
                let since = e.loaded_at.map(|t| t.elapsed()).unwrap_or(Duration::ZERO);
                let failed = e.loaded_at.is_some()
                    && playback_failed(idle, e.end_reason.as_deref(), since, e.seen_audio);
                Ok(Status {
                    position,
                    duration,
                    paused,
                    ended,
                    failed,
                })
            } else {
                Ok(Status::default())
            }
        })
        .await?
    }
}
#[cfg(unix)]
type Connection = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Connection = std::fs::File;
struct Engine {
    child: Child,
    connection: Connection,
    #[cfg(windows)]
    _job: rustmusic_core::WindowsJob,
    request_id: u64,
    end_reason: Option<String>,
    loaded_at: Option<Instant>,
    seen_audio: bool,
}
impl Engine {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        let (connection, inherited) = Connection::pair()?;
        #[cfg(unix)]
        let stdin = Stdio::from(std::os::fd::OwnedFd::from(inherited));
        #[cfg(unix)]
        let ipc = "--input-ipc-client=fd://0".to_owned();
        #[cfg(windows)]
        let socket = format!(r"\\.\pipe\rustmusic-{}", std::process::id());
        #[cfg(windows)]
        let stdin = Stdio::null();
        #[cfg(windows)]
        let ipc = format!("--input-ipc-server={socket}");
        let child = Command::new(tool_path("mpv"))
            .args([
                "--no-config",
                "--idle=yes",
                "--keep-open=yes",
                "--no-video",
                "--audio-display=no",
                "--ytdl=no",
                "--terminal=no",
                "--input-default-bindings=no",
                "--input-media-keys=no",
                "--cache=yes",
                "--demuxer-max-bytes=8MiB",
                "--demuxer-max-back-bytes=2MiB",
            ])
            .arg(ipc)
            .stdin(stdin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Не найден mpv. Используйте полную сборку RustMusic.")?;
        #[cfg(windows)]
        let mut child = child;
        #[cfg(windows)]
        let job = {
            use std::os::windows::io::AsRawHandle;
            rustmusic_core::WindowsJob::attach_raw(child.as_raw_handle() as isize)?
        };
        #[cfg(windows)]
        let started = Instant::now();
        #[cfg(windows)]
        let connection = loop {
            let result = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&socket);
            if let Ok(c) = result {
                break c;
            }
            if started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Не удалось запустить аудиоплеер");
            }
            std::thread::sleep(Duration::from_millis(30));
        };
        #[cfg(unix)]
        {
            connection.set_read_timeout(Some(Duration::from_secs(4)))?;
            connection.set_write_timeout(Some(Duration::from_secs(4)))?;
        }
        Ok(Self {
            child,
            connection,
            #[cfg(windows)]
            _job: job,
            request_id: 0,
            end_reason: None,
            loaded_at: None,
            seen_audio: false,
        })
    }
    fn note_load(&mut self) {
        self.end_reason = None;
        self.loaded_at = Some(Instant::now());
        self.seen_audio = false;
    }
    fn command(&mut self, command: Value) -> Result<Value> {
        self.request_id += 1;
        let id = self.request_id;
        let mut encoded = serde_json::to_vec(&json!({"command":command,"request_id":id}))?;
        encoded.push(b'\n');
        self.connection.write_all(&encoded)?;
        loop {
            let mut line = Vec::new();
            loop {
                let mut byte = [0];
                self.connection.read_exact(&mut byte)?;
                if byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
                if line.len() > 1024 * 1024 {
                    anyhow::bail!("Некорректный ответ mpv");
                }
            }
            let value: Value = serde_json::from_slice(&line)?;
            if value.get("event").and_then(|event| event.as_str()) == Some("end-file") {
                self.end_reason = value
                    .get("reason")
                    .and_then(|reason| reason.as_str())
                    .map(str::to_owned);
            }
            if value["request_id"].as_u64() == Some(id) {
                if value["error"]
                    .as_str()
                    .is_some_and(|e| e != "success" && e != "property unavailable")
                {
                    anyhow::bail!("Аудиоплеер отклонил команду");
                }
                return Ok(value["data"].clone());
            }
        }
    }
    fn property(&mut self, name: &str) -> Result<Value> {
        self.command(json!(["get_property", name]))
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn playback_failed(
    idle: bool,
    end_reason: Option<&str>,
    since_load: Duration,
    seen_audio: bool,
) -> bool {
    if !idle {
        return false;
    }
    if end_reason == Some("error") {
        return true;
    }
    // mpv stays idle for a moment after loadfile while the URL opens.
    // That gap is not a failed download.
    !seen_audio && since_load >= Duration::from_secs(15)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_while_opening_is_not_a_failed_download() {
        assert!(!playback_failed(
            true,
            None,
            Duration::from_millis(500),
            false
        ));
        assert!(!playback_failed(
            true,
            Some("stop"),
            Duration::from_secs(20),
            true
        ));
        assert!(!playback_failed(
            false,
            Some("error"),
            Duration::ZERO,
            false
        ));
        assert!(playback_failed(
            true,
            Some("error"),
            Duration::from_millis(100),
            false
        ));
        assert!(playback_failed(true, None, Duration::from_secs(15), false));
    }
    #[test]
    #[ignore = "Internal fixture used by the owner-exit test"]
    fn lifecycle_owner_fixture() {
        if std::env::var("RUSTMUSIC_LIFECYCLE_OWNER_ONLY").as_deref() != Ok("1") {
            return;
        }
        let mut engine = Engine::new().unwrap();
        engine.property("mpv-version").unwrap();
        println!("OWNED_PLAYER_PID={}", engine.child.id());
        std::io::stdout().flush().unwrap();
        // Deliberately skip Rust destructors, as macOS process termination can do.
        std::process::exit(0);
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "Requires mpv"]
    fn mpv_exits_without_owner_destructors() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "player::tests::lifecycle_owner_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("RUSTMUSIC_LIFECYCLE_OWNER_ONLY", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "Lifecycle owner failed");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let pid = stdout
            .lines()
            .find_map(|line| {
                line.split_once("OWNED_PLAYER_PID=")
                    .map(|(_, value)| value.trim().to_owned())
            })
            .unwrap();
        for _ in 0..50 {
            let state = Command::new("ps")
                .args(["-p", &pid, "-o", "stat="])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&state.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("mpv survived its owner process");
    }
    #[tokio::test]
    #[ignore = "requires network access, yt-dlp, Deno, mpv and an audio device"]
    async fn real_stream_play_pause_seek() {
        let stream = rustmusic_core::Resolver::new(None)
            .resolve("4D7u5KF7SP8", false)
            .await
            .unwrap();
        let player = Player::default();
        player.set_epoch(1);
        player.load(stream, 1).await.unwrap();
        let mut status = Status::default();
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            status = player.status().await.unwrap();
            if status.position > 0.5 && status.duration > 10.0 {
                break;
            }
        }
        assert!(status.position > 0.5, "stream did not start");
        assert!(status.duration > 10.0);
        player
            .command(json!(["set_property", "pause", true]))
            .await
            .unwrap();
        assert!(player.status().await.unwrap().paused);
        player
            .command(json!(["seek", 15, "absolute"]))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(player.status().await.unwrap().position >= 14.);
        player.command(json!(["stop"])).await.unwrap();
    }
}
