use chadex_runtime_core::build_info;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && matches!(args[0].as_str(), "--version" | "-V") {
        print!("{}", build_info::version_output("chadex-runtime-runner"));
        return;
    }
    chadex_runtime_runner::run_cli();
}
