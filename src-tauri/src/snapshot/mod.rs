//! Developer-only, in-process snapshots of the packaged macOS webview.
use crate::{media::Tags, session::Studio};
use master_core::Settings;
use std::{
    env,
    ffi::{c_char, c_void, CStr, CString},
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, OnceLock,
    },
    time::Duration,
};
use tauri::{Context, Emitter, Manager, Webview, Wry};

const PALETTES: &[&str] = &[
    "studio", "system", "ocean", "amethyst", "forest", "rose", "amber",
];
const PAGES: &[&str] = &[
    "empty",
    "master",
    "editor",
    "metadata",
    "dynamics",
    "spectrum",
    "fade-in",
    "fade-out",
    "preferences",
    "about",
];
static PLAN: OnceLock<Plan> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);

struct Plan {
    dir: PathBuf,
    pages: Vec<String>,
    palette: String,
    sweep: bool,
    glass: bool,
}

pub fn active() -> bool {
    PLAN.get().is_some()
}

pub fn configure(mut context: Context<Wry>) -> Result<Context<Wry>, String> {
    let Some(dir) = env::var_os("APP_SNAPSHOT") else {
        return Ok(context);
    };
    if dir.is_empty() {
        return Err("APP_SNAPSHOT must name an output directory.".into());
    }
    let dir = PathBuf::from(dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    let palette = env::var("APP_SNAPSHOT_THEME")
        .unwrap_or_else(|_| "studio".into())
        .to_lowercase();
    if !PALETTES.contains(&palette.as_str()) {
        return Err(format!("Unknown color palette: {palette}"));
    }
    let filter = env::var("APP_SNAPSHOT_PAGES").ok();
    let mut pages = vec!["empty".to_owned()];
    let wanted: Vec<_> = filter
        .as_deref()
        .map(|s| {
            s.split([',', ' ', ';'])
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase)
                .collect()
        })
        .unwrap_or_else(|| PAGES.iter().map(|p| p.to_string()).collect());
    for name in wanted {
        let name = if name == "waveform" { "master" } else { &name };
        if !PAGES.contains(&name) {
            return Err(format!("Unknown snapshot page: {name}"));
        }
        if !pages.iter().any(|p| p == name) {
            pages.push(name.to_owned());
        }
        if name == "master" && filter.is_some() {
            for extra in ["dynamics", "spectrum"] {
                if !pages.iter().any(|p| p == extra) {
                    pages.push(extra.to_owned());
                }
            }
        }
    }
    let config = context.config_mut();
    config.identifier.push_str(".snapshot");
    let window = config
        .app
        .windows
        .iter_mut()
        .find(|w| w.label == "main")
        .ok_or("Missing main window")?;
    window.incognito = true;
    window.focus = false;
    window.visible = false;
    window.skip_taskbar = true;
    window.background_throttling = Some(tauri::utils::config::BackgroundThrottlingPolicy::Disabled);
    if let Ok(size) = env::var("APP_SNAPSHOT_SIZE") {
        let (w, h) = size
            .split_once(['x', 'X'])
            .ok_or("Use APP_SNAPSHOT_SIZE=WIDTHxHEIGHT")?;
        let dimension = |n: &str| {
            n.parse::<u32>()
                .ok()
                .filter(|n| (200..=8192).contains(n))
                .map(f64::from)
                .ok_or("Snapshot dimensions must be 200–8192 points.")
        };
        window.width = dimension(w)?;
        window.height = dimension(h)?;
        window.min_width = None;
        window.min_height = None;
    }
    PLAN.set(Plan {
        dir,
        pages,
        palette,
        sweep: filter.is_none(),
        glass: env::var("APP_SNAPSHOT_GLASS").is_ok_and(|value| value == "1"),
    })
    .map_err(|_| "Snapshot mode already initialized")?;
    Ok(context)
}

type Done = extern "C" fn(*mut c_void, *const c_char);
type WebKitCall = unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void, Done);
extern "C" {
    fn fw_snapshot_script(
        view: *mut c_void,
        input: *const c_char,
        context: *mut c_void,
        done: Done,
    );
    fn fw_snapshot_present(
        view: *mut c_void,
        input: *const c_char,
        context: *mut c_void,
        done: Done,
    );
    fn fw_snapshot_window(
        view: *mut c_void,
        input: *const c_char,
        context: *mut c_void,
        done: Done,
    );
    fn fw_snapshot_about(view: *mut c_void, input: *const c_char, context: *mut c_void, done: Done);
    fn fw_snapshot_capture(
        view: *mut c_void,
        input: *const c_char,
        context: *mut c_void,
        done: Done,
    );
}

fn webkit(webview: &Webview, function: WebKitCall, input: &str) -> Result<(), String> {
    type Reply = mpsc::Sender<Result<(), String>>;
    extern "C" fn complete(context: *mut c_void, error: *const c_char) {
        // The native completion is called once and its error string is valid until return.
        let sender = unsafe { Box::from_raw(context.cast::<Reply>()) };
        let result = if error.is_null() {
            Ok(())
        } else {
            Err(unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned())
        };
        let _ = sender.send(result);
    }
    let (tx, rx) = mpsc::channel();
    let input = CString::new(input).map_err(|e| e.to_string())?;
    webview
        .with_webview(move |platform| {
            // WebKit copies the NSString before returning and invokes complete asynchronously.
            unsafe {
                function(
                    platform.inner(),
                    input.as_ptr(),
                    Box::into_raw(Box::new(tx)).cast(),
                    complete,
                );
            }
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(90))
        .map_err(|e| format!("Waiting for the webview: {e}"))?
}

pub fn start(webview: Webview) {
    let Some(plan) = PLAN.get() else {
        return;
    };
    if webview.label() != "main" || STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let code = match capture_all(&webview, plan) {
            Ok(()) => {
                println!("SNAPSHOT {}", plan.dir.display());
                0
            }
            Err(e) => {
                eprintln!("Faderwake snapshots failed: {e}");
                1
            }
        };
        webview.app_handle().exit(code);
    });
}

fn capture_all(webview: &Webview, plan: &Plan) -> Result<(), String> {
    let app = webview.app_handle();
    let studio = app.state::<Studio>();
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let demo = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join("assets/Half Light.wav");
    if plan.glass {
        webkit(webview, fw_snapshot_present, "")?;
    }
    let mut track_id = None;
    let mut capture = |page: &str, palette: &str, prefix: &str| -> Result<(), String> {
        if page != "empty" && track_id.is_none() {
            let first = studio.add(vec![demo.clone()], true)?;
            let secondary = temp.path().join("After Hours (demo).wav");
            fs::copy(&demo, &secondary).map_err(|e| e.to_string())?;
            studio.add(vec![secondary], false)?;
            let second = studio
                .queue()
                .into_iter()
                .find(|row| row.id != first.id)
                .ok_or("Missing secondary preview track")?;
            studio.update_settings(
                second.id,
                Settings::preset("Warm Glue")?,
                Some("Warm Glue".into()),
                None,
                None,
                None,
            )?;
            studio.update_settings(
                first.id,
                Settings::preset("AI Cleanup")?,
                Some("AI Cleanup".into()),
                None,
                None,
                None,
            )?;
            studio.set_tags(
                first.id,
                Tags {
                    title: "Half Light".into(),
                    artist: "Faderwake".into(),
                    album: "Studio Sessions".into(),
                    genre: "Electronic".into(),
                    year: "2026".into(),
                    track: "1".into(),
                    ..Tags::default()
                },
            )?;
            studio.set_region(first.id, Some([2., 6.]), false, false, None)?;
            track_id = Some(first.id);
        }
        let edit = match page {
            "fade-in" => Some("fadeIn"),
            "fade-out" => Some("fadeOut"),
            _ => None,
        };
        if let (Some(operation), Some(id)) = (edit, track_id) {
            studio.set_region(id, Some([2., 6.]), false, false, None)?;
            studio.edit(id, operation, 1., 0.)?;
        }
        if page != "empty" {
            app.emit("queue-changed", ()).map_err(|e| e.to_string())?;
        }
        for mode in ["light", "dark"] {
            let args =
                serde_json::json!({ "page":page,"palette":palette,"mode":mode,"glass":plan.glass });
            let body = format!("const args = {args};\n{}", include_str!("driver.js"));
            webkit(webview, fw_snapshot_script, &body)
                .map_err(|e| format!("{page} ({mode}): {e}"))?;
            let path = plan.dir.join(format!("{prefix}-{mode}.png"));
            webkit(
                webview,
                if page == "about" {
                    fw_snapshot_about
                } else if plan.glass {
                    fw_snapshot_window
                } else {
                    fw_snapshot_capture
                },
                path.to_str().ok_or("Snapshot path must be UTF-8")?,
            )?;
            println!("  {}", path.display());
        }
        if let (Some(_), Some(id)) = (edit, track_id) {
            studio.edit(id, "undo", 1., 0.)?;
        }
        Ok(())
    };
    for page in &plan.pages {
        capture(page, &plan.palette, page)?;
    }
    if plan.sweep {
        for &palette in PALETTES {
            capture("master", palette, &format!("theme-{palette}"))?;
        }
        for page in ["editor", "metadata"] {
            capture(page, "ocean", &format!("theme-ocean-{page}"))?;
        }
        capture("preferences", "ocean", "theme-picker")?;
    }
    Ok(())
}
