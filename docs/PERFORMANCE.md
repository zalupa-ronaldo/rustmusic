# Memory measurements

The target is at least 50% less average RAM than the YouTube Music website.
This is an acceptance target, not a verified claim. Authenticated 30-minute
comparison has not yet been completed.

The native client uses Iced's tiny-skia software renderer; Chromium/WebKit are
not included. mpv is warmed while idle and kept for the next track. Its
forward/backward buffers are capped at 8/2 MiB, the initial cache pause is off,
and the audio buffer is 0.1 s. yt-dlp is a short-lived resolver, not a persistent
browser. The application keeps up to 48 artwork handles, 500 visible items,
and sixteen resolved stream descriptors. JPEG/PNG/WebP are the only enabled
artwork codecs. Search and navigation cancel stale provider requests.

Measure the release build launched from Applications. Use a **separate browser
instance/profile** with only YouTube Music, the same account and the same track.
Compare the full native process tree (including mpv, yt-dlp and Deno) with the
full browser tree, not one application process against an entire normal browser
session. Total RSS includes shared pages and is not the same as macOS memory
pressure or physical footprint; report the metric consistently.

```sh
python3 scripts/measure-memory.py --native-pid NATIVE_PID --browser-pid BROWSER_PID
```

Run 10 minutes idle, 10 minutes playing the same track, and 10 minutes performing
the same searches and browsing actions. Record macOS/build/browser versions,
track selection, account type, average and peak RAM. The script collects no
cookies, URLs, titles or browser content. Raw samples and summary are saved to
`.data/performance`, ignored by Git. Add a sanitized result here only after the
complete scenario. If reduction is below 50%, profile artwork decoding, retained
lists, renderer allocations and helper-process peaks before making a claim.

## Attempted validation — 2026-10-11

Two local runs were excluded because native-window interactions changed the
intended phases. The website also remained paused while the native player
started, so the playback conditions did not match. No percentage or average
from these runs is published as an acceptance result. A coordinated 30-minute
run is still required.

The attempts exposed an mpv process surviving a GUI process exit. The Unix
player now uses an inherited socketpair with `--input-ipc-client`, whose
connection lifetime terminates mpv. Windows uses a kill-on-close job object.
A live test verifies termination after the owner exits without Rust destructors.
The IPC lifetime behavior is documented in the [mpv manual](https://mpv.io/manual/stable/#options-input-ipc-client).
