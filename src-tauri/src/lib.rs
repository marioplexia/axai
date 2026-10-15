// Ax Ai desktop shell (Tauri v2): loads the hosted Ax Ai platform and exposes
// only window.axDesktop.capture/act to it, same contract as the Electron shell.
use base64::{engine::general_purpose::STANDARD, Engine};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde::{Deserialize, Serialize};
use std::{io::Cursor, thread::sleep, time::Duration};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const APP_URL: &str = "https://id-preview--f65a6e6c-8153-4c1a-b2de-225543890fc5.lovable.app";
const CAPTURE_WIDTH: u32 = 1280;

const BRIDGE: &str = r#"
(function () {
  var inv = function (c, a) { return window.__TAURI_INTERNALS__.invoke(c, a || {}); };
  var p = navigator.userAgent.indexOf('Windows') >= 0 ? 'win32' : 'darwin';
  window.axDesktop = Object.freeze({
    platform: p,
    capture: function () { return inv('ax_capture'); },
    act: function (action) { return inv('ax_act', { action: action }); },
    minimize: function () { return inv('ax_minimize'); },
    restore: function () { return inv('ax_restore'); }
  });
})();
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Captura { image: String, width: u32, height: u32, screen_width: u32, screen_height: u32 }

#[derive(Deserialize)]
struct Accion { r#type: String, x: Option<f64>, y: Option<f64>, text: Option<String>, key: Option<String>, amount: Option<i32> }

fn with_hidden<T>(app: &tauri::AppHandle, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let win = app.get_webview_window("main");
    let minimized = win.as_ref().and_then(|w| w.is_minimized().ok()).unwrap_or(false);
    let visible = !minimized && win.as_ref().and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if visible { if let Some(w) = &win { let _ = w.hide(); } sleep(Duration::from_millis(350)); }
    let r = f();
    if visible { if let Some(w) = &win { let _ = w.show(); } }
    r
}

#[tauri::command]
async fn ax_capture(app: tauri::AppHandle) -> Result<Captura, String> {
    with_hidden(&app, || {
        let mon = xcap::Monitor::all().map_err(|e| e.to_string())?
            .into_iter().find(|m| m.is_primary()).ok_or("No se encontró la pantalla principal.")?;
        let scale = mon.scale_factor() as f64;
        let img = mon.capture_image().map_err(|e| e.to_string())?;
        let (sw, sh) = ((img.width() as f64 / scale) as u32, (img.height() as f64 / scale) as u32);
        let h = CAPTURE_WIDTH * img.height() / img.width();
        let small = image::DynamicImage::ImageRgba8(img)
            .resize_exact(CAPTURE_WIDTH, h, image::imageops::FilterType::Triangle).to_rgb8();
        let mut buf = Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 70)
            .encode_image(&small).map_err(|e| e.to_string())?;
        Ok(Captura { image: STANDARD.encode(buf.into_inner()), width: CAPTURE_WIDTH, height: h, screen_width: sw, screen_height: sh })
    })
}

#[tauri::command]
async fn ax_act(app: tauri::AppHandle, action: Accion) -> Result<(), String> {
    let allowed = ["click", "doubleClick", "type", "key", "scroll", "openUrl"];
    if !allowed.contains(&action.r#type.as_str()) { return Err("Acción no permitida.".into()); }
    if action.r#type == "openUrl" {
        let url = action.text.unwrap_or_default();
        if !(url.starts_with("https://") || url.starts_with("http://")) { return Err("Solo se pueden abrir direcciones web.".into()); }
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg(&url).spawn();
        #[cfg(target_os = "windows")]
        let r = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let r = std::process::Command::new("xdg-open").arg(&url).spawn();
        r.map_err(|e| e.to_string())?;
        sleep(Duration::from_millis(1500));
        return Ok(());
    }
    with_hidden(&app, || {
        let mut en = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
        // Safety stop: mouse parked in the top-left corner blocks every action.
        let (cx, cy) = en.location().map_err(|e| e.to_string())?;
        if cx <= 2 && cy <= 2 { return Err("Freno de emergencia: mouse en la esquina superior izquierda.".into()); }
        let scale = if cfg!(target_os = "windows") {
            xcap::Monitor::all().ok().and_then(|m| m.into_iter().find(|m| m.is_primary())).map(|m| m.scale_factor() as f64).unwrap_or(1.0)
        } else { 1.0 };
        let e = |r: enigo::InputResult<()>| r.map_err(|e| e.to_string());
        match action.r#type.as_str() {
            "click" | "doubleClick" => {
                let x = (action.x.unwrap_or(0.0) * scale).round() as i32;
                let y = (action.y.unwrap_or(0.0) * scale).round() as i32;
                e(en.move_mouse(x, y, Coordinate::Abs))?;
                sleep(Duration::from_millis(80));
                let n = if action.r#type == "doubleClick" { 2 } else { 1 };
                for _ in 0..n { e(en.button(Button::Left, Direction::Click))?; sleep(Duration::from_millis(90)); }
            }
            "type" => e(en.text(action.text.as_deref().unwrap_or("")))?,
            "key" => {
                let k = match action.key.as_deref().unwrap_or("enter") {
                    "tab" => Key::Tab, "escape" => Key::Escape, "backspace" => Key::Backspace,
                    "up" => Key::UpArrow, "down" => Key::DownArrow, "pagedown" => Key::PageDown,
                    "pageup" => Key::PageUp, _ => Key::Return,
                };
                e(en.key(k, Direction::Click))?
            }
            "scroll" => e(en.scroll(action.amount.unwrap_or(3), Axis::Vertical))?,
            _ => {}
        }
        Ok(())
    })
}

#[tauri::command]
async fn ax_minimize(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") { w.minimize().map_err(|e| e.to_string())?; }
    sleep(Duration::from_millis(400));
    Ok(())
}

#[tauri::command]
async fn ax_restore(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") { let _ = w.unminimize(); let _ = w.show(); let _ = w.set_focus(); }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ax_capture, ax_act, ax_minimize, ax_restore])
        .setup(|app| {
            let url = std::env::var("AX_AI_URL").unwrap_or_else(|_| APP_URL.to_string());
            let target = format!("{}/escritorio", url.trim_end_matches('/'));
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(target.parse().unwrap()))
                .title("Ax Ai")
                .inner_size(1360.0, 880.0)
                .initialization_script(BRIDGE)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error al iniciar Ax Ai");
}
