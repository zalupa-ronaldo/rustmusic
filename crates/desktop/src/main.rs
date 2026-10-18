#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod player;
mod vault;
use iced::{
    Color, Element, Length, Subscription, Task, Theme,
    widget::{
        button, column, container, image, pick_list, row, rule, scrollable, slider, text,
        text_input,
    },
};
use rustmusic_core::{
    Account, Item, ItemKind, MusicClient, Mutation, Page, Queue, Repeat, Resolver, Session,
};
use serde_json::json;
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Clone)]
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
    PlayResult(u64, Result<(), String>),
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
}
struct App {
    client: MusicClient,
    resolver: Resolver,
    player: player::Player,
    account: Option<Account>,
    route: Route,
    items: Vec<Item>,
    continuation: Option<String>,
    query: String,
    status: String,
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
        let app = Self {
            client: MusicClient::new(None).expect("HTTP client"),
            resolver: Resolver::new(None),
            player: player::Player::default(),
            account: None,
            route: Route::Home,
            items: vec![],
            continuation: None,
            query: String::new(),
            status: "Подключите Google-аккаунт в настройках".into(),
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
            #[cfg(target_os = "macos")]
            media,
            #[cfg(target_os = "macos")]
            media_events,
        };
        (
            app,
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
        )
    }
    fn fetch(&mut self, route: Route, more: bool) -> Task<Message> {
        if let Some(handle) = self.fetch.take() {
            handle.abort();
        }
        self.route = route.clone();
        self.request += 1;
        let generation = self.request;
        self.loading = true;
        self.status = "Загрузка…".into();
        self.selected = None;
        self.delete_confirm = false;
        if !more {
            self.items.clear();
            self.continuation = None;
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
        self.player.set_epoch(self.play_id);
        let Some(item) = self.queue.current().cloned() else {
            self.playing = false;
            self.loading_audio = false;
            self.media_update();
            return self.command(json!(["stop"]));
        };
        let id = self.play_id;
        self.loading_audio = true;
        self.playing = false;
        self.position = 0.;
        self.duration = 0.;
        self.status = format!("Получение потока: {}", item.title);
        let resolver = self.resolver.clone();
        let player = self.player.clone();
        let task = Task::perform(
            async move {
                result(
                    async {
                        let stream = resolver.resolve(&item.id, false).await?;
                        player.load(stream, id).await?;
                        Ok(())
                    }
                    .await,
                )
            },
            move |r| Message::PlayResult(id, r),
        );
        let (task, handle) = task.abortable();
        self.play_task = Some(handle);
        task
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
                Task::perform(
                    async move { result(client.account().await.map(|a| (session, a))) },
                    Message::SessionConnected,
                )
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
                        if startup.active {
                            self.install_session(session);
                        }
                        if startup.verified {
                            self.account = Some(account);
                        }
                        self.manual.clear();
                        self.settings = false;
                        self.fetch(Route::Home, false)
                    }
                    Err(e) => {
                        self.status = e;
                        if reconnect_prompt(self.client.session().is_some()) {
                            self.settings = true;
                            Task::none()
                        } else {
                            self.fetch(Route::Home, false)
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
                    self.fetch(Route::Search(self.query.clone()), false)
                }
            }
            Message::Go(route) => {
                self.settings = false;
                self.fetch(route, false)
            }
            Message::More => self.fetch(self.route.clone(), true),
            Message::Page(id, more, response) => {
                if id != self.request {
                    return Task::none();
                }
                self.loading = false;
                self.fetch = None;
                match response {
                    Err(e) => {
                        self.status = e;
                        Task::none()
                    }
                    Ok(page) => {
                        if !more {
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
                        let tasks = self
                            .items
                            .iter()
                            .take(24)
                            .filter_map(|i| i.thumbnail.clone())
                            .filter(|url| !self.covers.contains_key(url))
                            .map(|url| {
                                Task::perform(
                                    async move {
                                        let data = async {
                                            let client = reqwest::Client::builder()
                                                .timeout(Duration::from_secs(8))
                                                .build()
                                                .ok()?;
                                            let response = client.get(&url).send().await.ok()?;
                                            if response
                                                .content_length()
                                                .is_some_and(|n| n > 2 * 1024 * 1024)
                                            {
                                                return None;
                                            }
                                            let bytes = response.bytes().await.ok()?;
                                            if bytes.len() > 2 * 1024 * 1024 {
                                                return None;
                                            }
                                            let mut reader = ::image::ImageReader::new(
                                                std::io::Cursor::new(bytes),
                                            )
                                            .with_guessed_format()
                                            .ok()?;
                                            let mut limits = ::image::Limits::default();
                                            limits.max_image_width = Some(2048);
                                            limits.max_image_height = Some(2048);
                                            limits.max_alloc = Some(16 * 1024 * 1024);
                                            reader.limits(limits);
                                            let image = reader
                                                .decode()
                                                .ok()?
                                                .thumbnail(128, 128)
                                                .to_rgba8();
                                            Some((image.width(), image.height(), image.into_raw()))
                                        }
                                        .await;
                                        (url, data)
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
            Message::PlayResult(id, response) => {
                if id != self.play_id {
                    return Task::none();
                }
                self.loading_audio = false;
                self.play_task = None;
                match response {
                    Ok(()) => {
                        self.playing = true;
                        self.status = "Воспроизведение".into();
                        self.media_update();
                        self.command(json!(["set_property", "volume", self.volume]))
                    }
                    Err(e) => {
                        self.playing = false;
                        self.status = e;
                        Task::none()
                    }
                }
            }
            Message::Toggle => {
                if self.queue.current().is_none() {
                    return Task::none();
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
                if !self.polling && !self.loading_audio && self.queue.current().is_some() {
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
                            self.status="Аудиоплеер не смог открыть трек. Повторите выбор или подключите сессию заново.".into();
                            self.playing = false;
                            self.queue.index = None;
                            self.media_update();
                            return Task::none();
                        }
                        self.position = status.position;
                        self.duration = status.duration;
                        self.playing = !status.paused;
                        self.media_update();
                        if status.ended && !self.loading_audio {
                            return self.update(Message::Next(true));
                        }
                    }
                    Err(e) => {
                        self.status = e;
                        self.playing = false;
                    }
                }
                Task::none()
            }
            Message::PlayerResult(response) => {
                if let Err(e) = response {
                    self.status = e;
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
                    if let Some(handle) = self.play_task.take() {
                        handle.abort();
                    }
                    self.account = None;
                    self.client = MusicClient::new(None).unwrap();
                    self.resolver = Resolver::new(None);
                    self.queue = Queue::default();
                    self.playing = false;
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
                match response {
                    Ok(id) => {
                        self.status = "Изменение сохранено".into();
                        if !id.is_empty() {
                            self.target = id.clone();
                            self.fetch(Route::Browse(id), false)
                        } else {
                            self.fetch(self.route.clone(), false)
                        }
                    }
                    Err(e) => {
                        self.status = e;
                        Task::none()
                    }
                }
            }
            Message::DeleteConfirm(v) => {
                self.delete_confirm = v;
                Task::none()
            }
        }
    }
    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick)
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
                    soft_button(
                        text(format!(
                            "{} {}",
                            if self.queue.index == Some(i) {
                                "▶"
                            } else {
                                ""
                            },
                            item.title
                        ))
                        .size(12),
                    )
                    .on_press(Message::PlayQueue(i))
                    .width(Length::Fill)
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
            soft_button("Аккаунт / настройки").on_press(Message::Settings)
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
                let info = column![
                    text(&item.title).size(16),
                    text(&item.subtitle)
                        .size(12)
                        .color(Color::from_rgb8(150, 153, 165))
                ]
                .spacing(4);
                let mut line = row![
                    cover,
                    soft_button(info)
                        .style(button::text)
                        .on_press(Message::Open(index))
                        .width(Length::Fill),
                    text(item.duration.as_deref().unwrap_or("")).size(12)
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center);
                if item.kind == ItemKind::Track {
                    line = line.push(soft_button("•••").on_press(Message::TrackMenu(index)));
                }
                list = list.push(container(line).padding(6));
                if self.selected == Some(index) {
                    list = list.push(
                        row![
                            soft_button("В очередь").on_press(Message::Enqueue(index)),
                            soft_button("Радио").on_press(Message::Radio(index)),
                            soft_button("♥").on_press(Message::Mutate(Mutation::Like {
                                video_id: item.id.clone(),
                                liked: true
                            })),
                            soft_button("Снять лайк").on_press(Message::Mutate(Mutation::Like {
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
        let footer = column![
            row![
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
                    .size(12)
                ]
                .width(Length::Fill),
                controls,
                text("Громкость").size(12),
                slider(0.0..=100.0, self.volume, Message::Volume).width(100)
            ]
            .spacing(16)
            .align_y(iced::Alignment::Center),
            row![
                text(time(self.position)).size(12),
                slider(
                    0.0..=self.duration.max(1.),
                    self.position.min(self.duration.max(1.)),
                    Message::Seek
                ),
                text(time(self.duration)).size(12)
            ]
            .spacing(12)
        ]
        .spacing(12)
        .padding(18);
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

fn soft_button<'a>(content: impl Into<Element<'a, Message>>) -> iced::widget::Button<'a, Message> {
    iced::widget::button(content).style(button::secondary)
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
    use super::{reconnect_prompt, startup_session};

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
}
