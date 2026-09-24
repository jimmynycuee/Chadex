use chadex_runtime_core::build_info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && matches!(args[0].as_str(), "--version" | "-V") {
        print!("{}", build_info::version_output("chadex-runtime-cli"));
        return Ok(());
    }
    chadex_runtime_cli::run().await
}
