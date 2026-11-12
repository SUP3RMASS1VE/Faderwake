#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod glass;
mod media;
mod menus;
mod resample;
mod selftest;
mod session;
#[cfg(target_os = "macos")]
mod snapshot;
mod transport;

use master_core::{Band, Settings, Spectrogram};
use media::{Output, Tags};
use session::{QueueRow, Status, Studio, Track};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

async fn background<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn open_demo(app: AppHandle, studio: State<'_, Studio>) -> Result<Track, String> {
    let studio = studio.inner().clone();
    let path = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join("assets/Half Light.wav");
    background(move || studio.add(vec![path], true)).await
}
#[tauri::command]
async fn open_audio(
    window: WebviewWindow,
    studio: State<'_, Studio>,
) -> Result<Option<Track>, String> {
    let studio = studio.inner().clone();
    background(move || {
        let files = window
            .dialog()
            .file()
            .set_parent(&window)
            .set_title("Add audio files")
            .add_filter(
                "Audio files",
                &[
                    "wav", "wave", "flac", "m4a", "alac", "aac", "mp3", "aif", "aiff", "ogg",
                    "opus", "caf", "mp4", "wma", "aifc",
                ],
            )
            .blocking_pick_files();
        let Some(files) = files else { return Ok(None) };
        let paths = files
            .into_iter()
            .map(|p| p.into_path().map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        studio
            .add(paths, false)
            .inspect_err(|_| {
                let _ = window.emit("queue-changed", ());
            })
            .map(Some)
    })
    .await
}
#[tauri::command]
fn get_track(studio: State<'_, Studio>) -> Option<Track> {
    studio.track()
}
#[tauri::command]
fn get_queue(studio: State<'_, Studio>) -> Vec<QueueRow> {
    studio.queue()
}
#[tauri::command]
async fn select_track(
    app: AppHandle,
    studio: State<'_, Studio>,
    track_id: u64,
) -> Result<Track, String> {
    let s = studio.inner().clone();
    background(move || {
        s.select(track_id).inspect_err(|_| {
            let _ = app.emit("queue-changed", ());
        })
    })
    .await
}
#[tauri::command]
async fn remove_track(studio: State<'_, Studio>, track_id: u64) -> Result<Option<Track>, String> {
    let s = studio.inner().clone();
    background(move || s.remove(track_id)).await
}
#[tauri::command]
fn get_preset(preset: String) -> Result<[Band; 10], String> {
    Ok(Settings::preset(&preset)?.bands)
}
#[tauri::command]
fn get_preset_settings(preset: String) -> Result<Settings, String> {
    Settings::preset(&preset)
}
#[tauri::command]
fn update_master(
    studio: State<'_, Studio>,
    track_id: u64,
    edit_id: Option<u64>,
    bands: [Band; 10],
    preset: String,
) -> Result<(), String> {
    let mut settings = Settings::preset(&preset)?;
    settings.bands = bands;
    studio.update_settings(track_id, settings, Some(preset), edit_id, None, None)
}
#[tauri::command]
fn update_settings(
    studio: State<'_, Studio>,
    track_id: u64,
    settings: Settings,
    preset: Option<String>,
    edit_id: Option<u64>,
    gesture_id: Option<u64>,
    output: Option<Output>,
) -> Result<(), String> {
    studio.update_settings(track_id, settings, preset, edit_id, gesture_id, output)
}
#[tauri::command]
fn set_output(studio: State<'_, Studio>, track_id: u64, output: Output) -> Result<(), String> {
    studio.set_output(track_id, output)
}
#[tauri::command]
fn set_tags(studio: State<'_, Studio>, track_id: u64, tags: Tags) -> Result<(), String> {
    studio.set_tags(track_id, tags)
}
#[tauri::command]
fn apply_to_all(studio: State<'_, Studio>, track_id: u64, what: String) -> Result<(), String> {
    studio.apply_all(track_id, &what)
}
#[tauri::command]
async fn workspace_history(studio: State<'_, Studio>, redo: bool) -> Result<Option<Track>, String> {
    let s = studio.inner().clone();
    background(move || s.history(redo)).await
}
#[tauri::command]
async fn edit_audio(
    studio: State<'_, Studio>,
    track_id: u64,
    operation: String,
    gain_db: Option<f64>,
    fade_seconds: Option<f64>,
) -> Result<Track, String> {
    let s = studio.inner().clone();
    background(move || {
        s.edit(
            track_id,
            &operation,
            fade_seconds.unwrap_or(1.),
            gain_db.unwrap_or(0.),
        )
    })
    .await
}
#[tauri::command]
fn set_region(
    studio: State<'_, Studio>,
    track_id: u64,
    selection: Option<[f64; 2]>,
    snap: bool,
    looping: bool,
    snap_edge: Option<String>,
) -> Result<Option<[f64; 2]>, String> {
    studio.set_region(track_id, selection, snap, looping, snap_edge.as_deref())
}
#[tauri::command]
async fn editor_waveform(
    studio: State<'_, Studio>,
    track_id: u64,
    start: f64,
    end: f64,
    bins: usize,
    original: bool,
) -> Result<Vec<Vec<[f32; 2]>>, String> {
    let s = studio.inner().clone();
    background(move || s.editor_waveform(track_id, start, end, bins, original)).await
}
#[tauri::command]
fn get_overview(
    studio: State<'_, Studio>,
    track_id: u64,
    original: bool,
) -> Result<Vec<f32>, String> {
    studio.overview(track_id, original)
}
#[tauri::command]
async fn spectrogram(
    studio: State<'_, Studio>,
    track_id: u64,
    original: bool,
) -> Result<Spectrogram, String> {
    let s = studio.inner().clone();
    background(move || s.spectrogram(track_id, original)).await
}
#[tauri::command]
async fn set_playing(studio: State<'_, Studio>, playing: bool) -> Result<(), String> {
    let t = studio.transport.clone();
    background(move || t.set_playing(playing)).await
}
#[tauri::command]
fn seek(studio: State<'_, Studio>, seconds: f64) -> Result<(), String> {
    studio.transport.seek(seconds)
}
#[tauri::command]
fn set_audition(studio: State<'_, Studio>, original: bool, matched: bool) {
    studio.transport.set_audition(original, matched)
}
#[tauri::command]
async fn get_status(studio: State<'_, Studio>) -> Result<Status, String> {
    let s = studio.inner().clone();
    background(move || Ok(s.status())).await
}
#[tauri::command]
async fn export_audio(
    window: WebviewWindow,
    studio: State<'_, Studio>,
    all: bool,
) -> Result<Option<String>, String> {
    let studio = studio.inner().clone();
    let plan = studio.plan_export(all)?;
    background(move || {
        let dialog = window.dialog().file().set_parent(&window);
        let picked = if all {
            dialog
                .set_title("Export all tracks to folder")
                .blocking_pick_folder()
        } else {
            dialog
                .set_title("Export mastered audio")
                .set_file_name(plan.suggested_name())
                .add_filter("Mastered audio", &[plan.extension()])
                .blocking_save_file()
        };
        let Some(file) = picked else { return Ok(None) };
        studio
            .export_to(plan, file.into_path().map_err(|e| e.to_string())?, all)
            .map(Some)
    })
    .await
}
#[tauri::command]
async fn export_wav(
    window: WebviewWindow,
    studio: State<'_, Studio>,
) -> Result<Option<String>, String> {
    export_audio(window, studio, false).await
}
#[tauri::command]
fn cancel_export(studio: State<'_, Studio>) {
    studio.cancel_export();
}
#[tauri::command]
fn reveal_track(app: AppHandle, studio: State<'_, Studio>, track_id: u64) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(studio.path(track_id)?)
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn reveal_export(app: AppHandle, studio: State<'_, Studio>) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(studio.last_export()?)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_menu_state(
    app: AppHandle,
    enabled: std::collections::BTreeMap<String, bool>,
    editing_text: Option<bool>,
) -> Result<(), String> {
    use tauri::menu::MenuItemKind;
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    #[cfg(target_os = "macos")]
    menus::text_edit(&app, &menu, editing_text.unwrap_or(false)).map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "macos"))]
    let _ = editing_text;
    for top in menu.items().map_err(|e| e.to_string())? {
        if let MenuItemKind::Submenu(sub) = top {
            for item in sub.items().map_err(|e| e.to_string())? {
                if let MenuItemKind::MenuItem(item) = item {
                    if let Some(state) = enabled.get(item.id().as_ref()) {
                        item.set_enabled(*state).map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }
    Ok(())
}
fn open_from_os(app: &AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let handle = app.clone();
    let studio = app.state::<Studio>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = studio.add(paths, false);
        let _ = handle.emit("queue-changed", ());
        if let Err(error) = result {
            let _ = handle.emit("studio-error", error);
        }
    });
}

fn main() {
    let context = tauri::generate_context!();
    if std::env::args_os().any(|arg| arg == "--selftest") {
        let result =
            tauri::utils::platform::resource_dir(context.package_info(), &tauri::Env::default())
                .map_err(|error| error.to_string())
                .and_then(|dir| selftest::run(&dir));
        if let Err(error) = result {
            eprintln!("Faderwake self-test failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(target_os = "macos")]
    let context = snapshot::configure(context).unwrap_or_else(|error| {
        eprintln!("Faderwake snapshots: {error}");
        std::process::exit(1);
    });
    let builder = tauri::Builder::default();
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(
        |app, arguments, directory| {
            let directory = PathBuf::from(directory);
            let files = arguments
                .into_iter()
                .skip(1)
                .map(|name| directory.join(name))
                .filter(|file| file.is_file())
                .collect();
            open_from_os(app, files);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        },
    ));
    #[cfg(target_os = "macos")]
    let builder = builder.on_page_load(|webview, payload| {
        if payload.event() == tauri::webview::PageLoadEvent::Finished {
            snapshot::start(webview.clone());
        }
    });
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Studio::new())
        .menu(menus::build)
        .on_menu_event(|app, event| {
            let _ = app.emit("studio-action", event.id().as_ref());
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                open_from_os(window.app_handle(), paths.clone());
            }
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            if snapshot::active() {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                return Ok(());
            }
            let studio = app.state::<Studio>();
            studio.restore_defaults(app.path().app_config_dir()?.join("mastering-defaults.json"));
            #[cfg(not(target_os = "macos"))]
            open_from_os(
                app.handle(),
                std::env::args_os()
                    .skip(1)
                    .map(PathBuf::from)
                    .filter(|p| p.is_file())
                    .collect(),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            glass::get_glass_availability,
            glass::set_liquid_glass,
            open_demo,
            open_audio,
            get_track,
            get_queue,
            select_track,
            remove_track,
            get_preset,
            get_preset_settings,
            update_master,
            update_settings,
            set_output,
            set_tags,
            apply_to_all,
            workspace_history,
            edit_audio,
            set_region,
            editor_waveform,
            get_overview,
            spectrogram,
            set_playing,
            seek,
            set_audition,
            get_status,
            export_audio,
            export_wav,
            cancel_export,
            reveal_track,
            reveal_export,
            set_menu_state
        ])
        .build(context)
        .expect("Could not launch Faderwake");
    app.run(|_app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = event {
            if snapshot::active() {
                return;
            }
            open_from_os(
                _app,
                urls.into_iter()
                    .filter_map(|url| url.to_file_path().ok())
                    .collect(),
            );
        }
        #[cfg(not(target_os = "macos"))]
        let _ = event;
    });
}
