use tray_icon::{
    menu::{Menu, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

pub const SHOW_ID: &str = "show-dialog";
pub const QUIT_ID: &str = "quit";

pub fn build() -> anyhow::Result<TrayIcon> {
    let icon = load_icon();

    let menu = Menu::new();
    let show_item = MenuItem::with_id(SHOW_ID, "Create event / Show dialog", true, None);
    let quit_item = MenuItem::with_id(QUIT_ID, "Exit", true, None);
    menu.append(&show_item)?;
    menu.append(&quit_item)?;

    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("DCN Notify")
        // Left click should just open the dialog, not also pop up the right-click
        // menu on top of it (this only affects Windows/macOS - unsupported on Linux).
        .with_menu_on_left_click(false)
        .build()?;

    Ok(tray)
}

fn load_icon() -> Icon {
    const ICON_BYTES: &[u8] = include_bytes!("../icon.png");
    let image = image::load_from_memory(ICON_BYTES)
        .expect("failed to decode embedded tray icon")
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).expect("failed to build tray icon")
}
