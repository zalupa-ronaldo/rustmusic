# Memory measurements

The target is at least 50% less average RAM than the YouTube Music website.
This is an acceptance target, not a verified claim. Authenticated 30-minute
comparison has not yet been completed.

The native client uses Iced's tiny-skia software renderer; Chromium/WebKit are
not included. mpv is started only when a track is selected. Its forward/backward
buffers are capped at 8/2 MiB. yt-dlp is a short-lived resolver, not a persistent
browser. The application keeps up to 48 artwork handles, 500 visible items,
and eight resolved stream descriptors. JPEG/PNG/WebP are the only enabled
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
