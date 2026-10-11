# Validation

## Automated checks

- Workspace formatting, Clippy with warnings as errors, and unit tests.
- Queue end/repeat/shuffle semantics and empty-queue handling.
- Cookie domain/name filtering, redacted Debug, missing authentication and
  header injection rejection.
- Track/album/artist parsing and pagination on anonymized fixtures; unknown
  provider shapes produce errors instead of fake success.
- Audio proxy target validation.
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
7. Docker login, manual session import, browser audio and Range seeking.
8. Installed `.app` starts without Homebrew/Python in PATH, helpers have no
   Homebrew library references, bundle signature verifies.
9. Full 30-minute RAM comparison as described in PERFORMANCE.md.

Account-specific checks remain pending until the user connects a Google
session. Never use mock responses as evidence that those checks passed.

## Recorded local validation — 2026-10-11

- Formatting, Clippy (`--workspace --all-targets -D warnings`), 12 unit tests
  and the TypeScript/Vite build passed on Apple Silicon.
- Live public search, home pagination, album/artist browsing and a 50-track
  radio response passed; no mock provider was used.
- A real mpv stream passed play, pause and seek assertions. The same test
  passed with helpers from the signed app bundle and PATH limited to
  `/usr/bin:/bin`. The installed app opened from `/Applications`.
- Docker Compose built and became healthy. Browser audio played a real track
  (369.63 seconds), and the seek control moved to 120 seconds; pause was
  confirmed with no audio error.
- The ad-hoc bundle signature passed deep/strict verification. This build
  requires macOS 26.0 or newer.
- Google-account checks, macOS hardware media keys and the full 30-minute
  comparative RAM target are not yet recorded as passed.
