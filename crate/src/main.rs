//! Jenova subtitle watcher.

// Application modules
mod app;
mod background;
mod blades;
mod blades_layout;
mod config;
mod credentials;
mod data_structures;
mod downloader;
mod engine;
mod ff_layout;
mod green_layout;
mod ff_theme;
mod gui;
mod helper_functions;
mod logging;
mod notifications;
mod python_manager;
mod retry;
mod scan_history;
mod settings;
mod subtitle_utils;
mod tray;
mod updater;
mod watch;

// Re-export shared items
pub use config::*;
pub use data_structures::*;
pub use helper_functions::*;
pub use logging::*;
pub use python_manager::*;
pub use settings::*;
pub use subtitle_utils::*;

// Logging
use crate::logging::LOGGER;

// Third-party imports
use eframe::egui;

// Platform imports
#[cfg(windows)]
use windows::Win32::Foundation::POINT;
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// Initialize the application.
fn initialize_app() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging.
    if let Err(e) = setup_logging() {
        eprintln!("Failed to initialize logging: {}", e);
    }

    info!("Starting Jenova application");
    Ok(())
}

/// Load the application icon.
fn load_app_icon() -> Option<egui::IconData> {
    #[cfg(windows)]
    {
        if let Ok(image) = image::load_from_memory(include_bytes!("../resources/jenova_icon.ico")) {
            let rgba = image.to_rgba8();
            let size = [rgba.width(), rgba.height()];
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width: size[0],
                height: size[1],
            })
        } else {
            warn!("Failed to load application icon");
            None
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Use the macOS icon.
        if let Ok(image) =
            image::load_from_memory(include_bytes!("../resources/jenova_icon_512.png"))
        {
            let rgba = image.to_rgba8();
            let size = [rgba.width() as u32, rgba.height() as u32];
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width: size[0],
                height: size[1],
            })
        } else if let Ok(image) =
            image::load_from_memory(include_bytes!("../resources/jenova_icon_256.png"))
        {
            let rgba = image.to_rgba8();
            let size = [rgba.width() as u32, rgba.height() as u32];
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width: size[0],
                height: size[1],
            })
        } else if let Ok(image) =
            image::load_from_memory(include_bytes!("../resources/jenova_icon.ico"))
        {
            let rgba = image.to_rgba8();
            let size = [rgba.width() as u32, rgba.height() as u32];
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width: size[0],
                height: size[1],
            })
        } else {
            warn!("Failed to load application icon");
            None
        }
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        // Try PNG, then ICO.
        if let Ok(image) =
            image::load_from_memory(include_bytes!("../resources/jenova_icon_256.png"))
        {
            let rgba = image.to_rgba8();
            let size = [rgba.width() as u32, rgba.height() as u32];
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width: size[0],
                height: size[1],
            })
        } else if let Ok(image) =
            image::load_from_memory(include_bytes!("../resources/jenova_icon.ico"))
        {
            let rgba = image.to_rgba8();
            let size = [rgba.width() as u32, rgba.height() as u32];
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width: size[0],
                height: size[1],
            })
        } else {
            warn!("Failed to load application icon");
            None
        }
    }
}

/// Center the window on the active monitor.
fn calculate_window_position(window_size: [f32; 2]) -> egui::Pos2 {
    #[cfg(not(windows))]
    let _ = window_size;

    #[cfg(windows)]
    {
        unsafe {
            let mut point = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut point).is_ok() {
                let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(monitor, &mut info).as_bool() {
                    let work_left = info.rcWork.left;
                    let work_top = info.rcWork.top;
                    let work_width = (info.rcWork.right - info.rcWork.left) as f32;
                    let work_height = (info.rcWork.bottom - info.rcWork.top) as f32;
                    let x = work_left as f32 + (work_width - window_size[0]) / 2.0;
                    let y = work_top as f32 + (work_height - window_size[1]) / 2.0;
                    return egui::Pos2::new(x, y);
                }
            }
        }
    }

    egui::Pos2::new(100.0, 100.0)
}

/// Configure the application window.
fn configure_window(icon_data: Option<egui::IconData>, visible: bool) -> eframe::NativeOptions {
    let window_size = WINDOW_SIZE;
    let initial_position = calculate_window_position(window_size);

    let mut viewport_builder = egui::ViewportBuilder::default()
        .with_inner_size(window_size)
        .with_position(initial_position)
        .with_decorations(true)
        .with_resizable(true)
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_visible(visible);

    if let Some(icon) = icon_data {
        viewport_builder = viewport_builder.with_icon(icon);
    }

    eframe::NativeOptions {
        viewport: viewport_builder,
        persist_window: true,
        ..Default::default()
    }
}

/// Apply the theme.
fn configure_visuals(ctx: &egui::Context) {
    let saved = crate::settings::Settings::load();
    ff_theme::set_theme(ff_theme::Theme::from_key(&saved.theme));
    ff_theme::apply_style(ctx);
}

/// Configure the embedded font.
fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../resources/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "Nunito".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../resources/fonts/Nunito-700.ttf"
        ))),
    );
    if let Some(proportional) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        // Nunito covers Latin; Inter and the defaults cover everything else.
        proportional.insert(0, "Inter".to_owned());
        proportional.insert(0, "Nunito".to_owned());
    }
    if let Some(monospace) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        monospace.push("Inter".to_owned());
    }
    // Blocky display face for the green dashboard theme's headings and tabs.
    fonts.font_data.insert(
        "Orbitron".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../resources/fonts/Orbitron-800.ttf"
        ))),
    );
    fonts.families.insert(
        egui::FontFamily::Name("display".into()),
        vec!["Orbitron".to_owned(), "Nunito".to_owned(), "Inter".to_owned()],
    );
    ctx.set_fonts(fonts);
}

/// Cleanup.
fn cleanup_on_exit() {
    // Shutdown logger.
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(logger) = guard.take() {
            logger.shutdown();
        }
    }
}

fn main() {
    // Keep every file Jenova writes in %APPDATA%\\Jenova.
    #[cfg(windows)]
    helper_functions::Utils::migrate_legacy_files();

    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    // Started with Windows: stay hidden in the tray.
    let start_hidden = tray::available()
        && (has(background::TRAY_ARG) || has(background::LEGACY_BACKGROUND_ARG));
    let after_update = has(updater::AFTER_UPDATE_ARG);

    if let Err(e) = initialize_app() {
        eprintln!("Failed to initialize application: {}", e);
        return;
    }

    // Only one Jenova at a time: a second launch just shows the first one.
    // After an update, wait for the old copy to finish closing.
    let wait = if after_update {
        std::time::Duration::from_secs(20)
    } else {
        std::time::Duration::ZERO
    };
    let Some(_instance_lock) = background::hold_instance_lock(wait) else {
        if tray::signal_running_copy() {
            info!("Jenova is already running; showing it");
        } else {
            warn!("Jenova is already running");
        }
        cleanup_on_exit();
        return;
    };
    background::stop_legacy_background();

    if start_hidden {
        tray::set_started_hidden();
    }
    let icon_data = load_app_icon();
    let native_options = configure_window(icon_data, !start_hidden);
    info!(
        "Initializing GUI with window size: {}x{}{}",
        WINDOW_SIZE[0],
        WINDOW_SIZE[1],
        if start_hidden { " (hidden in the tray)" } else { "" }
    );

    let result = eframe::run_native(
        "Jenova",
        native_options,
        Box::new(|cc| {
            configure_visuals(&cc.egui_ctx);
            configure_fonts(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let app = SubtitleDownloader::new(&cc.egui_ctx);
            let engine = std::sync::Arc::clone(&app.engine);
            tray::start(
                cc.egui_ctx.clone(),
                Box::new(move || {
                    engine.shutdown();
                    cleanup_on_exit();
                    std::process::exit(0);
                }),
            );
            info!("GUI initialized successfully");
            Ok(Box::new(app))
        }),
    );

    cleanup_on_exit();

    if let Err(error) = result {
        eprintln!("Failed to start eframe: {error}");
    }
}
