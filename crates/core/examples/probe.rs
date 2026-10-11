use rustmusic_core::MusicClient;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = MusicClient::new(None)?;
    let query = std::env::args().nth(1).unwrap_or("Daft Punk".into());
    let page = client.search(&query, None).await?;
    println!(
        "{} public search results; continuation={}",
        page.items.len(),
        page.continuation.is_some()
    );
    for item in page.items.iter().take(6) {
        println!(
            "{:?}: {} — {} ({})",
            item.kind, item.title, item.subtitle, item.id
        );
    }
    Ok(())
}
