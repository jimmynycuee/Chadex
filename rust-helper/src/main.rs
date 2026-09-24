mod chadex_core;
mod runtime_bridge;
mod tunnel_supervisor;

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--supervise-tunnel")) {
        // Do not create the NDJSON/Tokio runtime in the pipe-lease supervisor.
        // Exiting also releases the stdin reader if the tunnel exits first.
        std::process::exit(tunnel_supervisor::run(args).unwrap_or(1));
    }
    runtime_bridge::run();
}
