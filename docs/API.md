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
| GET | `/status` | `{account: {name,handle,auth_user} | null, version}` |
| POST | `/session` | `{input,auth_user:"0"}`; validates Google session, encrypts at rest |
| DELETE | `/session` | Deletes encrypted Google session and clears queue |
| GET | `/search?q=…&continuation=…` | `Page` |
| GET | `/browse/{id}?continuation=…` | `Page` |
| GET | `/radio/{video_id}?continuation=…` | `Page` |
| POST | `/mutate` | Tagged mutation; `{ok:true}` or `{playlist_id}` |
| GET | `/queue` | Shared-core `Queue` |
| POST | `/queue` | Tagged queue command; updated `Queue` |
| POST | `/resolve/{video_id}` | `{url:"/api/v1/audio/{video_id}"}` |
| GET | `/audio/{video_id}` | Streamed audio; forwards Range, 206/416 and range headers |
| GET | `/events` | SSE `progress` events containing operation status strings |

`Page`: `{items: Item[], continuation: string | null}`. UI retains up to 500
items; provider parses at most 100 items per response. Continuation is opaque.
`Item`: `{id,title,subtitle,kind,thumbnail,duration,set_video_id}`; nullable
optional fields; `kind` is `track`, `album`, `artist` or `playlist`.

Mutation actions:

- `like`: `video_id`, `liked`.
- `create`: `title`; always private.
- `rename`: `playlist_id`, `title`.
- `delete`: `playlist_id`.
- `add`: `playlist_id`, `video_id`; skips duplicates.
- `remove`: `playlist_id`, `video_id`, `set_video_id`.
- `move`: `playlist_id`, `set_video_id`, `before_set_video_id`.

Queue actions: `replace` (`items`, `index`), `append` (`item`), `select`
(`index`), `next` (`ended`, default false), `previous`, `shuffle`, `repeat`.
`Queue`: `{items,index: number | null,repeat: "off" | "all" | "one"}`.
The queue is in-memory and capped at 500 tracks. Explicit next skips repeat-one;
end-of-track repeats the current item. Browser sound belongs to the browser,
not the server; multiple browser tabs are not independent music sessions.

Audio proxy targets come exclusively from yt-dlp and must be HTTPS subdomains
of `googlevideo.com`. Redirects are disabled. URLs, cookies and request headers
are never returned to the frontend. Stream metadata is cached for four minutes,
maximum eight entries; audio is never persisted.
