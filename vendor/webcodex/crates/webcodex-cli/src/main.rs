#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    webcodex_cli::run().await
}
