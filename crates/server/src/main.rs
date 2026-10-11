use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use rustmusic_core::{
    Item, ItemKind, MusicClient, Mutation, Page, Queue, Repeat, Resolver, Session, valid_video_id,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::{RwLock, broadcast};
use tower_http::services::ServeDir;

#[derive(Clone)]
struct App {
    inner: Arc<Inner>,
}
struct Inner {
    key: [u8; 32],
    password: String,
    token: String,
    path: PathBuf,
    client: RwLock<MusicClient>,
    resolver: RwLock<Resolver>,
    events: broadcast::Sender<String>,
    http: reqwest::Client,
    queue: RwLock<Queue>,
}
struct ApiError(String);
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(e.to_string())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({"error":self.0}))).into_response()
    }
}
type Result<T> = std::result::Result<T, ApiError>;
fn encrypted(key: &[u8; 32], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("Invalid key"))?;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let payload = cipher
        .encrypt((&nonce).into(), plaintext)
        .map_err(|_| anyhow::anyhow!("Encryption failed"))?;
    Ok([nonce.to_vec(), payload].concat())
}
fn decrypted(key: &[u8; 32], bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    if bytes.len() < 28 {
        anyhow::bail!("Invalid encrypted session");
    }
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("Invalid key"))?
        .decrypt((&bytes[..12]).into(), &bytes[12..])
        .map_err(|_| anyhow::anyhow!("Session cannot be decrypted"))
}
fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(key, _)| *key == "rustmusic_session")
        .map(|(_, value)| value)
}
async fn guard(State(app): State<App>, request: Request, next: Next) -> Response {
    if !request.uri().path().starts_with("/api/v1/") {
        return next.run(request).await;
    }
    if request.method() != Method::GET
        && request
            .headers()
            .get("x-rustmusic")
            .and_then(|v| v.to_str().ok())
            != Some("1")
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Требуется запрос из RustMusic"})),
        )
            .into_response();
    }
    if let Some(origin) = request
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
    {
        let host = request
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if origin != format!("http://{host}") && origin != format!("https://{host}") {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    if request.uri().path() != "/api/v1/login"
        && !session_cookie(request.headers())
            .is_some_and(|token| bool::from(token.as_bytes().ct_eq(app.inner.token.as_bytes())))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Войдите с ключом вашего сервера"})),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}
#[derive(Deserialize)]
struct Login {
    password: String,
}
async fn login(State(app): State<App>, Json(body): Json<Login>) -> Response {
    if !bool::from(
        body.password
            .trim()
            .as_bytes()
            .ct_eq(app.inner.password.as_bytes()),
    ) {
        tokio::time::sleep(Duration::from_millis(500)).await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Неверный ключ сервера"})),
        )
            .into_response();
    }
    let mut response = Json(json!({"ok":true})).into_response();
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&format!(
            "rustmusic_session={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200",
            app.inner.token
        ))
        .unwrap(),
    );
    response
}
async fn logout_web() -> Response {
    let mut response = Json(json!({"ok":true})).into_response();
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_static(
            "rustmusic_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        ),
    );
    response
}
async fn status(State(app): State<App>) -> Result<Json<Value>> {
    let client = app.inner.client.read().await.clone();
    let account = if client.authenticated() {
        Some(client.account().await?)
    } else {
        None
    };
    Ok(Json(
        json!({"account":account,"version":env!("CARGO_PKG_VERSION")}),
    ))
}
#[derive(Deserialize)]
struct Import {
    input: String,
    #[serde(default = "zero")]
    auth_user: String,
}
fn zero() -> String {
    "0".into()
}
async fn import(State(app): State<App>, Json(body): Json<Import>) -> Result<Json<Value>> {
    let session = Session::import(&body.input, &body.auth_user)?;
    let client = MusicClient::new(Some(session.clone()))?;
    let _ = app.inner.events.send("Проверка аккаунта".into());
    let account = client.account().await?;
    let bytes = encrypted(
        &app.inner.key,
        &serde_json::to_vec(&session).map_err(anyhow::Error::from)?,
    )?;
    let temp = app.inner.path.with_extension("tmp");
    tokio::fs::write(&temp, bytes)
        .await
        .map_err(anyhow::Error::from)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(anyhow::Error::from)?;
    }
    tokio::fs::rename(temp, &app.inner.path)
        .await
        .map_err(anyhow::Error::from)?;
    *app.inner.client.write().await = client;
    *app.inner.resolver.write().await = Resolver::new(Some(session));
    let _ = app.inner.events.send("Аккаунт подключён".into());
    Ok(Json(json!({"account":account})))
}
async fn disconnect(State(app): State<App>) -> Result<Json<Value>> {
    match tokio::fs::remove_file(&app.inner.path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::Error::from(e).into()),
    }
    *app.inner.queue.write().await = Queue::default();
    *app.inner.client.write().await = MusicClient::new(None)?;
    *app.inner.resolver.write().await = Resolver::new(None);
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct Paging {
    continuation: Option<String>,
}
#[derive(Deserialize)]
struct Search {
    q: String,
    continuation: Option<String>,
}
async fn search(State(app): State<App>, Query(q): Query<Search>) -> Result<Json<Page>> {
    let client = app.inner.client.read().await.clone();
    Ok(Json(client.search(&q.q, q.continuation.as_deref()).await?))
}
async fn browse(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<Paging>,
) -> Result<Json<Page>> {
    let client = app.inner.client.read().await.clone();
    Ok(Json(client.browse(&id, q.continuation.as_deref()).await?))
}
async fn radio(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<Paging>,
) -> Result<Json<Page>> {
    let client = app.inner.client.read().await.clone();
    Ok(Json(client.radio(&id, q.continuation.as_deref()).await?))
}
async fn mutate(State(app): State<App>, Json(body): Json<Mutation>) -> Result<Json<Value>> {
    let client = app.inner.client.read().await.clone();
    Ok(Json(client.mutate(body).await?))
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum QueueCommand {
    Replace {
        items: Vec<Item>,
        index: usize,
    },
    Append {
        item: Item,
    },
    Select {
        index: usize,
    },
    Next {
        #[serde(default)]
        ended: bool,
    },
    Previous,
    Shuffle,
    Repeat,
}
async fn queue_get(State(app): State<App>) -> Json<Queue> {
    Json(app.inner.queue.read().await.clone())
}
async fn queue_post(
    State(app): State<App>,
    Json(command): Json<QueueCommand>,
) -> Result<Json<Queue>> {
    let mut queue = app.inner.queue.write().await;
    let check = |item: &Item| -> Result<()> {
        if item.kind != ItemKind::Track
            || !valid_video_id(&item.id)
            || item.title.len() > 1000
            || item.subtitle.len() > 1000
        {
            return Err(ApiError("Некорректный трек очереди".into()));
        }
        Ok(())
    };
    match command {
        QueueCommand::Replace { items, index } => {
            if items.len() > 500 {
                return Err(ApiError("Очередь ограничена 500 треками".into()));
            }
            for item in &items {
                check(item)?;
            }
            queue.replace(items, index);
        }
        QueueCommand::Append { item } => {
            check(&item)?;
            if queue.items.len() >= 500 {
                return Err(ApiError("Очередь заполнена".into()));
            }
            queue.append(item);
        }
        QueueCommand::Select { index } => {
            if index >= queue.items.len() {
                return Err(ApiError("Трек отсутствует в очереди".into()));
            }
            queue.index = Some(index);
        }
        QueueCommand::Next { ended } => {
            queue.advance(ended);
        }
        QueueCommand::Previous => {
            queue.previous();
        }
        QueueCommand::Shuffle => queue.shuffle(),
        QueueCommand::Repeat => {
            queue.repeat = match queue.repeat {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            }
        }
    }
    Ok(Json(queue.clone()))
}
async fn resolve(State(app): State<App>, Path(id): Path<String>) -> Result<Json<Value>> {
    let resolver = app.inner.resolver.read().await.clone();
    let _ = app.inner.events.send("Получение аудиопотока".into());
    resolver.resolve(&id, true).await?;
    let _ = app.inner.events.send("Аудиопоток готов".into());
    Ok(Json(json!({"url":format!("/api/v1/audio/{id}")})))
}
async fn audio(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response> {
    let resolver = app.inner.resolver.read().await.clone();
    let stream = resolver.resolve(&id, true).await?;
    let mut request = app.inner.http.get(&stream.url);
    for (key, value) in &stream.headers {
        request = request.header(key, value);
    }
    if let Some(range) = headers.get("range") {
        request = request.header("range", range);
    }
    let upstream = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Не удалось подключиться к аудиопотоку"))?;
    let status = upstream.status();
    if !status.is_success() && status.as_u16() != 416 {
        return Err(ApiError(
            "Аудиопоток недоступен. Повторите воспроизведение.".into(),
        ));
    }
    let mut builder = Response::builder().status(status);
    for name in [
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
    ] {
        if let Some(value) = upstream.headers().get(name) {
            builder = builder.header(name, value);
        }
    }
    let response = builder
        .header("cache-control", "no-store")
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(anyhow::Error::from)?;
    Ok(response)
}
async fn events(
    State(app): State<App>,
) -> Sse<impl futures_util::Stream<Item = std::result::Result<Event, std::convert::Infallible>>> {
    let mut receiver = app.inner.events.subscribe();
    Sse::new(async_stream::stream! {loop {match receiver.recv().await {Ok(message)=>yield Ok(Event::default().event("progress").data(message)),Err(broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>break}}}).keep_alive(KeepAlive::default())
}
fn router(app: App, web: &str) -> Router {
    let api = Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout_web))
        .route("/status", get(status))
        .route("/session", post(import).delete(disconnect))
        .route("/search", get(search))
        .route("/browse/{id}", get(browse))
        .route("/radio/{id}", get(radio))
        .route("/queue", get(queue_get).post(queue_post))
        .route("/mutate", post(mutate))
        .route("/resolve/{id}", post(resolve))
        .route("/audio/{id}", get(audio))
        .route("/events", get(events));
    Router::new().nest("/api/v1",api).layer(middleware::from_fn_with_state(app.clone(),guard))
        .fallback_service(ServeDir::new(web).append_index_html_on_directories(true))
        .layer(DefaultBodyLimit::max(300*1024))
        .layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(axum::http::header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'self'; img-src 'self' https://*.ytimg.com https://*.googleusercontent.com https://*.ggpht.com; media-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'")))
        .with_state(app)
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let key_path = std::env::var("RUSTMUSIC_SECRET_FILE").unwrap_or(".secrets/server-key".into());
    let password = tokio::fs::read_to_string(&key_path)
        .await
        .map_err(|_| anyhow::anyhow!("Создайте ключ: ./scripts/setup-docker.sh"))?
        .trim()
        .to_owned();
    let key: [u8; 32] = hex::decode(&password)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Key must contain 64 hexadecimal characters"))?;
    let directory = PathBuf::from(std::env::var("RUSTMUSIC_DATA_DIR").unwrap_or(".data".into()));
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join("session.enc");
    let session = if path.exists() {
        Some(serde_json::from_slice::<Session>(&decrypted(
            &key,
            &tokio::fs::read(&path).await?,
        )?)?)
    } else {
        None
    };
    let (events, _) = broadcast::channel(32);
    let mut token = [0u8; 32];
    OsRng.fill_bytes(&mut token);
    let app = App {
        inner: Arc::new(Inner {
            key,
            password,
            token: hex::encode(token),
            path,
            queue: RwLock::new(Queue::default()),
            client: RwLock::new(MusicClient::new(session.clone())?),
            resolver: RwLock::new(Resolver::new(session)),
            events,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        }),
    };
    let addr = std::env::var("RUSTMUSIC_LISTEN").unwrap_or("127.0.0.1:8080".into());
    let web = std::env::var("RUSTMUSIC_WEB_DIR").unwrap_or("web/dist".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("RustMusic: http://{addr}");
    axum::serve(listener, router(app, &web))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    fn app() -> App {
        let (events, _) = broadcast::channel(4);
        App {
            inner: Arc::new(Inner {
                key: [7; 32],
                password: "test".into(),
                token: "token".into(),
                path: PathBuf::from("unused"),
                queue: RwLock::new(Queue::default()),
                client: RwLock::new(MusicClient::new(None).unwrap()),
                resolver: RwLock::new(Resolver::new(None)),
                events,
                http: reqwest::Client::new(),
            }),
        }
    }
    #[test]
    fn encryption_authenticates_ciphertext() {
        let bytes = encrypted(&[7; 32], b"session").unwrap();
        assert_eq!(decrypted(&[7; 32], &bytes).unwrap(), b"session");
        assert!(decrypted(&[8; 32], &bytes).is_err());
        let mut damaged = bytes;
        damaged[12] ^= 1;
        assert!(decrypted(&[7; 32], &damaged).is_err());
    }
    #[tokio::test]
    async fn protects_api_and_rejects_cross_origin() {
        let response = router(app(), "web/dist")
            .oneshot(
                Request::builder()
                    .uri("/api/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = router(app(), "web/dist")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/login")
                    .header("origin", "https://evil.test")
                    .header("host", "localhost:8080")
                    .header("x-rustmusic", "1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"password":"test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    #[tokio::test]
    async fn authenticates_with_httponly_cookie() {
        let response = router(app(), "web/dist")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/login")
                    .header("x-rustmusic", "1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"password":"test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains("HttpOnly; SameSite=Strict")
        );
    }
}
