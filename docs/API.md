# Local server API v1

Base: `/api/v1`. JSON requests use `Content-Type: application/json` and
`X-RustMusic: 1` for state-changing calls. Authentication uses the HttpOnly,
SameSite=Strict `rustmusic_session` cookie. Cross-origin calls are rejected.
Error responses have `{ "error": "human-readable message" }`; secrets and
stream URLs are omitted. The server supports one local user/account.

| Method | Path | Input / output |
|---|---|---|
| POST | `/login` | `{password}` is the local server key; sets session cookie |
| POST | `/logout` | Clears browser cookie; does not delete Google credentials |
| GET | `/status` | `{account: {name,handle,auth_user} | null, version, commit}` |
| POST | `/session` | `{input,auth_user:"0"}`; validates Google session, encrypts at rest |
| DELETE | `/session` | Deletes encrypted Google session and clears queue |
| GET | `/search?q=…&continuation=…` | `Page` |
| GET | `/browse/{id}?continuation=…` | `Page` |
| GET | `/radio/{video_id}?continuation=…` | `Page` |
| POST | `/mutate` | Tagged mutation; `{ok:true}` or `{playlist_id}` |
| GET | `/queue` | Shared-core `Queue` |
| POST | `/queue` | Tagged queue command; updated `Queue` |
| POST | `/resolve/{video_id}` | `{url:"/api/v1/audio/{video_id}"}` |
| GET | `/audio/{video_id}` | Streamed audio; forwards Range, 206/416 and range headers. A pinned download is served from disk and does not call yt-dlp |
| GET | `/art?url=` | Cover image through the disk cache. Only YouTube image hosts. `Cache-Control: public, max-age=604800` |
| POST | `/download/{video_id}` | Starts a pinned download; `{state,progress}`. Ready tracks are not fetched again |
| GET | `/downloads` | `{items:[{id,state,progress}], bytes}`. `state` is `active`, `ready` or `failed` |
| GET | `/events` | SSE `progress` events containing operation status strings |

`Page`: `{items: Item[], continuation: string | null}`. UI retains up to 500
items; provider parses at most 100 items per response. Continuation is opaque.
`Item`: `{id,title,subtitle,kind,thumbnail,duration,set_video_id}`; nullable
optional fields; `kind` is `track`, `album`, `artist` or `playlist`.

Mutation actions:

- `like`: `video_id`, `liked`. A like is how YouTube Music keeps a track in the library.
- `dislike`: `video_id`.
- `create`: `title`; always private.
- `rename`: `playlist_id`, `title`.
- `delete`: `playlist_id`.
- `add`: `playlist_id`, `video_id`; skips duplicates.
- `remove`: `playlist_id`, `video_id`, `set_video_id`.
- `move`: `playlist_id`, `set_video_id`, `before_set_video_id`. An empty `before_set_video_id` moves the row to the end.

Queue actions: `replace` (`items`, `index`), `append` (`item`), `insert_next`
(`item`, plays after the current track), `select` (`index`), `next` (`ended`,
default false), `previous`, `shuffle`, `repeat`, `reorder` (`from`, `to`,
final index after the move).
`Queue`: `{items,index: number | null,repeat: "off" | "all" | "one"}`.
The queue is in-memory and capped at 500 tracks. Explicit next skips repeat-one;
end-of-track repeats the current item. Browser sound belongs to the browser,
not the server; multiple browser tabs are not independent music sessions.

Audio proxy targets come exclusively from yt-dlp and must be HTTPS subdomains
of `googlevideo.com`. Redirects are disabled. URLs, cookies and request headers
are never returned to the frontend. Stream descriptors stay in memory until the
URL `expire` parameter, capped at 15 minutes, with at most 16 entries. The
server does not persist the temporary playback cache. Explicit downloads are
the exception: `POST /download/{video_id}` stores the file under
`RUSTMUSIC_DATA_DIR/downloads` and never evicts it. That store is not limited
by the playback-cache budget; one file may be up to 1 GiB. Playback of that id serves
the local file, including `Range`. Cover art is a separate 64 MB LRU under
`covers/`. `commit` is the short git hash baked in at build time (`-dirty`
when the tree was dirty). The desktop app separately caches played audio on
disk and keeps the same pinned downloads under its config directory.
