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
    #[cfg(test)]
    test_base: Option<String>,
    /// Homepage used to read Innertube config. Tests point this at a local server.
    #[cfg(test)]
    page_base: Option<String>,
}
impl MusicClient {
    pub fn new(session: Option<Session>) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(25))
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36")
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    let host = attempt.url().host_str().unwrap_or("");
                    if host == "music.youtube.com"
                        || host.ends_with(".youtube.com")
                        || host.ends_with(".googlevideo.com")
                        || host.ends_with(".ytimg.com")
                        || host == "127.0.0.1"
                        || host == "localhost"
                    {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()?,
            session,
            context: Arc::new(OnceCell::new()),
            #[cfg(test)]
            test_base: None,
            #[cfg(test)]
            page_base: None,
        })
    }
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }
    pub fn authenticated(&self) -> bool {
        self.session.is_some()
    }
    fn page_url(&self) -> String {
        #[cfg(test)]
        if let Some(base) = &self.page_base {
            return format!("{base}/");
        }
        "https://music.youtube.com/".to_owned()
    }
    /// Innertube config. A signed-in client must load this page with the session
    /// cookie: an anonymous `visitorData` makes library browse return the
    /// signed-out shell even when `account_menu` still shows the account name.
    async fn context(&self) -> Result<Value> {
        let http = self.http.clone();
        let page_url = self.page_url();
        let cookie = self.session.as_ref().map(|session| session.cookie.clone());
        let auth_user = self
            .session
            .as_ref()
            .map(|session| session.auth_user.clone());
        self.context
            .get_or_try_init(|| async move {
                let mut request = http.get(&page_url);
                if let Some(cookie) = &cookie {
                    request = request.header("cookie", cookie);
                }
                if let Some(auth_user) = &auth_user {
                    request = request.header("x-goog-authuser", auth_user);
                }
                let page = request
                    .send()
                    .await
                    .map_err(|_| anyhow::anyhow!("Не удалось подключиться к YouTube Music"))?
                    .text()
                    .await
                    .map_err(|_| anyhow::anyhow!("Не удалось загрузить настройки YouTube Music"))?;
                parse_innertube_context(&page)
            })
            .await
            .cloned()
    }
    pub async fn request(&self, endpoint: &str, mut body: Value) -> Result<Value> {
        let mut context = self.context().await?;
        context["client"]["hl"] = json!("ru");
        body["context"] = context;
        #[cfg(test)]
        let base = self
            .test_base
            .as_deref()
            .unwrap_or("https://music.youtube.com/youtubei/v1");
        #[cfg(not(test))]
        let base = "https://music.youtube.com/youtubei/v1";
        let mut req = self
            .http
            .post(format!("{base}/{endpoint}?prettyPrint=false"))
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
        parse_off_thread(self.request("search", body).await?).await
    }
    pub async fn browse(&self, id: &str, continuation: Option<&str>) -> Result<Page> {
        if !valid_browse_id(id) {
            bail!("Некорректный идентификатор раздела");
        }
        if library_requires_session(id) && !self.authenticated() {
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
        parse_off_thread(self.request("browse", body).await?).await
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
        parse_off_thread(self.request("next", body).await?).await
    }
    /// Fetch the Innertube context outside an abortable catalog request.
    pub async fn warm(&self) -> Result<()> {
        self.context().await.map(|_| ())
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
                Mutation::Dislike { video_id } => {
                    check_video(&video_id)?;
                    client
                        .rate_song(&video_id, ytmusicapi::LikeStatus::Dislike)
                        .await
                        .map_err(provider_error)?;
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
                    let mut action = json!({
                        "action": "ACTION_MOVE_VIDEO_BEFORE",
                        "setVideoId": set_video_id
                    });
                    if !before_set_video_id.is_empty() {
                        check_set_id(&before_set_video_id)?;
                        action["movedSetVideoIdSuccessor"] = json!(before_set_video_id);
                    }
                    self.edit(&playlist_id, json!([action])).await
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
fn library_requires_session(id: &str) -> bool {
    id.starts_with("FEmusic_liked")
        || id.starts_with("FEmusic_library")
        || id == "VLLM"
        || id == "LM"
}
async fn parse_off_thread(value: Value) -> Result<Page> {
    tokio::task::spawn_blocking(move || parse_checked(&value))
        .await
        .context("Не удалось разобрать ответ YouTube Music")?
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
    let watch = v
        .pointer("/navigationEndpoint/watchEndpoint")
        .or_else(|| v.pointer("/onTap/watchEndpoint"))
        .or_else(|| v.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint"))
        .or_else(|| v.pointer("/playlistItemData"));
    let browse = v
        .pointer("/navigationEndpoint/browseEndpoint")
        .or_else(|| v.pointer("/onTap/browseEndpoint"));
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
    let browse_id = browse
        .and_then(|endpoint| endpoint["browseId"].as_str())
        .or_else(|| run_browse_id(v.get("title")))
        .or_else(|| run_browse_id(v.get("subtitle")))
        .or_else(|| {
            flex.and_then(|columns| columns.first())
                .and_then(|column| {
                    find_key(
                        &column["musicResponsiveListItemFlexColumnRenderer"]["text"],
                        "browseEndpoint",
                    )
                })
                .and_then(|endpoint| endpoint["browseId"].as_str())
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
fn run_browse_id(value: Option<&Value>) -> Option<&str> {
    value?
        .get("runs")?
        .as_array()?
        .iter()
        .find_map(|run| run.pointer("/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(Value::as_str)
}
fn parse_lockup(v: &Value) -> Option<Item> {
    let title = v
        .pointer("/metadata/lockupMetadataViewModel/title/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if title.is_empty() {
        return None;
    }
    let content_type = v.get("contentType").and_then(Value::as_str).unwrap_or("");
    let content_id = v.get("contentId").and_then(Value::as_str);
    let video_id = v
        .pointer("/rendererContext/commandContext/onTap/innertubeCommand/watchEndpoint/videoId")
        .and_then(Value::as_str)
        .or_else(|| content_id.filter(|_| content_type.contains("VIDEO")));
    let browse_id = v
        .pointer("/rendererContext/commandContext/onTap/innertubeCommand/browseEndpoint/browseId")
        .and_then(Value::as_str)
        .or_else(|| content_id.filter(|_| !content_type.contains("VIDEO")));
    let (id, kind) = if let Some(video_id) = video_id.filter(|id| valid_video_id(id)) {
        (video_id, ItemKind::Track)
    } else if let Some(browse_id) = browse_id.filter(|id| !id.is_empty()) {
        let kind = if browse_id.starts_with("MPRE") {
            ItemKind::Album
        } else if browse_id.starts_with("UC") {
            ItemKind::Artist
        } else {
            ItemKind::Playlist
        };
        (browse_id, kind)
    } else {
        return None;
    };
    let thumbnail = v
        .pointer("/contentImage/thumbnailViewModel/image/sources")
        .and_then(Value::as_array)
        .and_then(|sources| {
            sources
                .iter()
                .rfind(|source| source["width"].as_u64().unwrap_or(120) <= 360)
                .or_else(|| sources.first())
        })
        .and_then(|source| source["url"].as_str())
        .map(str::to_owned);
    Some(Item {
        id: id.into(),
        title,
        subtitle: String::new(),
        kind,
        thumbnail,
        duration: None,
        set_video_id: None,
    })
}
fn lockup_has_content(v: &Value) -> bool {
    v.get("contentId").and_then(Value::as_str).is_some()
        || v.get("contentType").and_then(Value::as_str).is_some()
}
fn is_create_affordance(v: &Value) -> bool {
    v.pointer("/navigationEndpoint/createPlaylistEndpoint")
        .is_some()
}
struct Collected {
    items: Vec<Item>,
    saw_unparsed: bool,
}
fn is_item_renderer(key: &str) -> bool {
    key.ends_with("ItemRenderer")
        || key == "playlistPanelVideoRenderer"
        || key == "compactStationRenderer"
}
fn is_catalog_leaf(key: &str) -> bool {
    is_item_renderer(key) || key == "lockupViewModel"
}
fn note_leaf(key: &str, child: &Value, collected: &mut Collected) {
    if key == "lockupViewModel" {
        if let Some(item) = parse_lockup(child) {
            collected.items.push(item);
        } else if lockup_has_content(child) {
            collected.saw_unparsed = true;
        }
        return;
    }
    if let Some(item) = parse_renderer(child) {
        collected.items.push(item);
    } else if !is_create_affordance(child) {
        collected.saw_unparsed = true;
    }
}
fn collect(v: &Value, collected: &mut Collected) {
    match v {
        Value::Object(object) => {
            let mut matched = false;
            for (key, child) in object {
                if is_catalog_leaf(key) {
                    matched = true;
                    note_leaf(key, child, collected);
                }
            }
            for (key, child) in object {
                if !matched || !is_catalog_leaf(key) {
                    collect(child, collected);
                }
            }
        }
        Value::Array(array) => {
            for child in array {
                collect(child, collected);
            }
        }
        _ => {}
    }
}
fn continuation_token(v: &Value) -> Option<String> {
    find_key(v, "continuationItemRenderer")
        .and_then(|item| {
            item.pointer("/continuationEndpoint/continuationCommand/token")
                .and_then(Value::as_str)
                .or_else(|| {
                    item.pointer("/continuationEndpoint/commandExecutorCommand/commands")
                        .and_then(Value::as_array)
                        .and_then(|commands| {
                            commands.iter().find_map(|command| {
                                let request = command
                                    .pointer("/continuationCommand/request")
                                    .and_then(Value::as_str)?;
                                if request != "CONTINUATION_REQUEST_TYPE_BROWSE" {
                                    return None;
                                }
                                command
                                    .pointer("/continuationCommand/token")
                                    .and_then(Value::as_str)
                            })
                        })
                })
        })
        .or_else(|| {
            find_key(v, "nextContinuationData").and_then(|data| data["continuation"].as_str())
        })
        .or_else(|| {
            find_key(v, "reloadContinuationData").and_then(|data| data["continuation"].as_str())
        })
        .map(str::to_owned)
}
fn parse_detailed(v: &Value) -> (Page, bool) {
    let mut collected = Collected {
        items: Vec::new(),
        saw_unparsed: false,
    };
    collect(v, &mut collected);
    let mut seen = HashSet::new();
    collected
        .items
        .retain(|item| seen.insert((item.id.clone(), item.set_video_id.clone())));
    collected.items.truncate(100);
    (
        Page {
            items: collected.items,
            continuation: continuation_token(v),
        },
        collected.saw_unparsed,
    )
}
pub fn parse_page(v: &Value) -> Page {
    parse_detailed(v).0
}
fn known_catalog_container(v: &Value) -> bool {
    find_key(v, "musicShelfRenderer").is_some()
        || find_key(v, "gridRenderer").is_some()
        || find_key(v, "musicGridRenderer").is_some()
        || find_key(v, "musicCarouselShelfRenderer").is_some()
        || find_key(v, "musicImmersiveCarouselShelfRenderer").is_some()
        || find_key(v, "musicPlaylistShelfRenderer").is_some()
        || find_key(v, "sectionListRenderer").is_some()
        || find_key(v, "messageRenderer").is_some()
        || find_key(v, "continuationContents").is_some()
        || find_key(v, "appendContinuationItemsAction").is_some()
}
fn is_structural_renderer(key: &str) -> bool {
    matches!(
        key,
        "singleColumnBrowseResultsRenderer"
            | "twoColumnBrowseResultsRenderer"
            | "tabRenderer"
            | "sectionListRenderer"
            | "itemSectionRenderer"
            | "gridRenderer"
            | "musicGridRenderer"
            | "musicShelfRenderer"
            | "musicPlaylistShelfRenderer"
            | "musicCarouselShelfRenderer"
            | "musicImmersiveCarouselShelfRenderer"
            | "musicCarouselShelfBasicHeaderRenderer"
            | "musicTastebuilderShelfRenderer"
            | "messageRenderer"
            | "messageSubtextRenderer"
            | "buttonRenderer"
            | "menuRenderer"
            | "menuNavigationItemRenderer"
            | "menuServiceItemRenderer"
            | "toggleMenuServiceItemRenderer"
            | "modalWithTitleAndButtonRenderer"
            | "notificationTextRenderer"
            | "notificationActionRenderer"
            | "musicThumbnailRenderer"
            | "thumbnailRenderer"
            | "musicPlayButtonRenderer"
            | "musicItemThumbnailOverlayRenderer"
            | "musicResponsiveListItemFlexColumnRenderer"
            | "musicResponsiveListItemFixedColumnRenderer"
            | "likeButtonRenderer"
            | "menuServiceItemDownloadRenderer"
            | "checkboxRenderer"
            | "musicInlineBadgeRenderer"
            | "chipCloudChipRenderer"
            | "continuationItemRenderer"
            | "musicDetailHeaderRenderer"
            | "musicEditablePlaylistDetailHeaderRenderer"
            | "musicResponsiveHeaderRenderer"
            | "musicShelfContinuation"
            | "gridContinuation"
            | "musicPlaylistShelfContinuation"
    ) || is_catalog_leaf(key)
}
fn foreign_content(v: &Value) -> bool {
    match v {
        Value::Object(object) => object.iter().any(|(key, child)| {
            ((key.ends_with("Renderer") || key.ends_with("ViewModel"))
                && !is_structural_renderer(key))
                || foreign_content(child)
        }),
        Value::Array(array) => array.iter().any(foreign_content),
        _ => false,
    }
}
fn parse_innertube_context(page: &str) -> Result<Value> {
    let mut rest = page;
    while let Some(offset) = rest.find("ytcfg.set(") {
        rest = &rest[offset + "ytcfg.set(".len()..];
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        if let Some(Ok(config)) = stream.next() {
            if let Some(context) = config.get("INNERTUBE_CONTEXT") {
                return Ok(context.clone());
            }
            if let Some(version) = config
                .get("INNERTUBE_CLIENT_VERSION")
                .and_then(Value::as_str)
            {
                return Ok(json!({
                    "client": {"clientName": "WEB_REMIX", "clientVersion": version, "hl": "ru"},
                    "user": {}
                }));
            }
        }
    }
    bail!("YouTube Music изменил настройки клиента. Требуется обновление RustMusic.")
}
fn parse_checked(v: &Value) -> Result<Page> {
    let (page, unrecognized) = parse_detailed(v);
    if page.items.is_empty() && (unrecognized || foreign_content(v)) {
        bail!("YouTube Music вернул раздел в неизвестном формате. Попробуйте обновить клиент.");
    }
    if page.items.is_empty() && find_key(v, "signInEndpoint").is_some() {
        bail!("YouTube Music не отдал этот раздел для текущей сессии. Подключите аккаунт заново.");
    }
    if page.items.is_empty() && !known_catalog_container(v) {
        bail!("YouTube Music вернул неподдерживаемый ответ. Попробуйте обновить клиент.");
    }
    Ok(page)
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn local_client(status: u16, body: &'static str) -> MusicClient {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer).await;
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let mut client = MusicClient::new(None).unwrap();
        client.test_base = Some(format!("http://{address}"));
        client
            .context
            .set(json!({"client":{"clientName":"WEB_REMIX","clientVersion":"fixture"}}))
            .unwrap();
        client
    }
    #[tokio::test]
    async fn expired_session_and_service_failures_are_explicit() {
        for code in [401, 403] {
            let client = local_client(code, "{}").await;
            let error = client
                .request("browse", json!({}))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("Сессия истекла"));
        }
        let client = local_client(503, "{}").await;
        assert!(
            client
                .request("browse", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("503")
        );
        let client = local_client(200, "not json").await;
        assert!(
            client
                .request("browse", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("Неизвестный формат")
        );
        let client = local_client(200, r#"{"error":{"message":"fixture rejected"}}"#).await;
        assert!(
            client
                .request("browse", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("отклонил")
        );
    }
    #[tokio::test]
    async fn network_failure_is_actionable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut client = MusicClient::new(None).unwrap();
        client.test_base = Some(format!("http://{address}"));
        client
            .context
            .set(json!({"client":{"clientName":"WEB_REMIX","clientVersion":"fixture"}}))
            .unwrap();
        assert!(
            client
                .request("browse", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("Ошибка сети")
        );
    }
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
    #[test]
    fn empty_shelf_stays_empty_and_unknown_items_are_errors() {
        let empty = parse_checked(&json!({
            "contents": {"sectionListRenderer": {"contents": [
                {"musicShelfRenderer": {"contents": []}}
            ]}}
        }))
        .unwrap();
        assert!(empty.items.is_empty());
        let error = parse_checked(&json!({
            "contents": {"sectionListRenderer": {"contents": [
                {"musicShelfRenderer": {"contents": [
                    {"musicWeirdItemRenderer": {"title": {"simpleText": "X"}}}
                ]}}
            ]}}
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("неизвестном формате"));
    }
    #[test]
    fn home_rows_and_stations_parse() {
        let multi = parse_checked(&json!({
            "musicMultiRowListItemRenderer": {
                "title": {"runs": [{"text": "Episode"}]},
                "subtitle": {"runs": [{"text": "Show"}]},
                "onTap": {"watchEndpoint": {"videoId": "abcdefghijk"}}
            }
        }))
        .unwrap();
        assert_eq!(multi.items.len(), 1);
        assert_eq!(multi.items[0].kind, ItemKind::Track);
        assert_eq!(multi.items[0].id, "abcdefghijk");
        let station = parse_checked(&json!({
            "musicCarouselShelfRenderer": {"contents": [{
                "compactStationRenderer": {
                    "title": {"simpleText": "Radio"},
                    "navigationEndpoint": {"watchEndpoint": {"videoId": "bbcdefghijk"}}
                }
            }]}
        }))
        .unwrap();
        assert_eq!(station.items[0].title, "Radio");
        assert_eq!(station.items[0].kind, ItemKind::Track);
        assert!(library_requires_session("VLLM"));
        assert!(library_requires_session("LM"));
        assert!(library_requires_session("FEmusic_liked_playlists"));
        assert!(!library_requires_session("FEmusic_home"));
    }
    #[test]
    fn library_and_liked_fixtures_keep_identity_and_continuation() {
        let library: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/library_playlists.json"
        ))
        .unwrap();
        let page = parse_checked(&library).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].kind, ItemKind::Playlist);
        assert_eq!(page.items[0].id, "VLPLFIXTURE0001");
        assert_eq!(page.items[0].title, "Fixture playlist");
        assert_eq!(page.items[0].subtitle, "5 songs");
        assert_eq!(page.continuation.as_deref(), Some("continuation-fixture"));
        let liked: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/liked_tracks.json"))
                .unwrap();
        let page = parse_checked(&liked).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].kind, ItemKind::Track);
        assert_eq!(page.items[0].id, "abcdefghijk");
        assert_eq!(page.items[0].set_video_id.as_deref(), Some("set-fixture-1"));
        assert_eq!(
            page.continuation.as_deref(),
            Some("liked-continuation-fixture")
        );
        let next = parse_checked(&json!({
            "onResponseReceivedActions": [{
                "appendContinuationItemsAction": {
                    "continuationItems": [{
                        "musicResponsiveListItemRenderer": {
                            "flexColumns": [{
                                "musicResponsiveListItemFlexColumnRenderer": {
                                    "text": {"runs": [{"text": "Fixture page 2"}]}
                                }
                            }],
                            "playlistItemData": {"videoId": "bbcdefghijk"}
                        }
                    }]
                }
            }]
        }))
        .unwrap();
        assert_eq!(next.items[0].id, "bbcdefghijk");
        let lockup = parse_checked(&json!({
            "lockupViewModel": {
                "contentType": "LOCKUP_CONTENT_TYPE_PLAYLIST",
                "contentId": "PLFIXTURE0001",
                "metadata": {"lockupMetadataViewModel": {"title": {"content": "Fixture lockup"}}}
            }
        }))
        .unwrap();
        assert_eq!(lockup.items[0].kind, ItemKind::Playlist);
        assert_eq!(lockup.items[0].id, "PLFIXTURE0001");
        assert_eq!(lockup.items[0].title, "Fixture lockup");
    }
    #[test]
    fn signed_out_shell_and_unknown_shelf_are_not_an_empty_library() {
        let signed_out: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/library_signin.json"))
                .unwrap();
        let error = parse_checked(&signed_out).unwrap_err().to_string();
        assert!(error.contains("сессии"));
        assert!(!error.contains("Looking"));
        assert!(!error.contains("Sign in"));
        let error = parse_checked(&json!({
            "contents": {"sectionListRenderer": {"contents": [
                {"musicFutureShelfRenderer": {"title": "hidden"}}
            ]}}
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("неизвестном формате"));
        assert!(!error.contains("hidden"));
        let empty = parse_checked(&json!({
            "contents": {"sectionListRenderer": {"contents": [{
                "gridRenderer": {"items": [{
                    "musicTwoRowItemRenderer": {
                        "title": {"runs": [{"text": "New playlist"}]},
                        "navigationEndpoint": {"createPlaylistEndpoint": {"hack": true}}
                    }
                }]}
            }]}}
        }))
        .unwrap();
        assert!(empty.items.is_empty());
    }
    #[tokio::test]
    async fn authenticated_browse_sends_the_session_and_parses_library() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
        let record = seen.clone();
        let library = include_str!("../../../tests/fixtures/library_playlists.json");
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = Vec::new();
                loop {
                    let mut tmp = [0u8; 2048];
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(200),
                        stream.read(&mut tmp),
                    )
                    .await
                    {
                        Ok(Ok(0)) | Err(_) => break,
                        Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
                        Ok(Err(_)) => break,
                    }
                    if buf.len() > 32_000 {
                        break;
                    }
                }
                let raw = String::from_utf8_lossy(&buf);
                record.lock().await.push(raw.to_string());
                let body = if raw.starts_with("GET ") {
                    r#"ytcfg.set({"INNERTUBE_CONTEXT":{"client":{"clientName":"WEB_REMIX","clientVersion":"fixture","visitorData":"visitor-fixture"}}});"#
                } else {
                    library
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        let mut client = MusicClient::new(Some(Session {
            cookie: "SID=fixture; __Secure-3PAPISID=fixture".into(),
            auth_user: "0".into(),
            netscape: String::new(),
        }))
        .unwrap();
        let origin = format!("http://{address}");
        client.page_base = Some(origin.clone());
        client.test_base = Some(format!("{origin}/youtubei/v1"));
        let page = client
            .browse("FEmusic_liked_playlists", None)
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "VLPLFIXTURE0001");
        assert_eq!(page.continuation.as_deref(), Some("continuation-fixture"));
        let captured = seen.lock().await;
        assert!(captured.len() >= 2);
        let home = &captured[0];
        assert!(home.starts_with("GET "));
        assert!(home.to_ascii_lowercase().contains("cookie:"));
        assert!(home.contains("__Secure-3PAPISID=fixture"));
        assert!(home.to_ascii_lowercase().contains("x-goog-authuser: 0"));
        let browse = &captured[1];
        assert!(browse.starts_with("POST "));
        assert!(browse.contains("visitor-fixture"));
        assert!(browse.contains("FEmusic_liked_playlists"));
        assert!(browse.contains("__Secure-3PAPISID=fixture"));
    }
}
