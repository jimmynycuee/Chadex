#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    chadex_runtime_cli::run().await
}
