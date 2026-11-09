//! Loopback-only JSON API used by the DSM window (through api.cgi).
//! Every request must carry a valid DSM session for an administrator.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::config::{is_system_folder, Config};
use crate::engine::{self, folder_access, Msg, Shared};
use crate::{info, warn};

pub const DEFAULT_PORT: u16 = 5459;
const AUTH_TTL: Duration = Duration::from_secs(300);

pub fn port() -> u16 {
    std::env::var("JENOVA_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

type AuthCache = Mutex<HashMap<(String, String), (String, Instant)>>;

pub fn serve(shared: Arc<Shared>) {
    let addr = format!("127.0.0.1:{}", port());
    let server = match Server::http(&addr) {
        Ok(server) => server,
        Err(error) => {
            warn!("Can't listen on {addr}: {error}");
            std::process::exit(1);
        }
    };
    info!("API listening on {addr}");
    let cache: AuthCache = Mutex::new(HashMap::new());
    for request in server.incoming_requests() {
        handle(request, &shared, &cache);
    }
}

fn header(req: &Request, name: &str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn query(url: &str) -> HashMap<String, String> {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (!k.is_empty()).then(|| (percent_decode(k), percent_decode(v)))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Check the DSM session with DSM's own authenticate.cgi; admins only.
fn authenticate(cookie: &str, token: &str, remote: &str, cache: &AuthCache) -> Result<String, String> {
    if std::env::var("JENOVA_NO_AUTH").is_ok_and(|v| v == "1") {
        return Ok("dev".into());
    }
    if cookie.is_empty() {
        return Err("Not signed in to DSM".into());
    }
    let key = (cookie.to_string(), token.to_string());
    if let Some((user, at)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        if at.elapsed() < AUTH_TTL {
            return Ok(user.clone());
        }
    }
    let script = [
        "/usr/syno/synoman/webman/authenticate.cgi",
        "/usr/syno/synoman/webman/modules/authenticate.cgi",
    ]
    .into_iter()
    .find(|p| Path::new(p).exists())
    .ok_or("DSM sign-in check isn't available")?;
    let output = std::process::Command::new(script)
        .env_clear()
        .env("HTTP_COOKIE", cookie)
        .env("HTTP_X_SYNO_TOKEN", token)
        .env("REMOTE_ADDR", if remote.is_empty() { "127.0.0.1" } else { remote })
        .env("REQUEST_METHOD", "GET")
        .env("QUERY_STRING", format!("SynoToken={token}"))
        .output()
        .map_err(|e| e.to_string())?;
    let user = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if user.is_empty() {
        return Err("Your DSM session has expired. Sign in to DSM again.".into());
    }
    if !is_admin(&user) {
        return Err("Only DSM administrators can use Jenova".into());
    }
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (_, at)| at.elapsed() < AUTH_TTL);
    cache.insert(key, (user.clone(), Instant::now()));
    Ok(user)
}

fn is_admin(user: &str) -> bool {
    if user == "root" {
        return true;
    }
    std::process::Command::new("id")
        .args(["-Gn", user])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|g| g == "administrators"))
        .unwrap_or(false)
}

fn respond(req: Request, code: u16, body: Value) {
    let data = body.to_string();
    let response = Response::from_string(data)
        .with_status_code(code)
        .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap())
        .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
    let _ = req.respond(response);
}

fn handle(mut req: Request, shared: &Arc<Shared>, cache: &AuthCache) {
    let q = query(req.url());
    let route = q.get("r").cloned().unwrap_or_default();
    let cookie = header(&req, "X-Jenova-Cookie");
    let token = header(&req, "X-Jenova-Token");
    let remote = header(&req, "X-Jenova-Remote");
    if let Err(message) = authenticate(&cookie, &token, &remote, cache) {
        return respond(req, 401, json!({ "error": message }));
    }
    let mut body = String::new();
    if *req.method() == Method::Post {
        let _ = req.as_reader().take(64 * 1024).read_to_string(&mut body);
    }
    let input: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let post = *req.method() == Method::Post;

    let result: Result<Value, String> = match (post, route.as_str()) {
        (false, "status") => Ok(status(shared)),
        (false, "config") => Ok(config_json(shared)),
        (true, "config") => save_config(shared, input),
        (true, "folders/add") => add_folder(shared, input),
        (true, "folders/remove") => remove_folder(shared, input),
        (true, "check") => {
            shared.send(Msg::CheckNow);
            Ok(json!({ "ok": true }))
        }
        (false, "jobs") => Ok(jobs(shared)),
        (true, "jobs/clear") => {
            shared.clear_finished_jobs();
            Ok(json!({ "ok": true }))
        }
        (false, "log") => Ok(json!({ "lines": crate::log::recent() })),
        (false, "browse") => browse(q.get("path").map(String::as_str).unwrap_or("")),
        (true, "subliminal/reinstall") => {
            engine::request_reinstall(shared);
            Ok(json!({ "ok": true }))
        }
        _ => Err("Unknown request".into()),
    };
    match result {
        Ok(value) => respond(req, 200, value),
        Err(message) => respond(req, 400, json!({ "error": message })),
    }
}

fn status(shared: &Arc<Shared>) -> Value {
    let config = shared.config.read().unwrap_or_else(|e| e.into_inner()).clone();
    let folder_info = shared.folders.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let folders: Vec<Value> = config
        .folders
        .iter()
        .map(|path| {
            let info = folder_info.get(path).cloned().unwrap_or_default();
            json!({
                "path": path,
                "name": Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone()),
                "state": info.state,
                "error": info.error,
                "videos": info.videos,
                "with_subs": info.with_subs,
                "readable": info.readable,
                "writable": info.writable,
            })
        })
        .collect();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime": shared.started.elapsed().as_secs(),
        "watch_enabled": config.watch_enabled,
        "subliminal": *shared.subliminal.lock().unwrap_or_else(|e| e.into_inner()),
        "folders": folders,
        "settling": shared.settling.load(Ordering::SeqCst),
        "queued": shared.queued.load(Ordering::SeqCst),
        "running": shared.running.load(Ordering::SeqCst),
        "scanning": shared.scanning.load(Ordering::SeqCst) > 0,
        "subs_total": shared.subs_found_total(),
        "user": std::env::var("USER").unwrap_or_default(),
    })
}

fn config_json(shared: &Arc<Shared>) -> Value {
    let config = shared.config.read().unwrap_or_else(|e| e.into_inner()).clone();
    let mut value = serde_json::to_value(&config).unwrap_or(Value::Null);
    if let Some(obj) = value.as_object_mut() {
        let has_password = !config.opensubtitles_password.is_empty();
        obj.insert("opensubtitles_password".into(), Value::String(String::new()));
        obj.insert("has_password".into(), Value::Bool(has_password));
    }
    value
}

fn save_config(shared: &Arc<Shared>, input: Value) -> Result<Value, String> {
    let Value::Object(changes) = input else {
        return Err("Expected an object".into());
    };
    let current = shared.config.read().unwrap_or_else(|e| e.into_inner()).clone();
    let mut merged = serde_json::to_value(&current).map_err(|e| e.to_string())?;
    let target = merged.as_object_mut().ok_or("bad config")?;
    for (key, value) in changes {
        // An empty password field means "keep the saved one".
        if key == "opensubtitles_password" && value.as_str() == Some("") {
            continue;
        }
        if key == "clear_password" {
            if value.as_bool() == Some(true) {
                target.insert("opensubtitles_password".into(), Value::String(String::new()));
            }
            continue;
        }
        if target.contains_key(&key) {
            target.insert(key, value);
        }
    }
    let config: Config = serde_json::from_value(merged).map_err(|e| format!("Invalid setting: {e}"))?;
    shared.update_config(config).map_err(|e| format!("Couldn't save settings: {e}"))?;
    Ok(config_json(shared))
}

fn add_folder(shared: &Arc<Shared>, input: Value) -> Result<Value, String> {
    let path = input.get("path").and_then(Value::as_str).ok_or("Missing folder")?;
    let path = path.trim_end_matches('/').to_string();
    if !inside_volume(Path::new(&path)) || !Path::new(&path).is_dir() {
        return Err("Pick a folder on one of the NAS volumes".into());
    }
    let mut config = shared.config.read().unwrap_or_else(|e| e.into_inner()).clone();
    if config.folders.iter().any(|f| f == &path) {
        return Err("That folder is already watched".into());
    }
    if config.folders.iter().any(|f| Path::new(&path).starts_with(f)) {
        return Err("That folder is already covered by a watched parent folder".into());
    }
    config.folders.push(path.clone());
    info!("Added folder {path}");
    shared.update_config(config).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true }))
}

fn remove_folder(shared: &Arc<Shared>, input: Value) -> Result<Value, String> {
    let path = input.get("path").and_then(Value::as_str).ok_or("Missing folder")?;
    let mut config = shared.config.read().unwrap_or_else(|e| e.into_inner()).clone();
    config.folders.retain(|f| f != path);
    info!("Removed folder {path}");
    shared.update_config(config).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true }))
}

fn jobs(shared: &Arc<Shared>) -> Value {
    let jobs: Vec<engine::Job> = shared
        .jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .cloned()
        .collect();
    json!({ "jobs": jobs })
}

/// List folders for the folder picker (NAS volumes only).
fn browse(path: &str) -> Result<Value, String> {
    let root = browse_root();
    let path = path.trim_end_matches('/');
    let mut entries: Vec<Value> = Vec::new();
    let (current, parent): (String, Option<String>) = if path.is_empty() {
        // Shared folders on every volume.
        let mut volumes: Vec<PathBuf> = std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("volume") && n[6..].chars().all(|c| c.is_ascii_digit()))
            })
            .collect();
        volumes.sort();
        for volume in volumes {
            list_dirs(&volume, &mut entries, true);
        }
        (String::new(), None)
    } else {
        let p = Path::new(path);
        if !inside_volume(p) {
            return Err("Pick a folder on one of the NAS volumes".into());
        }
        list_dirs(p, &mut entries, false);
        let parent = p
            .parent()
            .filter(|pp| {
                pp.strip_prefix(&root)
                    .ok()
                    .is_some_and(|rel| rel.components().count() >= 2)
            })
            .map(|pp| pp.to_string_lossy().into_owned())
            .or(Some(String::new()));
        (path.to_string(), parent)
    };
    Ok(json!({ "path": current, "parent": parent, "entries": entries }))
}

fn browse_root() -> String {
    std::env::var("JENOVA_BROWSE_ROOT").unwrap_or_else(|_| "/".into())
}

/// Whether `path` is inside /volumeN (no `..` tricks).
fn inside_volume(path: &Path) -> bool {
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return false;
    }
    path.strip_prefix(browse_root())
        .ok()
        .and_then(|rel| rel.components().next())
        .and_then(|c| c.as_os_str().to_str())
        .is_some_and(|first| first.starts_with("volume") && first[6..].chars().all(|c| c.is_ascii_digit()) && first.len() > 6)
}

fn list_dirs(dir: &Path, out: &mut Vec<Value>, show_volume: bool) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !is_system_folder(n) && !n.starts_with('@') && !n.starts_with('#'))
        })
        .collect();
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    for path in dirs {
        let (readable, writable) = folder_access(&path);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let volume = show_volume
            .then(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
            .flatten();
        out.push(json!({
            "name": name,
            "path": path.to_string_lossy(),
            "volume": volume,
            "readable": readable,
            "writable": writable,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_decode() {
        let q = query("/x?r=browse&path=%2Fvolume1%2Fmy%20video&empty=");
        assert_eq!(q["r"], "browse");
        assert_eq!(q["path"], "/volume1/my video");
        assert_eq!(q["empty"], "");
    }

    #[test]
    fn browse_lists_shares_and_rejects_outside_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("volume1/video/Movies")).unwrap();
        std::fs::create_dir_all(dir.path().join("volume1/@appstore")).unwrap();
        std::fs::create_dir_all(dir.path().join("volume1/#recycle")).unwrap();
        std::fs::create_dir_all(dir.path().join("etc")).unwrap();
        std::env::set_var("JENOVA_BROWSE_ROOT", dir.path());
        let top = browse("").unwrap();
        let names: Vec<&str> = top["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["video"]);
        let inner = browse(&format!("{}/volume1/video", dir.path().display())).unwrap();
        assert_eq!(inner["entries"][0]["name"], "Movies");
        assert!(browse(&format!("{}/etc", dir.path().display())).is_err());
    }
}
