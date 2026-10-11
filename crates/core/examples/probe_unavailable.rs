use rustmusic_core::Resolver;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let result = Resolver::new(None).resolve("___________", false).await;
    anyhow::ensure!(
        result.is_err(),
        "An unavailable fixture video unexpectedly resolved"
    );
    println!("Unavailable track rejected with an explicit error");
    Ok(())
}
