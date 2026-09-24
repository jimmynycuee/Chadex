use std::io;

use chadex_runtime_core::build_info;

#[cfg(target_os = "macos")]
const MACOS_SERVER_RUNTIME_STACK_SIZE: usize = 8 * 1024 * 1024;

fn build_server_runtime() -> io::Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    #[cfg(target_os = "macos")]
    builder.thread_stack_size(MACOS_SERVER_RUNTIME_STACK_SIZE);
    builder.build()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => run_server(false),
        [arg] if arg == "--stop-on-stdin-eof" => run_server(true),
        [arg] if matches!(arg.as_str(), "--version" | "-V") => {
            print!("{}", build_info::version_output("chadex-runtime-server"));
            Ok(())
        }
        [arg] if matches!(arg.as_str(), "--help" | "-h") => {
            print!("Usage: chadex-runtime-server [OPTIONS]\n\nRun the Chadex local runtime server.\n\nOptions:\n      --stop-on-stdin-eof  Stop when the invoking parent closes stdin\n  -h, --help               Print help and exit\n  -V, --version            Print version and exit\n");
            Ok(())
        }
        _ => {
            eprintln!(
                "unknown argument(s): {}\nRun chadex-runtime-server --help for usage.",
                args.join(" ")
            );
            std::process::exit(2);
        }
    }
}

fn run_server(stop_on_stdin_eof: bool) -> Result<(), Box<dyn std::error::Error>> {
    chadex_runtime_engine::prepare_server_process_environment().map_err(io::Error::other)?;
    build_server_runtime()?.block_on(chadex_runtime_engine::run_server_with_parent_liveness(
        stop_on_stdin_eof,
    ))
}
