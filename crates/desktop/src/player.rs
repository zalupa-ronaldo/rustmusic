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
                Ok(Status {
                    position: e.property("time-pos")?.as_f64().unwrap_or(0.) as f32,
                    duration: e.property("duration")?.as_f64().unwrap_or(0.) as f32,
                    paused: e.property("pause")?.as_bool().unwrap_or(false),
                    ended: e.property("eof-reached")?.as_bool().unwrap_or(false),
                    failed: e.property("idle-active")?.as_bool().unwrap_or(false),
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
    _directory: tempfile::TempDir,
    request_id: u64,
}
impl Engine {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        #[cfg(unix)]
        let socket = directory
            .path()
            .join("mpv.sock")
            .to_string_lossy()
            .into_owned();
        #[cfg(windows)]
        let socket = format!(r"\\.\pipe\rustmusic-{}", std::process::id());
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
            .arg(format!("--input-ipc-server={socket}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Не найден mpv. Используйте полную сборку RustMusic.")?;
        let mut child = child;
        let started = Instant::now();
        let connection = loop {
            #[cfg(unix)]
            let result = Connection::connect(&socket);
            #[cfg(windows)]
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
            _directory: directory,
            request_id: 0,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
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
