use crate::player::Player;
use anyhow::Result;
use rustmusic_core::{CacheConfig, Resolver, Stream, TrackCache, valid_video_id};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartKind {
    Cached,
    Network,
    Stale,
}
/// Order of work for a track change. `stop` is always first, before any resolve.
#[cfg(test)]
fn plan(cached: bool, prefetch: bool) -> &'static [&'static str] {
    match (cached, prefetch) {
        (true, true) => &["stop", "load-cache", "prefetch"],
        (true, false) => &["stop", "load-cache"],
        (false, true) => &["stop", "resolve", "load-remote", "store", "prefetch"],
        (false, false) => &["stop", "resolve", "load-remote", "store"],
    }
}
#[derive(Clone)]
pub struct Playback {
    inner: Arc<Inner>,
}
struct Inner {
    epoch: AtomicU64,
    cache: Mutex<TrackCache>,
    prefetch: Mutex<Option<Prefetch>>,
    /// Download starts only after mpv has actually produced audio.
    pending_store: Mutex<Option<PendingStore>>,
}
struct PendingStore {
    epoch: u64,
    id: String,
    stream: Stream,
}
struct Prefetch {
    id: String,
    handle: AbortJoin<Result<Stream>>,
}
struct AbortJoin<T> {
    handle: Option<JoinHandle<T>>,
}
impl<T> AbortJoin<T> {
    fn new(handle: JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }
    async fn wait(mut self) -> std::result::Result<T, tokio::task::JoinError> {
        self.handle.take().expect("prefetch is joined once").await
    }
    async fn cancel(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}
impl<T> Drop for AbortJoin<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
impl Playback {
    pub fn open() -> Self {
        Self::new(open_cache(CacheConfig::load()))
    }
    fn new(cache: TrackCache) -> Self {
        Self {
            inner: Arc::new(Inner {
                epoch: AtomicU64::new(0),
                cache: Mutex::new(cache),
                prefetch: Mutex::new(None),
                pending_store: Mutex::new(None),
            }),
        }
    }
    pub fn cache(&self) -> TrackCache {
        lock(&self.inner.cache).clone()
    }
    pub fn set_cache(&self, cache: TrackCache) {
        *lock(&self.inner.cache) = cache;
    }
    pub fn arm(&self, epoch: u64) {
        self.inner.epoch.store(epoch, Ordering::SeqCst);
    }
    pub fn cancel_prefetch(&self) {
        lock(&self.inner.prefetch).take();
    }
    /// Remember the stream, but do not download it until playback is audible.
    pub fn stage_store(&self, epoch: u64, id: &str, stream: Stream) {
        if self.inner.epoch.load(Ordering::SeqCst) != epoch || !valid_video_id(id) {
            return;
        }
        *lock(&self.inner.pending_store) = Some(PendingStore {
            epoch,
            id: id.to_owned(),
            stream,
        });
    }
    /// Start the disk download once `time-pos` or `duration` has been seen.
    pub fn commit_store(&self, epoch: u64, id: &str) {
        let pending = {
            let mut slot = lock(&self.inner.pending_store);
            let matches = slot
                .as_ref()
                .is_some_and(|pending| pending.epoch == epoch && pending.id == id);
            if matches { slot.take() } else { None }
        };
        if let Some(pending) = pending
            && self.inner.epoch.load(Ordering::SeqCst) == epoch
        {
            self.cache().remember(&pending.id, pending.stream);
        }
    }
    pub fn drop_pending(&self, id: &str) {
        let mut slot = lock(&self.inner.pending_store);
        if slot.as_ref().is_some_and(|pending| pending.id == id) {
            slot.take();
        }
    }
    pub fn clear_pending(&self, epoch: u64) {
        let mut slot = lock(&self.inner.pending_store);
        if self.inner.epoch.load(Ordering::SeqCst) == epoch {
            slot.take();
        }
    }
    pub fn begin_prefetch(&self, epoch: u64, resolver: Resolver, id: String) {
        if self.inner.epoch.load(Ordering::SeqCst) != epoch || !valid_video_id(&id) {
            return;
        }
        if self.cache().lookup(&id).is_some() {
            return;
        }
        let mut slot = lock(&self.inner.prefetch);
        if self.inner.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        if slot.as_ref().is_some_and(|prefetch| prefetch.id == id) {
            return;
        }
        slot.take();
        let prefetch_id = id.clone();
        let handle = tokio::spawn(async move { resolver.resolve(&prefetch_id, false).await });
        if self.inner.epoch.load(Ordering::SeqCst) != epoch {
            handle.abort();
            return;
        }
        *slot = Some(Prefetch {
            id,
            handle: AbortJoin::new(handle),
        });
    }
    async fn claim(&self, id: &str) -> Option<AbortJoin<Result<Stream>>> {
        let taken = lock(&self.inner.prefetch).take();
        match taken {
            Some(prefetch) if prefetch.id == id => Some(prefetch.handle),
            Some(prefetch) => {
                prefetch.handle.cancel().await;
                None
            }
            None => None,
        }
    }
}
pub async fn start(
    playback: &Playback,
    player: &Player,
    resolver: &Resolver,
    video_id: &str,
    next_id: Option<String>,
    epoch: u64,
) -> Result<StartKind> {
    // Stop before resolve/cache lookup so the previous track ends at the switch,
    // not when the next file finally loads. See `plan`.
    let _ = player.stop().await;
    playback.clear_pending(epoch);
    if player.epoch() != epoch {
        return Ok(StartKind::Stale);
    }
    let cache = playback.cache();
    let lookup_id = video_id.to_owned();
    let cached = tokio::task::spawn_blocking(move || cache.lookup(&lookup_id))
        .await
        .ok()
        .flatten();
    if let Some(path) = cached {
        player.load_path(path, epoch).await?;
        prefetch_next(playback, player, resolver, next_id, epoch);
        return Ok(StartKind::Cached);
    }
    let stream = if let Some(prefetched) = playback.claim(video_id).await {
        match prefetched.wait().await {
            Ok(Ok(stream)) => stream,
            _ => resolver.resolve(video_id, false).await?,
        }
    } else {
        resolver.resolve(video_id, false).await?
    };
    if player.epoch() != epoch {
        return Ok(StartKind::Stale);
    }
    player.load(stream.clone(), epoch).await?;
    // Download only after mpv reports audio, so the cache client does not
    // race the first open of this URL.
    playback.stage_store(epoch, video_id, stream);
    prefetch_next(playback, player, resolver, next_id, epoch);
    Ok(StartKind::Network)
}
fn prefetch_next(
    playback: &Playback,
    player: &Player,
    resolver: &Resolver,
    next_id: Option<String>,
    epoch: u64,
) {
    if player.epoch() == epoch
        && let Some(next) = next_id
    {
        playback.begin_prefetch(epoch, resolver.clone(), next);
    }
}
fn open_cache(config: CacheConfig) -> TrackCache {
    if let Ok(cache) = TrackCache::open(config.clone()) {
        return cache;
    }
    let fallback = CacheConfig {
        directory: std::env::temp_dir().join("rustmusic-cache"),
        limit_bytes: config.limit_bytes,
    };
    TrackCache::open(fallback).unwrap_or_else(|_| {
        TrackCache::open(CacheConfig {
            directory: std::env::temp_dir().join(format!("rustmusic-{}", std::process::id())),
            limit_bytes: 0,
        })
        .expect("Не удалось создать каталог кэша")
    })
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_precedes_resolve_and_cache_skips_it() {
        let cold = plan(false, true);
        let warm = plan(true, true);
        assert_eq!(cold[0], "stop");
        assert!(
            cold.iter().position(|step| *step == "stop").unwrap()
                < cold.iter().position(|step| *step == "resolve").unwrap()
        );
        assert!(!warm.contains(&"resolve"));
        assert_eq!(warm, ["stop", "load-cache", "prefetch"]);
        assert!(!plan(true, false).contains(&"resolve"));
    }
}
