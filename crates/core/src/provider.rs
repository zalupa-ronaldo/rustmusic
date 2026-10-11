use crate::{Account, Item, ItemKind, Mutation, Page, Session, valid_browse_id, valid_video_id};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct MusicClient {
    http: reqwest::Client,
    session: Option<Session>,
    context: Arc<OnceCell<Value>>,
}
impl MusicClient {
    pub fn new(session: Option<Session>) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(25))
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36")
                .build()?,
            session,
            context: Arc::new(OnceCell::new()),
        })
    }
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }
    pub fn authenticated(&self) -> bool {
        self.session.is_some()
    }
    async fn context(&self) -> Result<Value> {
        self.context.get_or_try_init(|| async {
            let page=self.http.get("https://music.youtube.com/").send().await.map_err(|_| anyhow::anyhow!("Не удалось подключиться к YouTube Music"))?.text().await.map_err(|_| anyhow::anyhow!("Не удалось загрузить настройки YouTube Music"))?;
            for (offset,_) in page.match_indices("ytcfg.set(") {
                let mut stream=serde_json::Deserializer::from_str(&page[offset+10..]).into_iter::<Value>();
                if let Some(Ok(config))=stream.next() {
                    if let Some(context)=config.get("INNERTUBE_CONTEXT") { return Ok(context.clone()); }
                    if let Some(version)=config.get("INNERTUBE_CLIENT_VERSION").and_then(Value::as_str) {
                        return Ok(json!({"client":{"clientName":"WEB_REMIX","clientVersion":version,"hl":"ru"},"user":{}}));
                    }
                }
            }
            bail!("YouTube Music изменил настройки клиента. Требуется обновление RustMusic.")
        }).await.cloned()
    }
    pub async fn request(&self, endpoint: &str, mut body: Value) -> Result<Value> {
        let mut context = self.context().await?;
        context["client"]["hl"] = json!("ru");
        body["context"] = context;
        let mut req = self
            .http
            .post(format!(
                "https://music.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false"
            ))
            .header("origin", "https://music.youtube.com")
            .header("referer", "https://music.youtube.com/")
            .json(&body);
        if let Some(session) = &self.session {
            req = req
                .header("cookie", &session.cookie)
                .header("x-goog-authuser", &session.auth_user)
                .header(
                    "authorization",
                    session
                        .browser_auth()
                        .get_authorization()
                        .map_err(|_| anyhow::anyhow!("Сессия повреждена"))?,
                );
        }
        let response = req
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Ошибка сети: YouTube Music не отвечает"))?;
        if matches!(response.status().as_u16(), 401 | 403) {
            bail!("Сессия истекла или доступ запрещён. Подключите аккаунт заново.");
        }
        if !response.status().is_success() {
            bail!("YouTube Music вернул ошибку {}", response.status().as_u16());
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("Неизвестный формат ответа YouTube Music"))?;
        if value.get("error").is_some() {
            bail!("YouTube Music отклонил запрос");
        }
        Ok(value)
    }
    pub async fn account(&self) -> Result<Account> {
        let session = self
            .session
            .as_ref()
            .context("Сначала подключите аккаунт")?;
        let response = self.request("account/account_menu", json!({})).await?;
        let header = find_key(&response, "activeAccountHeaderRenderer")
            .context("Сессия не подтверждена. Проверьте номер аккаунта и повторите вход.")?;
        let name = text(&header["accountName"]);
        if name.is_empty() {
            bail!("YouTube Music не подтвердил выбранный аккаунт");
        }
        Ok(Account {
            name,
            handle: text(&header["channelHandle"]),
            auth_user: session.auth_user.clone(),
        })
    }
    pub async fn search(&self, query: &str, continuation: Option<&str>) -> Result<Page> {
        if query.trim().is_empty() || query.len() > 500 {
            bail!("Введите запрос длиной до 500 символов");
        }
        let body = if let Some(token) = continuation {
            json!({"continuation":token})
        } else {
            json!({"query":query})
        };
        let value = self.request("search", body).await?;
        parse_checked(&value)
    }
    pub async fn browse(&self, id: &str, continuation: Option<&str>) -> Result<Page> {
        if !valid_browse_id(id) {
            bail!("Некорректный идентификатор раздела");
        }
        if id.starts_with("FEmusic_liked") && !self.authenticated() {
            bail!("Подключите аккаунт для доступа к библиотеке");
        }
        let id = if id.starts_with("PL") || id == "LM" || id.starts_with("RD") {
            format!("VL{id}")
        } else {
            id.to_owned()
        };
        let body = if let Some(token) = continuation {
            json!({"continuation":token})
        } else {
            json!({"browseId":id})
        };
        parse_checked(&self.request("browse", body).await?)
    }
    pub async fn radio(&self, video_id: &str, continuation: Option<&str>) -> Result<Page> {
        if !valid_video_id(video_id) {
            bail!("Некорректный идентификатор трека");
        }
        let body = if let Some(token) = continuation {
            json!({"continuation":token})
        } else {
            json!({"videoId":video_id,"playlistId":format!("RDAMVM{video_id}"),"isAudioOnly":true})
        };
        parse_checked(&self.request("next", body).await?)
    }
    pub async fn mutate(&self, action: Mutation) -> Result<Value> {
        let session = self.session.as_ref().context("Подключите аккаунт")?;
        let client = ytmusicapi::YTMusicClient::builder()
            .with_browser_auth(session.browser_auth())
            .with_language("ru")
            .build()?;
        let task = async {
            match action {
                Mutation::Like { video_id, liked } => {
                    check_video(&video_id)?;
                    if liked {
                        client.like_song(&video_id).await.map_err(provider_error)?;
                    } else {
                        client
                            .unlike_song(&video_id)
                            .await
                            .map_err(provider_error)?;
                    }
                    Ok(json!({"ok":true}))
                }
                Mutation::Create { title } => {
                    check_title(&title)?;
                    let result = client
                        .create_playlist(&title, None, ytmusicapi::Privacy::Private)
                        .await
                        .map_err(provider_error)?;
                    Ok(json!({"playlist_id":result.playlist_id}))
                }
                Mutation::Delete { playlist_id } => {
                    check_playlist(&playlist_id)?;
                    client
                        .delete_playlist(&playlist_id)
                        .await
                        .map_err(provider_error)?;
                    Ok(json!({"ok":true}))
                }
                Mutation::Add {
                    playlist_id,
                    video_id,
                } => {
                    check_playlist(&playlist_id)?;
                    check_video(&video_id)?;
                    client
                        .add_playlist_items(&playlist_id, &[video_id], false)
                        .await
                        .map_err(provider_error)?;
                    Ok(json!({"ok":true}))
                }
                Mutation::Rename { playlist_id, title } => {
                    check_playlist(&playlist_id)?;
                    check_title(&title)?;
                    self.edit(
                        &playlist_id,
                        json!([{"action":"ACTION_SET_PLAYLIST_NAME","playlistName":title}]),
                    )
                    .await
                }
                Mutation::Remove {
                    playlist_id,
                    video_id,
                    set_video_id,
                } => {
                    check_playlist(&playlist_id)?;
                    check_video(&video_id)?;
                    check_set_id(&set_video_id)?;
                    self.edit(&playlist_id,json!([{"action":"ACTION_REMOVE_VIDEO","setVideoId":set_video_id,"removedVideoId":video_id}])).await
                }
                Mutation::Move {
                    playlist_id,
                    set_video_id,
                    before_set_video_id,
                } => {
                    check_playlist(&playlist_id)?;
                    check_set_id(&set_video_id)?;
                    check_set_id(&before_set_video_id)?;
                    self.edit(&playlist_id,json!([{"action":"ACTION_MOVE_VIDEO_BEFORE","setVideoId":set_video_id,"movedSetVideoIdSuccessor":before_set_video_id}])).await
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(30), task)
            .await
            .context("Время операции истекло. Обновите список перед повтором.")?
    }
    async fn edit(&self, id: &str, actions: Value) -> Result<Value> {
        let value = self
            .request(
                "browse/edit_playlist",
                json!({"playlistId":id.strip_prefix("VL").unwrap_or(id),"actions":actions}),
            )
            .await?;
        if value["status"]
            .as_str()
            .is_none_or(|s| !s.contains("SUCCEEDED"))
        {
            bail!("Изменение плейлиста не подтверждено. Проверьте права владельца.");
        }
        Ok(json!({"ok":true}))
    }
}
fn provider_error(_: ytmusicapi::Error) -> anyhow::Error {
    anyhow::anyhow!("YouTube Music не подтвердил изменение. Проверьте сессию и права на плейлист.")
}
fn check_video(id: &str) -> Result<()> {
    if !valid_video_id(id) {
        bail!("Некорректный трек");
    }
    Ok(())
}
fn check_playlist(id: &str) -> Result<()> {
    if !valid_browse_id(id) {
        bail!("Некорректный плейлист");
    }
    Ok(())
}
fn check_set_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 256 || id.contains(['\r', '\n']) {
        bail!("Некорректный элемент плейлиста");
    }
    Ok(())
}
fn check_title(title: &str) -> Result<()> {
    if title.trim().is_empty() || title.len() > 300 {
        bail!("Название должно содержать от 1 до 300 символов");
    }
    Ok(())
}

pub fn text(value: &Value) -> String {
    if let Some(s) = value["simpleText"].as_str() {
        return s.to_owned();
    }
    value["runs"]
        .as_array()
        .map(|runs| {
            runs.iter()
                .filter_map(|r| r["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}
pub fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(o) => o
            .get(key)
            .or_else(|| o.values().find_map(|v| find_key(v, key))),
        Value::Array(a) => a.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}
fn thumbnail(v: &Value) -> Option<String> {
    let thumbs = find_key(v, "thumbnails")?.as_array()?;
    thumbs
        .iter()
        .rfind(|t| t["width"].as_u64().unwrap_or(120) <= 360)
        .or_else(|| thumbs.first())?["url"]
        .as_str()
        .map(str::to_owned)
}
fn parse_renderer(v: &Value) -> Option<Item> {
    let watch=v.get("navigationEndpoint").and_then(|n|n.get("watchEndpoint"))
        .or_else(|| v.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint"))
        .or_else(||v.pointer("/playlistItemData"));
    let browse = v.pointer("/navigationEndpoint/browseEndpoint");
    let flex = v.get("flexColumns").and_then(Value::as_array);
    let title = if let Some(columns) = flex {
        text(&columns.first()?["musicResponsiveListItemFlexColumnRenderer"]["text"])
    } else {
        text(
            v.get("title")
                .or_else(|| v.get("headline"))
                .unwrap_or(&Value::Null),
        )
    };
    let subtitle = if let Some(columns) = flex {
        columns
            .iter()
            .skip(1)
            .map(|c| text(&c["musicResponsiveListItemFlexColumnRenderer"]["text"]))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    } else {
        text(&v["subtitle"])
    };
    let browse_id = browse.and_then(|b| b["browseId"].as_str()).or_else(|| {
        flex.and_then(|c| c.first())
            .and_then(|c| {
                find_key(
                    &c["musicResponsiveListItemFlexColumnRenderer"]["text"],
                    "browseEndpoint",
                )
            })
            .and_then(|b| b["browseId"].as_str())
    });
    let video_id = watch
        .and_then(|w| w["videoId"].as_str())
        .or_else(|| v["videoId"].as_str());
    let (id, kind) = if let Some(b) = browse_id {
        let kind = if b.starts_with("MPRE") {
            ItemKind::Album
        } else if b.starts_with("UC") {
            ItemKind::Artist
        } else {
            ItemKind::Playlist
        };
        (b, kind)
    } else if let Some(id) = video_id {
        (id, ItemKind::Track)
    } else {
        return None;
    };
    if title.is_empty() || (kind == ItemKind::Track && !valid_video_id(id)) {
        return None;
    }
    let duration = v
        .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text")
        .map(text)
        .or_else(|| v.get("lengthText").map(text))
        .filter(|s| !s.is_empty());
    let set_video_id = v
        .pointer("/playlistItemData/playlistSetVideoId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(Item {
        id: id.into(),
        title,
        subtitle,
        kind,
        thumbnail: thumbnail(v),
        duration,
        set_video_id,
    })
}
fn collect(v: &Value, items: &mut Vec<Item>) {
    match v {
        Value::Object(o) => {
            for key in [
                "musicResponsiveListItemRenderer",
                "musicTwoRowItemRenderer",
                "playlistPanelVideoRenderer",
            ] {
                if let Some(renderer) = o.get(key) {
                    if let Some(item) = parse_renderer(renderer) {
                        items.push(item);
                    }
                    return;
                }
            }
            for child in o.values() {
                collect(child, items);
            }
        }
        Value::Array(a) => {
            for child in a {
                collect(child, items);
            }
        }
        _ => {}
    }
}
pub fn parse_page(v: &Value) -> Page {
    let mut items = Vec::new();
    collect(v, &mut items);
    let mut seen = HashSet::new();
    items.retain(|i| seen.insert((i.id.clone(), i.set_video_id.clone())));
    items.truncate(100);
    let continuation = find_key(v, "nextContinuationData")
        .or_else(|| find_key(v, "reloadContinuationData"))
        .and_then(|d| d["continuation"].as_str())
        .or_else(|| find_key(v, "continuationCommand").and_then(|d| d["token"].as_str()))
        .map(str::to_owned);
    Page {
        items,
        continuation,
    }
}
fn parse_checked(v: &Value) -> Result<Page> {
    let page = parse_page(v);
    if page.items.is_empty()
        && find_key(v, "musicShelfRenderer").is_none()
        && find_key(v, "musicGridRenderer").is_none()
        && find_key(v, "sectionListRenderer").is_none()
        && find_key(v, "messageRenderer").is_none()
        && find_key(v, "continuationContents").is_none()
    {
        bail!("YouTube Music вернул неподдерживаемый ответ. Попробуйте обновить клиент.");
    }
    Ok(page)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_parses_tracks_and_collections() {
        let v: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/browse.json")).unwrap();
        let p = parse_page(&v);
        assert_eq!(p.items.len(), 3);
        assert_eq!(p.items[0].kind, ItemKind::Track);
        assert_eq!(p.items[0].set_video_id.as_deref(), Some("set-fixture"));
        assert_eq!(p.items[1].kind, ItemKind::Album);
        assert_eq!(p.items[2].kind, ItemKind::Artist);
        assert_eq!(p.continuation.as_deref(), Some("fixture-page-2"));
    }
    #[test]
    fn schema_changes_are_not_silent() {
        assert!(parse_checked(&json!({"unknown":{}})).is_err());
    }
}
