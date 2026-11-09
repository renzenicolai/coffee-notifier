#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod crypto;
mod event;
mod image_util;
mod ipc;
mod mqtt;
mod notifications;
mod notifier;
mod state;
mod tray;

use std::net::TcpListener;
use std::process::{Child, Command};
use std::sync::Arc;

#[cfg(not(windows))]
use crossbeam_channel::select;
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButton, TrayIconEvent};

use app::DcnApp;
use state::SharedState;

const PORT_ENV: &str = "DCN_NOTIFY_PORT";
const TOKEN_ENV: &str = "DCN_NOTIFY_TOKEN";

fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--dialog") {
        run_dialog()
    } else {
        run_daemon()
    }
}

/// The long-running tray + MQTT process. Never creates a GUI window itself -
/// on Wayland a window can't be hidden and later shown again (and winit only
/// allows one event loop per process, so it can't be torn down and recreated
/// either). Instead the dialog is a completely separate, short-lived process
/// started fresh for every click, which sidesteps both limitations and
/// behaves the same way on every platform.
fn run_daemon() -> anyhow::Result<()> {
    // rumqttc's websocket/TLS transport needs a process-wide default rustls
    // crypto provider installed before the first connection attempt.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let config = config::NotifierConfig::load();
    let shared = Arc::new(SharedState::new(config.clone()));
    let (outbox_tx, outbox_rx) = tokio::sync::mpsc::unbounded_channel();
    let (outbox_notifier_tx, outbox_notifier_rx) = tokio::sync::mpsc::unbounded_channel();
    let (config_tx, config_rx) = tokio::sync::watch::channel(config);

    let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
    rt.spawn(mqtt::run(shared.clone(), outbox_rx, config_rx.clone()));
    rt.spawn(mqtt::run_notifier(shared.clone(), outbox_notifier_rx, config_rx));
    // Keep the tokio runtime (and its worker threads) alive for the rest of
    // the process - nothing else owns it from here on.
    std::mem::forget(rt);

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().to_string();
    ipc::serve(listener, token.clone(), shared, outbox_tx, outbox_notifier_tx, config_tx);

    let _tray = tray::build().expect("failed to create tray icon");
    run_tray_event_loop(port, token)
}

fn handle_menu_event(event: &MenuEvent, dialog: &mut Option<Child>, port: u16, token: &str) {
    if event.id() == tray::SHOW_ID {
        spawn_dialog(dialog, port, token);
    } else if event.id() == tray::QUIT_ID {
        if let Some(mut child) = dialog.take() {
            let _ = child.kill();
        }
        std::process::exit(0);
    }
}

/// The KSNI backend used on Linux manages its own worker thread, so polling
/// the event channels directly is enough. Everywhere else (notably Windows)
/// tray-icon needs an actual native message loop running on this thread
/// before it will ever deliver a click or menu selection.
#[cfg(not(windows))]
fn run_tray_event_loop(port: u16, token: String) -> ! {
    let mut dialog: Option<Child> = None;
    loop {
        select! {
            recv(TrayIconEvent::receiver()) -> msg => {
                if let Ok(TrayIconEvent::Click { button: MouseButton::Left, .. }) = msg {
                    spawn_dialog(&mut dialog, port, &token);
                }
            }
            recv(MenuEvent::receiver()) -> msg => {
                if let Ok(event) = msg {
                    handle_menu_event(&event, &mut dialog, port, &token);
                }
            }
        }
    }
}

#[cfg(windows)]
fn run_tray_event_loop(port: u16, token: String) -> ! {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    let mut dialog: Option<Child> = None;
    let mut msg = MSG::default();
    loop {
        // Blocks until a message arrives for this thread. tray-icon's
        // Windows backend dispatches tray clicks and menu selections from
        // inside its window procedure, which only runs via this pump.
        let ret = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if ret <= 0 {
            std::process::exit(0);
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                spawn_dialog(&mut dialog, port, &token);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            handle_menu_event(&event, &mut dialog, port, &token);
        }
    }
}

fn spawn_dialog(dialog: &mut Option<Child>, port: u16, token: &str) {
    if let Some(child) = dialog.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            // Already open - just leave it be.
            return;
        }
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("[daemon] failed to resolve own executable: {e}");
            return;
        }
    };

    match Command::new(exe)
        .arg("--dialog")
        .env(PORT_ENV, port.to_string())
        .env(TOKEN_ENV, token)
        .spawn()
    {
        Ok(child) => *dialog = Some(child),
        Err(e) => eprintln!("[daemon] failed to open dialog: {e}"),
    }
}

/// The short-lived dialog process, spawned fresh by the daemon on every tray
/// click. Talks to the daemon over a loopback TCP connection for publishing
/// new events and reading the live ongoing-events list.
fn run_dialog() -> anyhow::Result<()> {
    let port: u16 = std::env::var(PORT_ENV)
        .expect("DCN_NOTIFY_PORT not set - this binary is meant to be launched by dcn-notify itself")
        .parse()
        .expect("DCN_NOTIFY_PORT was not a valid port number");
    let token = std::env::var(TOKEN_ENV)
        .expect("DCN_NOTIFY_TOKEN not set - this binary is meant to be launched by dcn-notify itself");

    let ipc = ipc::IpcClient::connect(port, token)?;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 640.0])
            .with_title("DCN Notify"),
        ..Default::default()
    };

    eframe::run_native(
        "DCN Notify",
        native_options,
        Box::new(move |_cc| Ok(Box::new(DcnApp::new(ipc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}
