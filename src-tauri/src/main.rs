#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tandem_core::{App, AppOptions, ErrorResponse, Request};
use tauri::{Emitter, Manager, State};

#[tauri::command]
async fn dispatch(
    state: State<'_, Arc<App>>,
    request: Request,
) -> Result<serde_json::Value, ErrorResponse> {
    state.dispatch(request).await.map_err(ErrorResponse::from)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Tandem could not start: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), tauri::Error> {
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![dispatch])
        .setup(|tauri_app| {
            let mut options = AppOptions::native()?;
            options.remote_artifacts.bundled_directory =
                Some(tauri_app.path().resource_dir()?.join("remote"));
            let current = std::env::current_exe()?;
            options.remote_artifacts.local_server = current.parent().map(|parent| {
                parent.join(if cfg!(windows) {
                    "tandem-server.exe"
                } else {
                    "tandem-server"
                })
            });
            let app = App::open(options)?;
            let mut events = app.subscribe();
            tauri_app.manage(app);
            let handle = tauri_app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            if let Err(error) = handle.emit("runtime-event", event) {
                                tracing::warn!(%error, "window event delivery failed");
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = handle.emit(
                                "runtime-event",
                                serde_json::json!({
                                    "sessionId": "",
                                    "kind": "error",
                                    "data": {
                                        "message": "UI events were interrupted. Reopen the conversation to restore its history.",
                                    },
                                }),
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())?;
    let closing = Arc::new(AtomicBool::new(false));
    application.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event
            && !closing.swap(true, Ordering::AcqRel)
        {
            api.prevent_exit();
            let app = handle.state::<Arc<App>>().inner().clone();
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = app.shutdown().await {
                    tracing::warn!(%error, "runtime shutdown incomplete");
                }
                handle.exit(0);
            });
        }
    });
    Ok(())
}
