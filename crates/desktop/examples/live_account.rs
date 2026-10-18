//! Explicit opt-in account check. Never prints session contents or personal library data.
#[path = "../src/vault.rs"]
#[allow(dead_code)]
mod vault;
use anyhow::{Context, Result, ensure};
use rustmusic_core::{ItemKind, MusicClient, Mutation};
#[tokio::main]
async fn main() -> Result<()> {
    ensure!(
        std::env::var("RUSTMUSIC_LIVE_ACCOUNT").as_deref() == Ok("1"),
        "Set RUSTMUSIC_LIVE_ACCOUNT=1 to test the session already saved by the app"
    );
    let session = if std::env::var("RUSTMUSIC_TEST_CONNECT_CHROME_DEFAULT").as_deref() == Ok("1") {
        let session = rustmusic_core::Session::from_browser("chrome", "Default", "0").await?;
        MusicClient::new(Some(session.clone()))?.account().await?;
        vault::save(&session)?;
        session
    } else {
        vault::load()?.context("Connect your account in the app first")?
    };
    let client = MusicClient::new(Some(session))?;
    client.account().await?;
    println!("Account verified (identity omitted)");
    for id in [
        "FEmusic_home",
        "FEmusic_liked_playlists",
        "VLLM",
        "FEmusic_liked_albums",
        "FEmusic_library_corpus_track_artists",
    ] {
        let page = client.browse(id, None).await?;
        println!(
            "{id}: {} items; pagination={}",
            page.items.len(),
            page.continuation.is_some()
        );
    }
    ensure!(
        !client.radio("4D7u5KF7SP8", None).await?.items.is_empty(),
        "Empty radio"
    );
    let liked = client.browse("VLLM", None).await?;
    if let Some(track) = liked.items.iter().find(|item| item.kind == ItemKind::Track) {
        client
            .mutate(Mutation::Like {
                video_id: track.id.clone(),
                liked: false,
            })
            .await?;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let removed = client
            .request(
                "next",
                serde_json::json!({"videoId":track.id,"isAudioOnly":true}),
            )
            .await;
        // Restore the original like even if the subsequent read fails.
        client
            .mutate(Mutation::Like {
                video_id: track.id.clone(),
                liked: true,
            })
            .await?;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let restored = client
            .request(
                "next",
                serde_json::json!({"videoId":track.id,"isAudioOnly":true}),
            )
            .await?;
        ensure!(
            rustmusic_core::provider::find_key(&removed?, "likeStatus")
                .and_then(serde_json::Value::as_str)
                == Some("INDIFFERENT"),
            "Unlike did not persist"
        );
        ensure!(
            rustmusic_core::provider::find_key(&restored, "likeStatus")
                .and_then(serde_json::Value::as_str)
                == Some("LIKE"),
            "Original like was not restored"
        );
        println!("Unlike/like verified; original like restored");
    }
    if std::env::var("RUSTMUSIC_TEST_DOCKER").as_deref() == Ok("1") {
        let http = reqwest::Client::new();
        let key = std::fs::read_to_string(".secrets/server-key")?;
        let login = http
            .post("http://127.0.0.1:8080/api/v1/login")
            .header("x-rustmusic", "1")
            .json(&serde_json::json!({"password":key.trim()}))
            .send()
            .await?;
        ensure!(login.status().is_success(), "Docker login failed");
        let cookie = login
            .headers()
            .get("set-cookie")
            .context("Missing local session cookie")?
            .to_str()?
            .split(';')
            .next()
            .context("Invalid local cookie")?
            .to_owned();
        let session = client.session().context("Missing session")?;
        let import = http
            .post("http://127.0.0.1:8080/api/v1/session")
            .header("cookie", &cookie)
            .header("x-rustmusic", "1")
            .json(&serde_json::json!({"input":session.cookie,"auth_user":session.auth_user}))
            .send()
            .await?;
        ensure!(import.status().is_success(), "Docker account import failed");
        let library = http
            .get("http://127.0.0.1:8080/api/v1/browse/VLLM")
            .header("cookie", &cookie)
            .send()
            .await?;
        ensure!(
            library.status().is_success(),
            "Docker authenticated library failed"
        );
        let page: rustmusic_core::Page = library.json().await?;
        ensure!(!page.items.is_empty(), "Empty Docker liked library");
        println!("Docker encrypted session import and authenticated library verified");
    }
    let created = client
        .mutate(Mutation::Create {
            title: "RustMusic temporary integration check".into(),
        })
        .await?;
    let id = created["playlist_id"]
        .as_str()
        .context("Missing playlist ID")?
        .to_owned();
    // Always attempt to clean up this disposable playlist, including on an assertion failure.
    let checked: Result<()> = async {
        for video in ["4D7u5KF7SP8", "CqZgd6-xQl8"] {
            client
                .mutate(Mutation::Add {
                    playlist_id: id.clone(),
                    video_id: video.into(),
                })
                .await?;
        }
        let first = client.browse(&id, None).await?;
        let tracks: Vec<_> = first
            .items
            .into_iter()
            .filter(|i| i.kind == ItemKind::Track)
            .collect();
        ensure!(tracks.len() == 2, "Expected two test tracks");
        client
            .mutate(Mutation::Rename {
                playlist_id: id.clone(),
                title: "RustMusic renamed integration check".into(),
            })
            .await?;
        client
            .mutate(Mutation::Move {
                playlist_id: id.clone(),
                set_video_id: tracks[1]
                    .set_video_id
                    .clone()
                    .context("Missing track membership")?,
                before_set_video_id: tracks[0]
                    .set_video_id
                    .clone()
                    .context("Missing track membership")?,
            })
            .await?;
        let moved = client.browse(&id, None).await?;
        let moved: Vec<_> = moved
            .items
            .into_iter()
            .filter(|i| i.kind == ItemKind::Track)
            .collect();
        ensure!(
            moved[0].id == tracks[1].id,
            "Playlist reorder did not persist"
        );
        client
            .mutate(Mutation::Remove {
                playlist_id: id.clone(),
                video_id: moved[0].id.clone(),
                set_video_id: moved[0]
                    .set_video_id
                    .clone()
                    .context("Missing track membership")?,
            })
            .await?;
        let remaining = client.browse(&id, None).await?;
        ensure!(
            remaining
                .items
                .iter()
                .filter(|i| i.kind == ItemKind::Track)
                .count()
                == 1,
            "Playlist removal did not persist"
        );
        println!("Private playlist create/add/rename/move/remove verified");
        Ok(())
    }
    .await;
    let cleanup = client.mutate(Mutation::Delete { playlist_id: id }).await;
    cleanup.context("Disposable playlist cleanup failed; inspect your library")?;
    println!("Disposable playlist deleted");
    checked?;
    Ok(())
}
