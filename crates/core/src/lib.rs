pub mod auth;
pub mod cache;
mod helper;
pub mod model;
pub mod provider;
pub mod queue;
pub mod resolver;
pub use auth::Session;
pub use cache::{CacheConfig, TrackCache, default_cache_dir};
pub use model::*;
pub use provider::MusicClient;
pub use queue::{Queue, Repeat};
pub use resolver::{Resolver, Stream};

#[cfg(windows)]
pub use helper::WindowsJob;
