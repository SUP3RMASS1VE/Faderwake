use tauri::menu::{Menu, MenuBuilder, MenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Runtime};

fn submenu<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    items: &[(&str, &str, Option<&str>)],
) -> tauri::Result<Submenu<R>> {
    let mut builder = SubmenuBuilder::with_id(app, name, name);
    for &(id, label, key) in items {
        if id.is_empty() {
            builder = builder.separator();
        } else {
            let item = MenuItem::with_id(app, id, label, true, key)?;
            builder = builder.item(&item);
        }
    }
    builder.build()
}

fn studio_edit<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Submenu<R>> {
    submenu(
        app,
        "Edit",
        &[
            ("undo", "Undo", Some("CmdOrCtrl+Z")),
            ("redo", "Redo", Some("CmdOrCtrl+Shift+Z")),
            ("", "", None),
            ("cut", "Cut", Some("CmdOrCtrl+X")),
            ("copy", "Copy", Some("CmdOrCtrl+C")),
            ("paste", "Paste", Some("CmdOrCtrl+V")),
            ("select-all", "Select All", Some("CmdOrCtrl+A")),
        ],
    )
}

#[cfg(target_os = "macos")]
pub fn text_edit<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
    editing: bool,
) -> tauri::Result<()> {
    let current = if editing { "Edit" } else { "native-text-edit" };
    let items = menu.items()?;
    let Some(index) = items.iter().position(|item| item.id().as_ref() == current) else {
        return Ok(());
    };
    let replacement = if editing {
        SubmenuBuilder::with_id(app, "native-text-edit", "Edit")
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .build()?
    } else {
        studio_edit(app)?
    };
    menu.remove_at(index)?;
    menu.insert(&replacement, index)
}
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let file = submenu(
        app,
        "File",
        &[
            ("open", "Add Audio Files…", Some("CmdOrCtrl+O")),
            ("demo", "Open Demo Track", None),
            ("", "", None),
            ("export", "Export Audio…", Some("CmdOrCtrl+E")),
            ("export-all", "Export All…", Some("CmdOrCtrl+Shift+E")),
        ],
    )?;
    let edit = studio_edit(app)?;
    let audio = submenu(
        app,
        "Audio",
        &[
            ("trim", "Keep Selection", Some("CmdOrCtrl+T")),
            ("fade-in", "Fade In", None),
            ("fade-out", "Fade Out", None),
            ("silence", "Silence Selection", None),
            ("reverse", "Reverse", None),
            ("", "", None),
            ("zoom-in", "Zoom In", Some("CmdOrCtrl+Equal")),
            ("zoom-out", "Zoom Out", Some("CmdOrCtrl+Minus")),
            ("fit-track", "Fit Track", Some("CmdOrCtrl+0")),
            ("zoom-selection", "Zoom Selection", None),
        ],
    )?;
    let playback = submenu(
        app,
        "Playback",
        &[
            ("play", "Play / Pause", Some("CmdOrCtrl+Space")),
            ("stop", "Stop", Some("CmdOrCtrl+Period")),
            ("bypass", "Bypass Effects", Some("CmdOrCtrl+B")),
        ],
    )?;
    let settings = MenuItem::with_id(
        app,
        "settings",
        "Appearance…",
        true,
        Some("CmdOrCtrl+Comma"),
    )?;
    let menus = MenuBuilder::new(app);
    #[cfg(target_os = "macos")]
    let menus = {
        let app_menu = SubmenuBuilder::new(app, "Faderwake")
            .about(None)
            .separator()
            .item(&settings)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .separator()
            .quit()
            .build()?;
        menus.item(&app_menu)
    };
    let menus = menus.item(&file).item(&edit).item(&audio).item(&playback);
    #[cfg(not(target_os = "macos"))]
    let menus = {
        let prefs = SubmenuBuilder::new(app, "Settings")
            .item(&settings)
            .build()?;
        menus.item(&prefs)
    };
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .fullscreen()
        .close_window()
        .build()?;
    menus.item(&window).build()
}
