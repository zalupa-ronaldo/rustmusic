pub mod art;
pub mod auth;
pub mod cache;
pub mod downloads;
mod helper;
pub mod model;
pub mod provider;
pub mod queue;
pub mod resolver;
pub use art::{ArtCache, allowed_artwork_url, default_covers_dir};
pub use auth::Session;
pub use cache::{CacheConfig, TrackCache, clear_failed_open, config_dir, default_cache_dir};
pub use downloads::{
    BadByteRange, DownloadInfo, DownloadPhase, DownloadStore, default_downloads_dir,
    http_byte_range,
};
pub use model::*;
pub use provider::MusicClient;
pub use queue::{PlaylistReorder, Queue, Repeat, playlist_reorder, reorder_vec};
pub use resolver::{Resolver, Stream};

pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn app_commit() -> &'static str {
    env!("RUSTMUSIC_COMMIT")
}

#[cfg(windows)]
pub use helper::WindowsJob;

#[cfg(test)]
mod tests {
    use super::{app_commit, app_version};

    #[test]
    fn build_identity_is_a_public_label() {
        let commit = app_commit();
        let core = commit.strip_suffix("-dirty").unwrap_or(commit);
        let hex =
            (7..=40).contains(&core.len()) && core.bytes().all(|byte| byte.is_ascii_hexdigit());
        assert!(core == "unknown" || hex, "{commit}");
        assert!(!commit.contains([' ', '/', '=', '\n']));
        assert_eq!(app_version(), env!("CARGO_PKG_VERSION"));
    }
}
