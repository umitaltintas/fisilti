//! Keyboard injection on Wayland through the XDG RemoteDesktop portal.
//!
//! Wayland gives apps no way to type into other windows. The usual workarounds
//! are external tools: `wtype` (unsupported by KWin), `kwtype`, `dotool` or
//! `ydotool` (needs a root-ish daemon and `/dev/uinput`). Most desktops ship
//! none of them, so dictation produced text and then silently failed to insert
//! it. The RemoteDesktop portal is the sanctioned route and is implemented by
//! KDE Plasma and GNOME: the desktop asks the user once ("allow Fısıltı to
//! control the keyboard?"), and the restore token we store keeps that grant
//! across restarts.
//!
//! We only ever send the paste chord (Ctrl+V and friends); the text itself goes
//! through the clipboard, which handles every script and layout correctly,
//! unlike synthesizing one keysym per character.

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use ashpd::desktop::remote_desktop::{
    DeviceType, KeyState, NotifyKeyboardKeysymOptions, RemoteDesktop, SelectDevicesOptions,
    StartOptions,
};
use ashpd::desktop::{PersistMode, Session};
use log::{info, warn};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

use crate::settings::PasteMethod;

// X keysyms.
const XK_CONTROL_L: i32 = 0xffe3;
const XK_SHIFT_L: i32 = 0xffe1;
const XK_INSERT: i32 = 0xff63;
const XK_V: i32 = 0x0076;

/// How long a caller waits for the chord, including a first-run permission
/// dialog the user has to answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

type Reply = std_mpsc::SyncSender<Result<(), String>>;

struct Request {
    keys: Vec<i32>,
    reply: Reply,
}

static WORKER: OnceLock<Mutex<Option<mpsc::UnboundedSender<Request>>>> = OnceLock::new();

/// Whether this session can use the portal at all (Wayland only; X11 has
/// working tools).
pub fn is_supported() -> bool {
    crate::utils::is_wayland()
}

fn chord(method: &PasteMethod) -> Option<Vec<i32>> {
    match method {
        PasteMethod::CtrlV | PasteMethod::Direct => Some(vec![XK_CONTROL_L, XK_V]),
        PasteMethod::CtrlShiftV => Some(vec![XK_CONTROL_L, XK_SHIFT_L, XK_V]),
        PasteMethod::ShiftInsert => Some(vec![XK_SHIFT_L, XK_INSERT]),
        _ => None,
    }
}

/// Send the paste chord for `method` to the focused window. Blocks until the
/// portal accepted the key events (or failed). The first call may show the
/// desktop's permission dialog.
pub fn send_paste(app: &AppHandle, method: &PasteMethod) -> Result<(), String> {
    let keys = chord(method).ok_or("No key chord for this paste method")?;
    let (reply, rx) = std_mpsc::sync_channel(1);
    let request = Request { keys, reply };

    let slot = WORKER.get_or_init(|| Mutex::new(None));
    {
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        let sent = match guard.as_ref() {
            Some(tx) => tx.send(request).map_err(|e| e.0),
            None => Err(request),
        };
        if let Err(request) = sent {
            let (tx, rx) = mpsc::unbounded_channel();
            tauri::async_runtime::spawn(worker(token_path(app), rx));
            let _ = tx.send(request);
            *guard = Some(tx);
        }
    }

    rx.recv_timeout(REQUEST_TIMEOUT)
        .map_err(|_| "Timed out waiting for the remote desktop portal".to_string())?
}

fn token_path(app: &AppHandle) -> Option<PathBuf> {
    crate::portable::app_data_dir(app)
        .ok()
        .or_else(|| app.path().app_data_dir().ok())
        .map(|dir| dir.join("remote_desktop_token"))
}

struct Active {
    portal: RemoteDesktop,
    session: Session<RemoteDesktop>,
}

async fn worker(token_path: Option<PathBuf>, mut rx: mpsc::UnboundedReceiver<Request>) {
    let mut active: Option<Active> = None;
    while let Some(request) = rx.recv().await {
        let mut result = Err(String::new());
        // One retry with a fresh session: the user may have revoked the grant
        // or the portal restarted since the session was opened.
        for _ in 0..2 {
            if active.is_none() {
                match start_session(token_path.as_ref()).await {
                    Ok(a) => active = Some(a),
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            let Some(a) = active.as_ref() else { break };
            result = press_chord(a, &request.keys).await;
            if result.is_ok() {
                break;
            }
            warn!(
                "remote desktop: key events rejected ({}); reopening the session",
                result.as_ref().err().map(String::as_str).unwrap_or("")
            );
            if let Some(a) = active.take() {
                let _ = a.session.close().await;
            }
        }
        let _ = request.reply.send(result);
    }
}

async fn start_session(token_path: Option<&PathBuf>) -> Result<Active, String> {
    let saved = token_path.and_then(|p| std::fs::read_to_string(p).ok());
    let portal = RemoteDesktop::new()
        .await
        .map_err(|e| format!("Remote desktop portal unavailable: {e}"))?;
    let session = portal
        .create_session(Default::default())
        .await
        .map_err(|e| format!("Could not create a remote desktop session: {e}"))?;
    portal
        .select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(enumflags2::BitFlags::from(DeviceType::Keyboard))
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(saved.as_deref().map(str::trim)),
        )
        .await
        .and_then(|r| r.response())
        .map_err(|e| format!("Could not select the keyboard device: {e}"))?;
    let selected = portal
        .start(&session, None, StartOptions::default())
        .await
        .and_then(|r| r.response())
        .map_err(|e| format!("Keyboard control was not allowed: {e}"))?;
    if !selected.devices().contains(DeviceType::Keyboard) {
        return Err("Keyboard control was not granted".into());
    }
    if let (Some(path), Some(token)) = (token_path, selected.restore_token()) {
        if let Err(e) = std::fs::write(path, token) {
            warn!("remote desktop: could not save the restore token: {e}");
        }
    }
    info!("remote desktop: keyboard control session started");
    Ok(Active { portal, session })
}

async fn press_chord(active: &Active, keys: &[i32]) -> Result<(), String> {
    let send = |keysym: i32, state: KeyState| {
        active.portal.notify_keyboard_keysym(
            &active.session,
            keysym,
            state,
            NotifyKeyboardKeysymOptions::default(),
        )
    };
    let mut result = Ok(());
    let mut pressed = Vec::new();
    for &key in keys {
        if let Err(e) = send(key, KeyState::Pressed).await {
            result = Err(e.to_string());
            break;
        }
        pressed.push(key);
        tokio::time::sleep(Duration::from_millis(8)).await;
    }
    // Always release what was pressed, so a failure never leaves Ctrl stuck.
    for &key in pressed.iter().rev() {
        let _ = send(key, KeyState::Released).await;
        tokio::time::sleep(Duration::from_millis(8)).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_match_paste_methods() {
        assert_eq!(chord(&PasteMethod::CtrlV), Some(vec![XK_CONTROL_L, XK_V]));
        assert_eq!(
            chord(&PasteMethod::ShiftInsert),
            Some(vec![XK_SHIFT_L, XK_INSERT])
        );
        assert_eq!(chord(&PasteMethod::None), None);
    }
}
