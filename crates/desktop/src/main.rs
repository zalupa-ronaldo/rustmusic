#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod caption;
mod playback;
mod player;
mod vault;
use iced::{
    Color, Element, Length, Subscription, Task, Theme,
    widget::{
        button, column, container, image, mouse_area, pick_list, row, rule, scrollable, slider,
        text, text_input,
    },
};
use rustmusic_core::{
    Account, ArtCache, CacheConfig, DownloadPhase, Item, ItemKind, MusicClient, Mutation, Page,
    Queue, Repeat, Resolver, Session, TrackCache, app_commit, app_version, clear_failed_open,
    default_cache_dir, default_covers_dir, playlist_reorder,
};
use serde_json::json;
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Route {
    Home,
    Library,
    Liked,
    Search(String),
    Browse(String),
    Radio(String),
}
impl Route {
    fn title(&self) -> &str {
        match self {
            Self::Home => "Для вас",
            Self::Library => "Библиотека",
            Self::Liked => "Любимые треки",
            Self::Search(_) => "Результаты поиска",
            Self::Browse(id) if id == "FEmusic_liked_albums" => "Альбомы в библиотеке",
            Self::Browse(id) if id == "FEmusic_library_corpus_track_artists" => {
                "Исполнители в библиотеке"
            }
            Self::Browse(_) => "Альбом / плейлист",
            Self::Radio(_) => "Радио",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragList {
    Queue,
    Playlist,
}
#[derive(Debug, Clone, Copy)]
struct Drag {
    list: DragList,
    from: usize,
    hover: usize,
}
#[derive(Debug, Clone)]
enum Message {
    Boot(Result<Option<Session>, String>),
    SessionConnected(Result<(Session, Account), String>),
    Query(String),
    Search,
    Go(Route),
    Page(u64, bool, Result<Page, String>),
    More,
    Cover(String, Option<(u32, u32, Vec<u8>)>),
    Open(usize),
    PlayQueue(usize),
    Enqueue(usize),
    Radio(usize),
    TrackMenu(usize),
    DragStart(DragList, usize),
    DragHover(DragList, usize),
    DragDrop,
    PlayNext(usize),
    Download(usize),
    DownloadCurrent,
    DownloadFinished(Result<(), String>),
    PlayResult(u64, Result<playback::StartKind, String>),
    Toggle,
    Next(bool),
    Previous,
    Shuffle,
    Repeat,
    Seek(f32),
    Volume(f32),
    Tick,
    Status(u64, Result<player::Status, String>),
    PlayerResult(Result<(), String>),
    Settings,
    OpenGoogle,
    Browser(String),
    Profile(String),
    AuthUser(String),
    ConnectBrowser,
    ImportFile,
    Manual(String),
    ImportManual,
    Disconnect,
    Target(String),
    Title(String),
    Mutate(Mutation),
    MutationDone(Result<String, String>),
    DeleteConfirm(bool),
    Ready,
    CacheDir(String),
    CacheLimit(String),
    SaveCache,
}
struct App {
    client: MusicClient,
    resolver: Resolver,
    player: player::Player,
    playback: playback::Playback,
    cache_dir: String,
    cache_limit: String,
    account: Option<Account>,
    route: Route,
    /// Catalog request in flight. A repeated click on this route is ignored.
    target_route: Option<Route>,
    /// Set when the user picks a section. The session probe must not replace it.
    catalog_chosen: bool,
    items: Vec<Item>,
    continuation: Option<String>,
    query: String,
    /// Catalog and library messages. Playback never writes this field.
    status: String,
    /// Player messages. Catalog loading never writes this field.
    player_status: String,
    loading: bool,
    request: u64,
    fetch: Option<iced::task::Handle>,
    covers: HashMap<String, image::Handle>,
    queue: Queue,
    play_id: u64,
    play_task: Option<iced::task::Handle>,
    playing: bool,
    position: f32,
    duration: f32,
    volume: f32,
    polling: bool,
    loading_audio: bool,
    open_failed: bool,
    played_from_cache: bool,
    store_armed: bool,
    settings: bool,
    browser: String,
    profile: String,
    auth_user: String,
    manual: String,
    target: String,
    title: String,
    selected: Option<usize>,
    delete_confirm: bool,
    connecting: bool,
    art: ArtCache,
    drag: Option<Drag>,
    playlist_sync: bool,
    #[cfg(target_os = "macos")]
    media: Option<souvlaki::MediaControls>,
    #[cfg(target_os = "macos")]
    media_events: std::sync::mpsc::Receiver<souvlaki::MediaControlEvent>,
}
fn result<T>(value: anyhow::Result<T>) -> Result<T, String> {
    value.map_err(|e| e.to_string())
}
impl App {
    fn boot() -> (Self, Task<Message>) {
        #[cfg(target_os = "macos")]
        let (media, media_events) = {
            let (sender, receiver) = std::sync::mpsc::channel();
            let media = souvlaki::MediaControls::new(souvlaki::PlatformConfig {
                dbus_name: "rustmusic",
                display_name: "RustMusic",
                hwnd: None,
            })
            .ok()
            .and_then(|mut controls| {
                controls
                    .attach(move |event| {
                        let _ = sender.send(event);
                    })
                    .ok()
                    .map(|_| controls)
            });
            (media, receiver)
        };
        let playback = playback::Playback::open();
        let cache_dir = playback.cache().directory().display().to_string();
        let cache_limit = (playback.cache().limit_bytes() / (1024 * 1024)).to_string();
        let app = Self {
            client: MusicClient::new(None).expect("HTTP client"),
            resolver: Resolver::new(None),
            player: player::Player::default(),
            playback,
            cache_dir,
            cache_limit,
            account: None,
            route: Route::Home,
            target_route: None,
            catalog_chosen: false,
            items: vec![],
            continuation: None,
            query: String::new(),
            status: "Подключите Google-аккаунт в настройках".into(),
            player_status: String::new(),
            loading: false,
            request: 0,
            fetch: None,
            covers: HashMap::new(),
            queue: Queue::default(),
            play_id: 0,
            play_task: None,
            playing: false,
            position: 0.,
            duration: 0.,
            volume: 80.,
            polling: false,
            loading_audio: false,
            open_failed: false,
            played_from_cache: false,
            store_armed: false,
            settings: false,
            browser: "chrome".into(),
            profile: String::new(),
            auth_user: "0".into(),
            manual: String::new(),
            target: String::new(),
            title: String::new(),
            selected: None,
            delete_confirm: false,
            connecting: false,
            art: open_art(),
            drag: None,
            playlist_sync: false,
            #[cfg(target_os = "macos")]
            media,
            #[cfg(target_os = "macos")]
            media_events,
        };
        let warm_player = app.player.clone();
        (
            app,
            Task::batch([
                Task::perform(
                    async {
                        result(
                            tokio::task::spawn_blocking(vault::load)
                                .await
                                .map_err(anyhow::Error::from)
                                .and_then(|r| r),
                        )
                    },
                    Message::Boot,
                ),
                Task::perform(
                    async move {
                        let _ = warm_player.warm().await;
                    },
                    |_| Message::Ready,
                ),
            ]),
        )
    }
    fn fetch(&mut self, route: Route, more: bool) -> Task<Message> {
        if keep_inflight_catalog(
            self.loading,
            self.target_route.as_ref() == Some(&route),
            more,
        ) {
            return Task::none();
        }
        if let Some(handle) = self.fetch.take() {
            handle.abort();
        }
        self.request += 1;
        let generation = self.request;
        self.loading = true;
        self.status = "Загрузка…".into();
        self.selected = None;
        self.delete_confirm = false;
        if !more {
            self.target_route = Some(route.clone());
        }
        let continuation = if more {
            self.continuation.clone()
        } else {
            None
        };
        let client = self.client.clone();
        let task = Task::perform(
            async move {
                result(match route {
                    Route::Home => client.browse("FEmusic_home", continuation.as_deref()).await,
                    Route::Library => {
                        client
                            .browse("FEmusic_liked_playlists", continuation.as_deref())
                            .await
                    }
                    Route::Liked => client.browse("VLLM", continuation.as_deref()).await,
                    Route::Search(query) => client.search(&query, continuation.as_deref()).await,
                    Route::Browse(id) => client.browse(&id, continuation.as_deref()).await,
                    Route::Radio(id) => client.radio(&id, continuation.as_deref()).await,
                })
            },
            move |r| Message::Page(generation, more, r),
        );
        let (task, handle) = task.abortable();
        self.fetch = Some(handle);
        task
    }
    fn play(&mut self) -> Task<Message> {
        if let Some(handle) = self.play_task.take() {
            handle.abort();
        }
        self.play_id += 1;
        let epoch = self.play_id;
        self.player.set_epoch(epoch);
        self.playback.arm(epoch);
        let Some(item) = self.queue.current().cloned() else {
            self.playing = false;
            self.loading_audio = false;
            self.playback.cancel_prefetch();
            self.media_update();
            return self.command(json!(["stop"]));
        };
        let next = self
            .queue
            .upcoming()
            .map(|track| track.id.clone())
            .filter(|id| id != &item.id);
        self.loading_audio = true;
        self.open_failed = false;
        self.played_from_cache = false;
        self.store_armed = false;
        self.playing = false;
        self.position = 0.;
        self.duration = 0.;
        self.player_status = if self.playback.downloads().lookup(&item.id).is_some() {
            format!("Скачано: {}", item.title)
        } else if self.playback.cache().lookup(&item.id).is_some() {
            format!("Кэш: {}", item.title)
        } else {
            format!("Открытие: {}", item.title)
        };
        let resolver = self.resolver.clone();
        let player = self.player.clone();
        let playback = self.playback.clone();
        let video_id = item.id;
        let task = Task::perform(
            async move {
                result(playback::start(&playback, &player, &resolver, &video_id, next, epoch).await)
            },
            move |response| Message::PlayResult(epoch, response),
        );
        let (task, handle) = task.abortable();
        self.play_task = Some(handle);
        task
    }
    fn drop_current_stream(&self) -> Task<Message> {
        let Some(id) = self.queue.current().map(|item| item.id.clone()) else {
            return Task::none();
        };
        self.drop_stream(id)
    }
    fn drop_stream(&self, id: String) -> Task<Message> {
        self.playback.drop_pending(&id);
        let resolver = self.resolver.clone();
        let cache = self.playback.cache();
        Task::perform(
            async move {
                clear_failed_open(&resolver, &cache, &id).await;
            },
            |_| Message::PlayerResult(Ok(())),
        )
    }
    fn store_audible_track(&mut self, position: f32, duration: f32) -> Task<Message> {
        let heard = position > 0.05 || duration > 0.;
        if !heard || self.store_armed {
            return Task::none();
        }
        self.store_armed = true;
        let Some(id) = self.queue.current().map(|item| item.id.clone()) else {
            return Task::none();
        };
        let playback = self.playback.clone();
        let epoch = self.play_id;
        Task::perform(
            async move {
                playback.commit_store(epoch, &id);
            },
            |_| Message::Ready,
        )
    }
    fn command(&self, value: serde_json::Value) -> Task<Message> {
        let player = self.player.clone();
        Task::perform(
            async move { result(player.command(value).await) },
            Message::PlayerResult,
        )
    }
    fn connect(&mut self, input: Option<String>) -> Task<Message> {
        if self.connecting {
            return Task::none();
        }
        self.connecting = true;
        self.status = "Проверка сессии…".into();
        let browser = self.browser.clone();
        let profile = self.profile.clone();
        let user = self.auth_user.clone();
        Task::perform(
            async move {
                result(
                    async {
                        let session = if let Some(input) = input {
                            Session::import(&input, &user)?
                        } else {
                            Session::from_browser(&browser, &profile, &user).await?
                        };
                        let account = MusicClient::new(Some(session.clone()))?.account().await?;
                        let saved = session.clone();
                        tokio::task::spawn_blocking(move || vault::save(&saved)).await??;
                        Ok((session, account))
                    }
                    .await,
                )
            },
            Message::SessionConnected,
        )
    }
    fn warm_catalog(&self) -> Task<Message> {
        let client = self.client.clone();
        Task::perform(
            async move {
                let _ = client.warm().await;
            },
            |_| Message::Ready,
        )
    }
    fn install_session(&mut self, session: Session) {
        self.auth_user = session.auth_user.clone();
        self.client = MusicClient::new(Some(session.clone())).expect("HTTP client");
        self.resolver = Resolver::new(Some(session));
    }
    fn media_update(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(media) = self.media.as_mut() {
            let item = self.queue.current();
            let _ = media.set_metadata(souvlaki::MediaMetadata {
                title: item.map(|i| i.title.as_str()),
                artist: item.map(|i| i.subtitle.as_str()),
                duration: (self.duration > 0.).then(|| Duration::from_secs_f32(self.duration)),
                ..Default::default()
            });
            let progress = Some(souvlaki::MediaPosition(Duration::from_secs_f32(
                self.position.max(0.),
            )));
            let playback = if item.is_none() {
                souvlaki::MediaPlayback::Stopped
            } else if self.playing {
                souvlaki::MediaPlayback::Playing { progress }
            } else {
                souvlaki::MediaPlayback::Paused { progress }
            };
            let _ = media.set_playback(playback);
        }
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Boot(Ok(Some(session))) => {
                // Install the restored cookies before the account probe. A
                // network or account-menu failure must not drop a session that
                // was already read from the keychain.
                let startup = startup_session(true, false);
                if startup.active {
                    self.install_session(session.clone());
                }
                let client = self.client.clone();
                Task::batch([
                    Task::perform(
                        async move { result(client.account().await.map(|a| (session, a))) },
                        Message::SessionConnected,
                    ),
                    self.warm_catalog(),
                ])
            }
            Message::Boot(Ok(None)) => self.fetch(Route::Home, false),
            Message::Boot(Err(e)) => {
                self.status = format!("Не удалось открыть хранилище: {e}");
                Task::none()
            }
            Message::SessionConnected(value) => {
                self.connecting = false;
                match value {
                    Ok((session, account)) => {
                        let startup = startup_session(true, true);
                        if startup.active && self.client.session().is_none() {
                            self.install_session(session);
                        }
                        if startup.verified {
                            self.account = Some(account);
                        }
                        self.manual.clear();
                        self.settings = false;
                        let warm = self.warm_catalog();
                        if session_may_open_home(self.catalog_chosen) {
                            Task::batch([warm, self.fetch(Route::Home, false)])
                        } else {
                            warm
                        }
                    }
                    Err(e) => {
                        if reconnect_prompt(self.client.session().is_some()) {
                            if !self.loading {
                                self.status = e;
                            }
                            self.settings = true;
                            Task::none()
                        } else if session_may_open_home(self.catalog_chosen) {
                            self.fetch(Route::Home, false)
                        } else {
                            if !self.loading {
                                self.status = e;
                            }
                            Task::none()
                        }
                    }
                }
            }
            Message::Query(s) => {
                self.query = s;
                Task::none()
            }
            Message::Search => {
                if self.query.trim().is_empty() {
                    Task::none()
                } else {
                    self.catalog_chosen = true;
                    self.fetch(Route::Search(self.query.clone()), false)
                }
            }
            Message::Go(route) => {
                self.settings = false;
                self.catalog_chosen = true;
                self.fetch(route, false)
            }
            Message::More => {
                self.catalog_chosen = true;
                self.fetch(self.route.clone(), true)
            }
            Message::Page(id, more, response) => {
                if id != self.request {
                    return Task::none();
                }
                self.loading = false;
                self.fetch = None;
                match response {
                    Err(e) => {
                        self.target_route = None;
                        self.status = e;
                        Task::none()
                    }
                    Ok(page) => {
                        if !more {
                            if let Some(route) = self.target_route.take() {
                                self.route = route;
                            }
                            self.items.clear();
                        }
                        self.items.extend(page.items);
                        self.continuation = page.continuation;
                        if self.items.len() > 500 {
                            let excess = self.items.len() - 500;
                            self.items.drain(..excess);
                        }
                        self.status = if self.items.is_empty() {
                            "В этом разделе пока нет элементов".into()
                        } else {
                            format!("{} элементов", self.items.len())
                        };
                        let art = self.art.clone();
                        let tasks = self
                            .items
                            .iter()
                            .take(24)
                            .filter_map(|i| i.thumbnail.clone())
                            .filter(|url| !self.covers.contains_key(url))
                            .map(|url| {
                                let art = art.clone();
                                Task::perform(
                                    async move {
                                        let bytes = match art.load(&url).await {
                                            Ok(bytes) => bytes,
                                            Err(_) => return (url, None),
                                        };
                                        let image = tokio::task::spawn_blocking(move || {
                                            decode_cover(&bytes)
                                        })
                                        .await
                                        .ok()
                                        .flatten();
                                        if image.is_none() {
                                            art.forget(&url);
                                        }
                                        (url, image)
                                    },
                                    |(url, data)| Message::Cover(url, data),
                                )
                            })
                            .collect::<Vec<_>>();
                        Task::batch(tasks)
                    }
                }
            }
            Message::Cover(url, Some((width, height, bytes))) => {
                if self.covers.len() >= 48 {
                    self.covers.clear();
                }
                self.covers
                    .insert(url, image::Handle::from_rgba(width, height, bytes));
                Task::none()
            }
            Message::Cover(_, None) => Task::none(),
            Message::Open(index) => {
                let Some(item) = self.items.get(index).cloned() else {
                    return Task::none();
                };
                if item.kind != ItemKind::Track {
                    self.target = if item.kind == ItemKind::Playlist {
                        item.id.trim_start_matches("VL").into()
                    } else {
                        String::new()
                    };
                    self.catalog_chosen = true;
                    return self.fetch(Route::Browse(item.id), false);
                }
                let tracks = self
                    .items
                    .iter()
                    .filter(|i| i.kind == ItemKind::Track)
                    .cloned()
                    .collect::<Vec<_>>();
                let selected = tracks.iter().position(|t| t.id == item.id).unwrap_or(0);
                self.queue.replace(tracks, selected);
                self.play()
            }
            Message::PlayQueue(i) => {
                if i < self.queue.items.len() {
                    self.queue.index = Some(i);
                    self.play()
                } else {
                    Task::none()
                }
            }
            Message::Enqueue(i) => {
                if let Some(item) = self.items.get(i) {
                    self.queue.append(item.clone());
                    self.status = "Добавлено в очередь".into();
                }
                Task::none()
            }
            Message::Radio(i) => {
                if let Some(item) = self.items.get(i) {
                    self.catalog_chosen = true;
                    self.fetch(Route::Radio(item.id.clone()), false)
                } else {
                    Task::none()
                }
            }
            Message::TrackMenu(i) => {
                self.selected = if self.selected == Some(i) {
                    None
                } else {
                    Some(i)
                };
                Task::none()
            }
            Message::DragStart(list, index) => {
                self.drag = Some(Drag {
                    list,
                    from: index,
                    hover: index,
                });
                self.status = "Перетащите трек и отпустите, чтобы изменить порядок".into();
                Task::none()
            }
            Message::DragHover(list, index) => {
                if let Some(drag) = &mut self.drag
                    && drag.list == list
                {
                    drag.hover = index;
                }
                Task::none()
            }
            Message::DragDrop => self.finish_drag(),
            Message::PlayNext(index) => {
                if let Some(item) = self.items.get(index).cloned()
                    && item.kind == ItemKind::Track
                    && self.queue.insert_next(item)
                {
                    self.status = "Будет следующим".into();
                }
                Task::none()
            }
            Message::Download(index) => {
                let Some(item) = self.items.get(index).cloned() else {
                    return Task::none();
                };
                self.start_download(item.id)
            }
            Message::DownloadCurrent => {
                let Some(id) = self.queue.current().map(|item| item.id.clone()) else {
                    return Task::none();
                };
                self.start_download(id)
            }
            Message::DownloadFinished(Ok(())) => {
                self.status = "Трек скачан и будет играть без сети".into();
                Task::none()
            }
            Message::DownloadFinished(Err(error)) => {
                self.status = error;
                Task::none()
            }
            Message::PlayResult(id, response) => {
                if id != self.play_id {
                    return Task::none();
                }
                self.loading_audio = false;
                self.play_task = None;
                match response {
                    Ok(playback::StartKind::Stale) => Task::none(),
                    Ok(kind) => {
                        self.playing = true;
                        self.played_from_cache = kind == playback::StartKind::Cached;
                        self.player_status = match kind {
                            playback::StartKind::Offline => "Скачанный трек".into(),
                            playback::StartKind::Cached => "Воспроизведение из кэша".into(),
                            playback::StartKind::Network | playback::StartKind::Stale => {
                                "Воспроизведение".into()
                            }
                        };
                        self.media_update();
                        self.command(json!(["set_property", "volume", self.volume]))
                    }
                    Err(e) => {
                        self.playing = false;
                        self.open_failed = true;
                        self.player_status = e;
                        self.drop_current_stream()
                    }
                }
            }
            Message::Toggle => {
                if self.queue.current().is_none() {
                    return Task::none();
                }
                if self.open_failed {
                    return self.play();
                }
                self.playing = !self.playing;
                self.media_update();
                self.command(json!(["set_property", "pause", !self.playing]))
            }
            Message::Next(ended) => {
                self.queue.advance(ended);
                self.play()
            }
            Message::Previous => {
                if self.position > 3. {
                    self.command(json!(["seek", 0, "absolute"]))
                } else {
                    self.queue.previous();
                    self.play()
                }
            }
            Message::Shuffle => {
                self.queue.shuffle();
                Task::none()
            }
            Message::Repeat => {
                self.queue.repeat = match self.queue.repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
                Task::none()
            }
            Message::Seek(value) => {
                self.position = value;
                self.command(json!(["seek", value, "absolute"]))
            }
            Message::Volume(value) => {
                self.volume = value;
                self.command(json!(["set_property", "volume", value]))
            }
            Message::Tick => {
                let mut tasks = Vec::new();
                #[cfg(target_os = "macos")]
                {
                    let events = self.media_events.try_iter().take(8).collect::<Vec<_>>();
                    for event in events {
                        use souvlaki::MediaControlEvent as E;
                        let msg = match event {
                            E::Play if !self.playing => Some(Message::Toggle),
                            E::Pause if self.playing => Some(Message::Toggle),
                            E::Toggle => Some(Message::Toggle),
                            E::Next => Some(Message::Next(false)),
                            E::Previous => Some(Message::Previous),
                            E::SetPosition(p) => Some(Message::Seek(p.0.as_secs_f32())),
                            _ => None,
                        };
                        if let Some(msg) = msg {
                            tasks.push(self.update(msg));
                        }
                    }
                }
                if !self.polling
                    && !self.loading_audio
                    && !self.open_failed
                    && self.queue.current().is_some()
                {
                    self.polling = true;
                    let player = self.player.clone();
                    let epoch = self.play_id;
                    tasks.push(Task::perform(
                        async move { result(player.status().await) },
                        move |response| Message::Status(epoch, response),
                    ));
                }
                Task::batch(tasks)
            }
            Message::Status(epoch, response) => {
                self.polling = false;
                if epoch != self.play_id {
                    return Task::none();
                }
                match response {
                    Ok(status) => {
                        if status.failed {
                            if self.open_failed {
                                return Task::none();
                            }
                            self.open_failed = true;
                            self.playing = false;
                            self.player_status = player_open_error(self.played_from_cache).into();
                            self.media_update();
                            let id = self.queue.current().map(|item| item.id.clone());
                            return id.map(|id| self.drop_stream(id)).unwrap_or_else(Task::none);
                        }
                        self.position = status.position;
                        self.duration = status.duration;
                        self.playing = !status.paused;
                        self.media_update();
                        let store = self.store_audible_track(status.position, status.duration);
                        if status.ended
                            && !self.loading_audio
                            && status.duration > 1.
                            && status.position + 0.75 >= status.duration
                        {
                            return Task::batch([store, self.update(Message::Next(true))]);
                        }
                        return store;
                    }
                    Err(e) => {
                        self.player_status = e;
                        self.playing = false;
                    }
                }
                Task::none()
            }
            Message::PlayerResult(response) => {
                if let Err(e) = response {
                    self.player_status = e;
                }
                Task::none()
            }
            Message::Settings => {
                self.settings = !self.settings;
                Task::none()
            }
            Message::OpenGoogle => {
                if webbrowser::open("https://music.youtube.com/").is_err() {
                    self.status = "Откройте https://music.youtube.com в браузере".into();
                }
                Task::none()
            }
            Message::Browser(v) => {
                self.browser = v;
                Task::none()
            }
            Message::Profile(v) => {
                self.profile = v;
                Task::none()
            }
            Message::AuthUser(v) => {
                self.auth_user = v;
                Task::none()
            }
            Message::Manual(v) => {
                self.manual = v;
                Task::none()
            }
            Message::ConnectBrowser => self.connect(None),
            Message::ImportManual => self.connect(Some(self.manual.clone())),
            Message::ImportFile => Task::perform(
                async {
                    let file = rfd::AsyncFileDialog::new()
                        .add_filter("Сессия", &["json", "txt"])
                        .pick_file()
                        .await;
                    match file {
                        Some(file) => String::from_utf8(file.read().await).unwrap_or_default(),
                        None => String::new(),
                    }
                },
                Message::Manual,
            ),
            Message::Disconnect => match vault::clear() {
                Ok(()) => {
                    self.play_id += 1;
                    self.player.set_epoch(self.play_id);
                    self.playback.arm(self.play_id);
                    self.playback.cancel_prefetch();
                    if let Some(handle) = self.play_task.take() {
                        handle.abort();
                    }
                    self.account = None;
                    self.client = MusicClient::new(None).unwrap();
                    self.resolver = Resolver::new(None);
                    self.queue = Queue::default();
                    self.playing = false;
                    self.open_failed = false;
                    self.player_status.clear();
                    self.items.clear();
                    self.covers.clear();
                    self.status = "Сохранённая сессия удалена".into();
                    self.media_update();
                    self.command(json!(["stop"]))
                }
                Err(e) => {
                    self.status = e.to_string();
                    Task::none()
                }
            },
            Message::Target(v) => {
                self.target = v;
                Task::none()
            }
            Message::Title(v) => {
                self.title = v;
                Task::none()
            }
            Message::Mutate(mutation) => {
                let client = self.client.clone();
                self.status = "Сохранение…".into();
                Task::perform(
                    async move {
                        result(
                            client
                                .mutate(mutation)
                                .await
                                .map(|v| v["playlist_id"].as_str().unwrap_or("").to_owned()),
                        )
                    },
                    Message::MutationDone,
                )
            }
            Message::MutationDone(response) => {
                self.delete_confirm = false;
                let refresh_playlist = self.playlist_sync;
                self.playlist_sync = false;
                match response {
                    Ok(id) => {
                        self.status = "Изменение сохранено".into();
                        if !id.is_empty() {
                            self.target = id.clone();
                            self.catalog_chosen = true;
                            self.fetch(Route::Browse(id), false)
                        } else {
                            self.catalog_chosen = true;
                            self.fetch(self.route.clone(), false)
                        }
                    }
                    Err(e) => {
                        self.status = e;
                        if refresh_playlist {
                            self.fetch(self.route.clone(), false)
                        } else {
                            Task::none()
                        }
                    }
                }
            }
            Message::DeleteConfirm(v) => {
                self.delete_confirm = v;
                Task::none()
            }
            Message::Ready => Task::none(),
            Message::CacheDir(value) => {
                self.cache_dir = value;
                Task::none()
            }
            Message::CacheLimit(value) => {
                self.cache_limit = value;
                Task::none()
            }
            Message::SaveCache => self.save_cache(),
        }
    }
    fn save_cache(&mut self) -> Task<Message> {
        let Ok(limit_mb) = self.cache_limit.trim().parse::<u64>() else {
            self.status = "Лимит кэша должен быть целым числом мегабайт".into();
            return Task::none();
        };
        let trimmed = self.cache_dir.trim();
        let directory = if trimmed.is_empty() {
            default_cache_dir()
        } else {
            std::path::PathBuf::from(trimmed)
        };
        let config = CacheConfig {
            directory,
            limit_bytes: limit_mb.saturating_mul(1024 * 1024),
        };
        match TrackCache::open(config.clone()) {
            Ok(cache) => {
                self.cache_dir = config.directory.display().to_string();
                self.cache_limit = limit_mb.to_string();
                self.playback.set_cache(cache);
                self.status = match config.save() {
                    Ok(()) => "Настройки кэша сохранены".into(),
                    Err(error) => error.to_string(),
                };
            }
            Err(error) => self.status = error.to_string(),
        }
        Task::none()
    }
    fn finish_drag(&mut self) -> Task<Message> {
        let Some(drag) = self.drag.take() else {
            return Task::none();
        };
        if drag.from == drag.hover {
            return Task::none();
        }
        match drag.list {
            DragList::Queue => {
                self.queue.reorder(drag.from, drag.hover);
                self.status = "Порядок очереди изменён".into();
                Task::none()
            }
            DragList::Playlist => {
                let Some(playlist_id) = owned_playlist(&self.route) else {
                    return Task::none();
                };
                let Some(action) = playlist_reorder(&self.items, drag.from, drag.hover) else {
                    self.status = "YouTube Music не дал позицию этого трека".into();
                    return Task::none();
                };
                let item = self.items.remove(drag.from);
                self.items.insert(drag.hover, item);
                self.playlist_sync = true;
                self.status = "Сохраняю порядок в YouTube Music…".into();
                self.update(Message::Mutate(Mutation::Move {
                    playlist_id,
                    set_video_id: action.set_video_id,
                    before_set_video_id: action.before_set_video_id.unwrap_or_default(),
                }))
            }
        }
    }
    fn start_download(&mut self, id: String) -> Task<Message> {
        match self.playback.downloads().phase(&id) {
            DownloadPhase::Ready => {
                self.status = "Трек уже скачан".into();
                return Task::none();
            }
            DownloadPhase::Active(progress) => {
                self.status = format!("Скачивание {progress}%");
                return Task::none();
            }
            DownloadPhase::Idle | DownloadPhase::Failed => {}
        }
        let downloads = self.playback.downloads();
        if !downloads.reserve(&id) {
            self.status = "Скачивание уже идёт".into();
            return Task::none();
        }
        self.status = "Скачивание…".into();
        let cached = self.playback.cache().lookup(&id);
        let resolver = self.resolver.clone();
        Task::perform(
            async move {
                result(
                    async {
                        if let Some(path) = cached {
                            let store = downloads.clone();
                            let video_id = id.clone();
                            match tokio::task::spawn_blocking(move || {
                                store.pin_copy(&video_id, &path)
                            })
                            .await
                            {
                                Ok(Ok(())) => return Ok(()),
                                Ok(Err(error)) => {
                                    downloads.mark_failed(&id, error.to_string());
                                    return Err(error);
                                }
                                Err(error) => {
                                    let message = error.to_string();
                                    downloads.mark_failed(&id, message.clone());
                                    return Err(anyhow::anyhow!(message));
                                }
                            }
                        }
                        match resolver.resolve(&id, false).await {
                            Ok(stream) => downloads.save_stream(&id, stream).await,
                            Err(error) => {
                                downloads.mark_failed(&id, error.to_string());
                                Err(error)
                            }
                        }
                    }
                    .await,
                )
            },
            Message::DownloadFinished,
        )
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
            iced::event::listen_with(|event, _status, _window| {
                if let iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )) = event
                {
                    Some(Message::DragDrop)
                } else {
                    None
                }
            }),
        ])
    }
    fn view(&self) -> Element<'_, Message> {
        let accent = Color::from_rgb8(237, 83, 85);
        let sidebar = column![
            text("RustMusic").size(26).color(accent),
            text("YOUTUBE MUSIC CLIENT").size(10),
            rule::horizontal(1),
            soft_button("Для вас")
                .on_press(Message::Go(Route::Home))
                .width(Length::Fill),
            soft_button("Библиотека")
                .on_press(Message::Go(Route::Library))
                .width(Length::Fill),
            soft_button("Любимые треки")
                .on_press(Message::Go(Route::Liked))
                .width(Length::Fill),
            soft_button("Мои альбомы")
                .on_press(Message::Go(Route::Browse("FEmusic_liked_albums".into())))
                .width(Length::Fill),
            soft_button("Мои исполнители")
                .on_press(Message::Go(Route::Browse(
                    "FEmusic_library_corpus_track_artists".into()
                )))
                .width(Length::Fill),
            text("ОЧЕРЕДЬ").size(12),
            scrollable(column(self.queue.items.iter().enumerate().map(
                |(i, item)| {
                    let caption = caption::split_subtitle(&item.subtitle);
                    let playing = if self.queue.index == Some(i) {
                        "▶ "
                    } else {
                        ""
                    };
                    let marker = if self.drag.is_some_and(|drag| {
                        drag.list == DragList::Queue && drag.hover == i && drag.from != i
                    }) {
                        "▸ "
                    } else {
                        ""
                    };
                    let title =
                        soft_button(text(format!("{marker}{playing}{}", item.title)).size(12))
                            .on_press(Message::PlayQueue(i))
                            .width(Length::Fill);
                    let mut entry = column![
                        row![
                            soft_button(text("⋮⋮").size(12))
                                .on_press(Message::DragStart(DragList::Queue, i))
                                .padding(4),
                            title
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center)
                    ]
                    .spacing(0);
                    if let Some(album) = caption.album {
                        entry = entry.push(
                            soft_button(
                                text(album).size(11).color(Color::from_rgb8(150, 153, 165)),
                            )
                            .style(button::text)
                            .on_press(Message::PlayQueue(i))
                            .width(Length::Fill),
                        );
                    }
                    mouse_area(entry)
                        .on_enter(Message::DragHover(DragList::Queue, i))
                        .into()
                }
            )))
            .height(Length::Fill),
            text(
                self.account
                    .as_ref()
                    .map(|a| a.name.as_str())
                    .unwrap_or("Аккаунт не подключён")
            )
            .size(13),
            soft_button("Аккаунт / настройки").on_press(Message::Settings),
            text(format!("{} · {}", app_version(), app_commit())).size(11)
        ]
        .spacing(14)
        .padding(20)
        .width(230);
        let search = row![
            text_input("Найти музыку, альбом или исполнителя", &self.query)
                .on_input(Message::Query)
                .on_submit(Message::Search)
                .padding(12),
            soft_button("Поиск").on_press(Message::Search).padding(12)
        ]
        .spacing(10);
        let content: Element<_> = if self.settings {
            self.settings_view()
        } else {
            let mut list = column![text(self.route.title()).size(28)].spacing(10);
            if self.account.is_some() {
                list = list.push(
                    row![
                        text_input("Название плейлиста", &self.title)
                            .on_input(Message::Title)
                            .padding(8),
                        soft_button("Создать").on_press(Message::Mutate(Mutation::Create {
                            title: self.title.clone()
                        }))
                    ]
                    .spacing(8),
                );
                if let Route::Browse(id) = &self.route
                    && (id.starts_with("VLPL") || id.starts_with("PL"))
                {
                    let playlist_id = id.trim_start_matches("VL").to_owned();
                    list = list.push(
                        row![
                            soft_button("Переименовать").on_press(Message::Mutate(
                                Mutation::Rename {
                                    playlist_id: playlist_id.clone(),
                                    title: self.title.clone()
                                }
                            )),
                            soft_button("Удалить плейлист").on_press(Message::DeleteConfirm(true))
                        ]
                        .spacing(8),
                    );
                    if self.delete_confirm {
                        list = list.push(
                            row![
                                text("Удалить этот плейлист из аккаунта?"),
                                soft_button("Да, удалить").on_press(Message::Mutate(
                                    Mutation::Delete {
                                        playlist_id: playlist_id.clone()
                                    }
                                )),
                                soft_button("Отмена").on_press(Message::DeleteConfirm(false))
                            ]
                            .spacing(8),
                        );
                    }
                }
            }
            for (index, item) in self.items.iter().enumerate() {
                let cover: Element<_> = item
                    .thumbnail
                    .as_ref()
                    .and_then(|url| self.covers.get(url))
                    .map(|handle| image(handle.clone()).width(52).height(52).into())
                    .unwrap_or_else(|| {
                        container(
                            text(if item.kind == ItemKind::Track {
                                "♫"
                            } else {
                                "▣"
                            })
                            .size(24)
                            .color(accent),
                        )
                        .center_x(52)
                        .center_y(52)
                        .into()
                    });
                let caption = caption::split_subtitle(&item.subtitle);
                let muted = Color::from_rgb8(150, 153, 165);
                let title_button = soft_button(
                    container(text(&item.title).size(16))
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Left),
                )
                .style(button::text)
                .on_press(Message::Open(index))
                .width(Length::FillPortion(3));
                let mut heading = row![title_button]
                    .spacing(8)
                    .align_y(iced::Alignment::Center);
                if let Some(album) = caption.album {
                    heading = heading.push(
                        soft_button(text(album).size(12).color(muted))
                            .style(button::text)
                            .on_press(Message::Open(index))
                            .width(Length::FillPortion(2)),
                    );
                }
                let mut info = column![heading].spacing(2).width(Length::Fill);
                if !caption.artist.is_empty() {
                    info = info.push(
                        soft_button(
                            container(text(caption.artist).size(12).color(muted))
                                .width(Length::Fill)
                                .align_x(iced::alignment::Horizontal::Left),
                        )
                        .style(button::text)
                        .on_press(Message::Open(index))
                        .width(Length::Fill),
                    );
                }
                let mut line = row![
                    soft_button(cover)
                        .style(button::text)
                        .padding(0)
                        .on_press(Message::Open(index)),
                    info,
                    text(item.duration.as_deref().unwrap_or("")).size(12)
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center);
                if item.kind == ItemKind::Track {
                    let saved = download_caption(&self.playback.downloads(), &item.id);
                    line = line
                        .push(soft_button(text(saved).size(12)).on_press(Message::Download(index)));
                    line = line.push(soft_button("•••").on_press(Message::TrackMenu(index)));
                }
                let playlist_drag = item.kind == ItemKind::Track
                    && item.set_video_id.is_some()
                    && owned_playlist(&self.route).is_some();
                if playlist_drag {
                    let marker = if self.drag.is_some_and(|drag| {
                        drag.list == DragList::Playlist && drag.hover == index && drag.from != index
                    }) {
                        "▸"
                    } else {
                        "⋮⋮"
                    };
                    line = row![
                        soft_button(text(marker).size(12))
                            .on_press(Message::DragStart(DragList::Playlist, index))
                            .padding(4),
                        line
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center);
                }
                let row_view: Element<_> = container(line).padding(6).into();
                let row_view = if playlist_drag {
                    mouse_area(row_view)
                        .on_enter(Message::DragHover(DragList::Playlist, index))
                        .into()
                } else {
                    row_view
                };
                list = list.push(row_view);
                if self.selected == Some(index) {
                    list = list.push(
                        row![
                            soft_button("В очередь").on_press(Message::Enqueue(index)),
                            soft_button("Следующим").on_press(Message::PlayNext(index)),
                            soft_button("Радио").on_press(Message::Radio(index)),
                            soft_button("Лайк").on_press(Message::Mutate(Mutation::Like {
                                video_id: item.id.clone(),
                                liked: true
                            })),
                            soft_button("Дизлайк").on_press(Message::Mutate(Mutation::Dislike {
                                video_id: item.id.clone()
                            })),
                            soft_button("Снять оценку").on_press(Message::Mutate(Mutation::Like {
                                video_id: item.id.clone(),
                                liked: false
                            }))
                        ]
                        .spacing(8),
                    );
                    list = list.push(
                        row![
                            text_input("ID целевого плейлиста", &self.target)
                                .on_input(Message::Target),
                            soft_button("Добавить").on_press(Message::Mutate(Mutation::Add {
                                playlist_id: self.target.clone(),
                                video_id: item.id.clone()
                            }))
                        ]
                        .spacing(8),
                    );
                    if let Some(set_id) = &item.set_video_id
                        && let Route::Browse(id) = &self.route
                    {
                        let mut actions = row![soft_button("Убрать из плейлиста").on_press(
                            Message::Mutate(Mutation::Remove {
                                playlist_id: id.trim_start_matches("VL").into(),
                                video_id: item.id.clone(),
                                set_video_id: set_id.clone()
                            })
                        )]
                        .spacing(8);
                        if index > 0
                            && let Some(previous) = self.items[index - 1].set_video_id.clone()
                        {
                            actions = actions.push(soft_button("Выше").on_press(Message::Mutate(
                                Mutation::Move {
                                    playlist_id: id.trim_start_matches("VL").into(),
                                    set_video_id: set_id.clone(),
                                    before_set_video_id: previous,
                                },
                            )));
                        }
                        list = list.push(actions);
                    }
                }
            }
            if self.continuation.is_some() {
                let mut more = soft_button("Загрузить ещё");
                if !self.loading {
                    more = more.on_press(Message::More);
                }
                list = list.push(more);
            }
            scrollable(list).height(Length::Fill).into()
        };
        let main = column![
            search,
            content,
            text(&self.status)
                .size(12)
                .color(Color::from_rgb8(172, 175, 187))
        ]
        .spacing(18)
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill);
        let current = self.queue.current();
        let mut like = soft_button("♥");
        let mut dislike = soft_button("👎");
        let mut save = soft_button(text(
            current
                .map(|item| download_caption(&self.playback.downloads(), &item.id))
                .unwrap_or_else(|| "↓".into()),
        ));
        if let Some(item) = current {
            like = like.on_press(Message::Mutate(Mutation::Like {
                video_id: item.id.clone(),
                liked: true,
            }));
            dislike = dislike.on_press(Message::Mutate(Mutation::Dislike {
                video_id: item.id.clone(),
            }));
            save = save.on_press(Message::DownloadCurrent);
        }
        let controls = row![
            soft_button("◀◀").on_press(Message::Previous),
            soft_button(if self.playing {
                "Пауза"
            } else {
                "Играть"
            })
            .on_press(Message::Toggle),
            soft_button("▶▶").on_press(Message::Next(false)),
            soft_button("Перемешать").on_press(Message::Shuffle),
            soft_button(match self.queue.repeat {
                Repeat::Off => "Повтор: выкл",
                Repeat::All => "Повтор: все",
                Repeat::One => "Повтор: один",
            })
            .on_press(Message::Repeat)
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let transport = row![
            column![
                text(
                    current
                        .map(|i| i.title.as_str())
                        .unwrap_or("Выберите музыку")
                )
                .size(16),
                text(
                    current
                        .map(|i| i.subtitle.as_str())
                        .unwrap_or("Нативный звук • Rust")
                )
                .size(12),
                row![like, dislike, save].spacing(6)
            ]
            .width(Length::Fill),
            controls,
            text("Громкость").size(12),
            slider(0.0..=100.0, self.volume, Message::Volume).width(100)
        ]
        .spacing(16)
        .align_y(iced::Alignment::Center);
        let mut footer = column![transport].spacing(12).padding(18);
        if !self.player_status.is_empty() {
            let color = if self.open_failed {
                accent
            } else {
                Color::from_rgb8(172, 175, 187)
            };
            footer = footer.push(text(&self.player_status).size(12).color(color));
        }
        let footer = footer.push(
            row![
                text(time(self.position)).size(12),
                slider(
                    0.0..=self.duration.max(1.),
                    self.position.min(self.duration.max(1.)),
                    Message::Seek
                ),
                text(time(self.duration)).size(12)
            ]
            .spacing(12),
        );
        column![
            row![sidebar, main].height(Length::Fill),
            rule::horizontal(1),
            footer
        ]
        .into()
    }
    fn settings_view(&self) -> Element<'_, Message> {
        let mut connect = soft_button(if self.connecting {
            "Подключение…"
        } else {
            "Подключить выбранный профиль"
        })
        .padding(10);
        if !self.connecting {
            connect = connect.on_press(Message::ConnectBrowser);
        }
        let mut manual = soft_button("Проверить и сохранить сессию").padding(10);
        if !self.connecting {
            manual = manual.on_press(Message::ImportManual);
        }
        scrollable(
            column![
                text("Google / YouTube Music").size(28),
                text("1. Войдите в Google на сайте YouTube Music в обычном браузере."),
                soft_button("Открыть YouTube Music").on_press(Message::OpenGoogle),
                text("2. Выберите браузер и явно разрешите импорт его сессии."),
                row![
                    pick_list(
                        vec![
                            "chrome".to_owned(),
                            "firefox".into(),
                            "safari".into(),
                            "edge".into(),
                            "brave".into()
                        ],
                        Some(self.browser.clone()),
                        Message::Browser
                    ),
                    text_input("Профиль: Default / Profile 1 / путь Firefox", &self.profile)
                        .on_input(Message::Profile)
                ],
                text_input("Номер Google-аккаунта: 0, 1, 2…", &self.auth_user)
                    .on_input(Message::AuthUser),
                connect,
                rule::horizontal(1),
                text("Ручной импорт (JSON headers, Cookie или Netscape cookies.txt)"),
                soft_button("Выбрать файл").on_press(Message::ImportFile),
                text_input(
                    "Вставьте сессию; она сохранится на этом компьютере",
                    &self.manual
                )
                .on_input(Message::Manual)
                .secure(true),
                manual,
                text("При отказе Keychain или недоступном профиле используйте ручной импорт.")
                    .size(12),
                rule::horizontal(1),
                text("Кэш прослушанных треков").size(22),
                text(
                    "Треки, которые уже играли, сохраняются на диск и в следующий раз запускаются без сети. Лимит 0 отключает запись. При переполнении удаляются давно не слушавшиеся файлы.",
                )
                .size(12),
                text_input("Каталог кэша", &self.cache_dir).on_input(Message::CacheDir),
                text_input("Лимит, МБ", &self.cache_limit).on_input(Message::CacheLimit),
                soft_button("Сохранить настройки кэша").on_press(Message::SaveCache),
                text(format!(
                    "Сейчас занято {} МБ из {} МБ. Переменные RUSTMUSIC_CACHE_DIR и RUSTMUSIC_CACHE_LIMIT_MB перекрывают сохранённые настройки при следующем запуске.",
                    self.playback.cache().usage_bytes() / (1024 * 1024),
                    self.playback.cache().limit_bytes() / (1024 * 1024)
                ))
                .size(12),
                text("Обложки и скачанные треки").size(22),
                text(
                    "Обложки сохраняются на диск и не запрашиваются заново. Кнопка ↓ рядом с треком скачивает его отдельно от кэша прослушивания: эти файлы не удаляются, когда кэш вытесняет старые треки. Повторное воспроизведение идёт из скачанного файла без сети.",
                )
                .size(12),
                text(format!(
                    "Скачано {} ({} МБ). Каталог: {}",
                    self.playback.downloads().count(),
                    self.playback.downloads().bytes() / (1024 * 1024),
                    self.playback.downloads().directory().display()
                ))
                .size(12),
                text(format!("Сборка {} · {}", app_version(), app_commit())).size(12),
                soft_button("Выйти и удалить сохранённую сессию").on_press(Message::Disconnect)
            ]
            .spacing(16),
        )
        .height(Length::Fill)
        .into()
    }
}
struct StartupSession {
    active: bool,
    verified: bool,
}

/// A session read at startup stays active even when the account probe fails.
/// `verified` is only the display name from YouTube Music.
fn startup_session(loaded: bool, account_verified: bool) -> StartupSession {
    StartupSession {
        active: loaded,
        verified: loaded && account_verified,
    }
}

/// The sign-in screen is the reconnect prompt only when no session is loaded.
fn reconnect_prompt(session_active: bool) -> bool {
    !session_active
}

/// A second click on the tab that is already loading must not cancel that load.
fn keep_inflight_catalog(loading: bool, same_route: bool, more: bool) -> bool {
    loading && same_route && !more
}

/// Home is opened after sign-in only when the user has not chosen a section.
fn session_may_open_home(catalog_chosen: bool) -> bool {
    !catalog_chosen
}

fn player_open_error(from_cache: bool) -> &'static str {
    if from_cache {
        "Не удалось открыть файл в кэше. Повторите трек — он будет запрошен заново."
    } else {
        "Не удалось открыть адрес потока. Повторите выбор трека."
    }
}

fn decode_cover(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    let mut reader = ::image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = ::image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(128, 128).to_rgba8();
    Some((image.width(), image.height(), image.into_raw()))
}

fn soft_button<'a>(content: impl Into<Element<'a, Message>>) -> iced::widget::Button<'a, Message> {
    iced::widget::button(content).style(button::secondary)
}
fn owned_playlist(route: &Route) -> Option<String> {
    match route {
        Route::Browse(id) if id.starts_with("VLPL") || id.starts_with("PL") => {
            Some(id.trim_start_matches("VL").to_owned())
        }
        _ => None,
    }
}
fn download_caption(store: &rustmusic_core::DownloadStore, id: &str) -> String {
    match store.phase(id) {
        DownloadPhase::Ready => "✓".into(),
        DownloadPhase::Active(progress) => format!("{progress}%"),
        DownloadPhase::Failed => "!".into(),
        DownloadPhase::Idle => "↓".into(),
    }
}
fn open_art() -> ArtCache {
    let directory = default_covers_dir();
    ArtCache::open(&directory).unwrap_or_else(|_| {
        ArtCache::open(
            std::env::temp_dir().join(format!("rustmusic-covers-{}", std::process::id())),
        )
        .expect("Не удалось создать каталог обложек")
    })
}
fn time(seconds: f32) -> String {
    let s = seconds.max(0.) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}
fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("RustMusic")
        .theme(Theme::custom(
            "RustMusic",
            iced::theme::Palette {
                background: Color::from_rgb8(16, 17, 23),
                text: Color::from_rgb8(238, 240, 246),
                primary: Color::from_rgb8(255, 99, 105),
                success: Color::from_rgb8(99, 205, 155),
                warning: Color::from_rgb8(225, 180, 90),
                danger: Color::from_rgb8(255, 99, 105),
            },
        ))
        .subscription(App::subscription)
        .window_size((1180., 800.))
        .run()
}

#[cfg(test)]
mod tests {
    use super::{
        keep_inflight_catalog, player_open_error, reconnect_prompt, session_may_open_home,
        startup_session,
    };

    #[test]
    fn restored_session_stays_active_when_account_probe_fails() {
        let startup = startup_session(true, false);
        assert!(startup.active);
        assert!(!startup.verified);
        assert!(!reconnect_prompt(startup.active));
        let verified = startup_session(true, true);
        assert!(verified.active && verified.verified);
        assert!(reconnect_prompt(startup_session(false, true).active));
    }

    #[test]
    fn repeated_catalog_click_keeps_the_inflight_request() {
        assert!(keep_inflight_catalog(true, true, false));
        assert!(!keep_inflight_catalog(true, false, false));
        assert!(!keep_inflight_catalog(false, true, false));
        assert!(!keep_inflight_catalog(true, true, true));
    }

    #[test]
    fn session_probe_does_not_replace_a_chosen_tab() {
        assert!(session_may_open_home(false));
        assert!(!session_may_open_home(true));
    }

    #[test]
    fn player_open_errors_do_not_ask_to_reconnect() {
        assert!(!player_open_error(true).contains("сесс"));
        assert!(!player_open_error(false).contains("сесс"));
        assert!(player_open_error(true).contains("кэш"));
        assert!(player_open_error(false).contains("адрес потока"));
    }
}
