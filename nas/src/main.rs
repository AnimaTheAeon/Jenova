//! Jenova for Synology DSM.
//!
//! `jenova-nas daemon`  background service (started by DSM)
//! `jenova-nas cgi`     the DSM web bridge (run by DSM as api.cgi)

mod api;
mod cgi;
mod config;
mod engine;
mod log;
mod python;
mod subs;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "daemon".into());
    match mode.as_str() {
        "cgi" => cgi::run(),
        "version" | "--version" => println!("{}", env!("CARGO_PKG_VERSION")),
        _ => daemon(),
    }
}

fn daemon() {
    let var = config::var_dir();
    let _ = std::fs::create_dir_all(&var);
    log::init(var.join("jenova.log"));
    let _ = std::fs::write(var.join("jenova.pid"), std::process::id().to_string());
    info!("Jenova {} starting", env!("CARGO_PKG_VERSION"));
    let shared = engine::start();
    api::serve(shared);
}
