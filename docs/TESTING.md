# Validation

## Automated checks

- Workspace formatting, Clippy with warnings as errors, and unit tests.
- Queue end/repeat/shuffle semantics and empty-queue handling.
- Cookie domain/name filtering, redacted Debug, missing authentication and
  header injection rejection.
- Track/album/artist parsing and pagination on anonymized fixtures; unknown
  provider shapes produce errors instead of fake success.
- Audio proxy target validation.
- Disk-cache LRU eviction, disabled/oversized entries, partial-file cleanup,
  index rebuild, and cache-hit lookup staying under a network-resolve budget.
- Stream URL expiry bounds and in-memory descriptor eviction.
- Playback planning stops the current track before a resolve and skips the
  resolve when the track is already cached.
- Expired/forbidden sessions (401/403), service failures, malformed responses
  and connection failures against an isolated local fixture server.
- Cancellation terminates forked resolver children, not just their parent.
- A separate owner process exits without running Rust destructors; its mpv
  child must still terminate (live ignored test).
- AES-GCM round trip, wrong-key and ciphertext-tamper rejection.
- Local API authentication, HttpOnly/SameSite cookie and cross-origin rejection.
- TypeScript checking and production Vite build.

## Live checks

`probe` and `probe_stream` use public YouTube Music/YouTube, without importing
any browser credentials. `real_stream_play_pause_seek` plays a real public
track using mpv; it is ignored in CI because it needs network and audio output.

For account validation, connect your session through settings and check:

1. The displayed Google channel/account is the intended one; repeat with a
   different `auth_user` if multiple Google accounts exist.
2. Library, liked songs, recommendations, albums, artists, radio and pagination.
3. Playback, seek, next/previous, repeat, shuffle, queue and macOS media keys.
4. Create a **temporary private test playlist**, add two tracks, rename it,
   move/remove a track, refresh to verify persistence, then delete it.
5. Like/unlike a chosen test track and verify on music.youtube.com.
6. Disconnect, relaunch, confirm credentials are removed; expired cookies
   produce a reconnect prompt. Cancel browser Keychain access and try manual import.
   On macOS the session is a login-keychain item written by `/usr/bin/security`,
   not `~/Library/Application Support/rustmusic/session.json`. Quit and relaunch
   the same build, then rebuild the ad-hoc app (`python3 scripts/package-macos.py`),
   replace it, and relaunch: the account must still be signed in, with no
   keychain prompt. A quarantined Finder launch (App Translocation) must keep
   the session too. Sign-out removes the `beer.rustmusic.app` items.
7. Docker login, manual session import, browser audio and Range seeking.
8. Installed `.app` starts without Homebrew/Python in PATH, helpers have no
   Homebrew library references, bundle signature verifies.
9. Full 30-minute RAM comparison as described in PERFORMANCE.md.

Account tests require explicit opt-in and a session already connected by the
user. Never use mock responses as evidence that live checks passed.

```sh
RUSTMUSIC_LIVE_ACCOUNT=1 cargo run -p rustmusic --example live_account
```

This creates and deletes a disposable private playlist and briefly removes then
restores the first liked track. It prints no credentials or personal track data.
Add `RUSTMUSIC_TEST_DOCKER=1` to import that session into the local Docker server
and verify its library.

## Recorded local validation — 2026-10-11

- Formatting, Clippy (`--workspace --all-targets -D warnings`), 15 unit tests
  and the TypeScript/Vite build passed on Apple Silicon.
- Live public search, home pagination, album/artist browsing and a 50-track
  radio response passed; no mock provider was used.
- A real mpv stream passed play, pause and seek assertions. The same test
  passed with helpers from the signed app bundle and PATH limited to
  `/usr/bin:/bin`. The installed app opened from `/Applications`.
- Docker Compose built and became healthy. Browser audio played a real track
  (369.63 seconds), and the seek control moved to 120 seconds; pause was
  confirmed with no audio error.
- An unavailable public video ID produced an explicit resolver error.
- The ad-hoc bundle signature passed deep/strict verification. This build
  requires macOS 26.0 or newer.
- Player ownership/IPC-disconnect termination passed with bundled mpv.
- The user-authorized Chrome Default/account 0 session was imported, its
  account was verified, and Keychain persistence survived app relaunch.
- Authenticated recommendations, liked songs, playlists, albums (empty for
  this account), artist library and radio passed against the real service.
- A private disposable playlist passed create, add, rename, reorder and
  remove verification via refreshed reads; it was deleted afterwards.
- Unlike/like passed against the track's own `likeStatus`, with the original
  like restored. The library list can lag behind the mutation response.
- Docker accepted the authenticated session, encrypted it in its volume and
  returned the personal library. Browser mobile layout was inspected at
  390×844.
- GitHub CI passed on macOS, Linux and Windows, plus web and Docker jobs.
  The macOS packaging workflow also passed on a fresh runner.
- macOS hardware media keys and the full 30-minute comparative RAM target
  are not yet recorded as passed.
