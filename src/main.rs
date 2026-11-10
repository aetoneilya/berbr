mod artifacts;
mod cli;
mod client;
mod config;
mod log;
mod metadata;
mod table;
mod trace;
mod tui;
mod vault;

fn main() {
    log::install_panic_hook();
    let _total = trace::span("total");
    let result = cli::run();
    log::finish(&result);
    if let Err(err) = result {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}
