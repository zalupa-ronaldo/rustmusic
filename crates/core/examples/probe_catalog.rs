use rustmusic_core::MusicClient;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = MusicClient::new(None)?;
    for id in [
        "FEmusic_home",
        "MPREb_7wWKjwcSBkR",
        "UClYV6hHlupm_S_ObS1W-DYw",
    ] {
        let page = client.browse(id, None).await?;
        println!(
            "{id}: {} items; continuation={}",
            page.items.len(),
            page.continuation.is_some()
        );
    }
    let home = client.browse("FEmusic_home", None).await?;
    if let Some(token) = home.continuation {
        let more = client.browse("FEmusic_home", Some(&token)).await?;
        println!("Home page 2: {} items", more.items.len());
    }
    let page = client.radio("4D7u5KF7SP8", None).await?;
    println!(
        "Radio: {} tracks; continuation={}",
        page.items.len(),
        page.continuation.is_some()
    );
    Ok(())
}
