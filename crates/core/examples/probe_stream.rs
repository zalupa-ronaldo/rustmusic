use rustmusic_core::Resolver;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let id = std::env::args().nth(1).unwrap_or("4D7u5KF7SP8".into());
    let stream = Resolver::new(None).resolve(&id, false).await?;
    println!(
        "Audio stream resolved: {} (URL and headers redacted)",
        stream.mime
    );
    Ok(())
}
