use serde::{Deserialize, Serialize};
use tauri::WebviewWindow;

#[derive(Deserialize)]
#[repr(C)]
pub struct Region {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    supported: bool,
    reduce_transparency: bool,
    active: bool,
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use tauri::{AppHandle, Emitter, Manager};
    static APP: OnceLock<AppHandle> = OnceLock::new();
    extern "C" {
        fn fw_glass_status(view: *mut c_void) -> u32;
        fn fw_glass_set(
            view: *mut c_void,
            enabled: bool,
            regions: *const Region,
            count: usize,
        ) -> u32;
        fn fw_glass_observe(changed: extern "C" fn());
    }

    extern "C" fn changed() {
        if let Some(app) = APP.get() {
            let _ = app.emit("glass-accessibility-changed", ());
        }
    }
    pub async fn apply(
        window: WebviewWindow,
        change: Option<(bool, Vec<Region>)>,
    ) -> Result<Availability, String> {
        let app = window.app_handle().clone();
        let (tx, rx) = tauri::async_runtime::channel(1);
        window
            .with_webview(move |platform| {
                APP.get_or_init(|| {
                    unsafe {
                        fw_glass_observe(changed);
                    }
                    app
                });
                // Tauri dispatches this closure to the AppKit main thread. The synchronous
                // bridge consumes the geometry before the owned vector is dropped.
                let flags = unsafe {
                    match change {
                        Some((enabled, regions)) => {
                            fw_glass_set(platform.inner(), enabled, regions.as_ptr(), regions.len())
                        }
                        None => fw_glass_status(platform.inner()),
                    }
                };
                let _ = tx.try_send(Availability {
                    supported: flags & 1 != 0,
                    reduce_transparency: flags & 2 != 0,
                    active: flags & 4 != 0,
                });
            })
            .map_err(|e| e.to_string())?;
        let mut rx = rx;
        rx.recv()
            .await
            .ok_or_else(|| "Window closed before setting its material.".into())
    }
}

#[tauri::command]
pub async fn get_glass_availability(window: WebviewWindow) -> Result<Availability, String> {
    #[cfg(target_os = "macos")]
    {
        mac::apply(window, None).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(Availability::default())
    }
}

#[tauri::command]
pub async fn set_liquid_glass(
    window: WebviewWindow,
    enabled: bool,
    regions: Vec<Region>,
) -> Result<Availability, String> {
    if regions.len() > 16
        || regions.iter().any(|r| {
            [r.x, r.y, r.width, r.height, r.radius]
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 100000.)
                || r.width < 0.
                || r.height < 0.
                || r.radius < 0.
        })
    {
        return Err("Invalid window material geometry.".into());
    }
    #[cfg(target_os = "macos")]
    {
        mac::apply(window, Some((enabled, regions))).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, enabled);
        Ok(Availability::default())
    }
}
