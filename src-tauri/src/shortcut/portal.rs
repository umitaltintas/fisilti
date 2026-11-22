//! XDG Desktop Portal `GlobalShortcuts` backend (Linux / Wayland)
//!
//! Wayland compositors do not let clients grab keys, so the X11-based
//! backends (`tauri_impl`, `handy_keys`) never fire while a native Wayland
//! window has focus. The `org.freedesktop.portal.GlobalShortcuts` portal
//! (KDE Plasma 6, GNOME 48+) is the sanctioned way around that: the app
//! *declares* shortcuts and the compositor delivers `Activated` /
//! `Deactivated` signals when the user presses them.
//!
//! Key differences from the other backends:
//!
//! - The app only supplies a *preferred* trigger. The user (or the desktop)
//!   owns the final choice and can change it in system settings, so the
//!   trigger actually in effect is read back (`ListShortcuts`,
//!   `ShortcutsChanged`) and surfaced to the UI via [`PortalShortcutStatus`].
//! - Shortcuts cannot be removed one by one from a session; a changed or
//!   removed binding therefore re-creates the session, while pure additions
//!   are bound incrementally.
//! - Everything lives on one worker task that owns the session; the sync
//!   entry points below only update the desired set and nudge the worker.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut, Shortcut};
use ashpd::desktop::Session;
use ashpd::AppID;
use futures_util::StreamExt;
use log::{debug, error, info, warn};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use super::handler::handle_shortcut_event;
use super::PortalShortcutStatus;
use crate::settings::{self, ShortcutBinding};

/// Event carrying a fresh [`PortalShortcutStatus`] whenever it changes.
pub const PORTAL_STATUS_EVENT: &str = "portal-shortcuts-changed";

/// How long the startup probe may wait for the portal before falling back.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Escape hatch: set to any value to skip the portal backend entirely.
const DISABLE_ENV: &str = "FISILTI_DISABLE_PORTAL_SHORTCUTS";

enum Cmd {
    /// Reconcile the portal session with the desired set of bindings.
    Sync,
    /// Open the desktop's shortcut configuration UI.
    Configure,
}

/// What we asked the portal for: description + preferred trigger per id.
type Wanted = BTreeMap<String, (String, Option<String>)>;

#[derive(Default)]
struct Shared {
    /// Bindings the app wants registered, keyed by binding id.
    desired: Mutex<BTreeMap<String, ShortcutBinding>>,
    status: Mutex<PortalShortcutStatus>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Managed state present only while the portal backend is the active one.
pub struct PortalState {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Shared>,
}

impl PortalState {
    fn nudge(&self) {
        if self.tx.send(Cmd::Sync).is_err() {
            warn!("Portal shortcut worker is gone; shortcut change not applied");
        }
    }
}

/// Whether the session looks like Wayland.
fn wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Whether the portal backend has taken over shortcut handling.
pub fn is_active(app: &AppHandle) -> bool {
    app.try_state::<PortalState>().is_some()
}

/// Current status for the frontend.
pub fn status(app: &AppHandle) -> PortalShortcutStatus {
    match app.try_state::<PortalState>() {
        Some(state) => lock(&state.shared.status).clone(),
        None => PortalShortcutStatus::default(),
    }
}

// ============================================================================
// Trigger syntax
// ============================================================================

/// Convert an app binding string (`"ctrl+shift+space"`) into the portal's
/// trigger syntax (`"CTRL+SHIFT+space"`: upper-case modifiers `CTRL`, `ALT`,
/// `SHIFT`, `LOGO`, then an xkb keysym name).
pub fn to_portal_trigger(raw: &str) -> Result<String, String> {
    let mut modifiers: Vec<String> = Vec::new();
    let mut key: Option<String> = None;

    for part in raw.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        let modifier = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some("CTRL"),
            "alt" | "option" => Some("ALT"),
            "shift" => Some("SHIFT"),
            "meta" | "command" | "cmd" | "super" | "win" | "windows" | "logo" => Some("LOGO"),
            "fn" | "function" => {
                return Err("The 'fn' key cannot be used with portal shortcuts".into())
            }
            _ => None,
        };
        match modifier {
            Some(m) => {
                if !modifiers.iter().any(|existing| existing == m) {
                    modifiers.push(m.to_string());
                }
            }
            None => {
                if key.replace(keysym_name(part)).is_some() {
                    return Err(format!("'{raw}' has more than one non-modifier key"));
                }
            }
        }
    }

    let Some(key) = key else {
        return Err("A shortcut needs a main key (letter, number, F-key, …)".into());
    };
    modifiers.push(key);
    Ok(modifiers.join("+"))
}

/// Map a key name from the app's recorder to an xkb keysym name.
fn keysym_name(key: &str) -> String {
    let lower = key.to_ascii_lowercase();
    let mapped = match lower.as_str() {
        "escape" | "esc" => "Escape",
        "enter" | "return" => "Return",
        "tab" => "Tab",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page up" | "page_up" => "Page_Up",
        "pagedown" | "page down" | "page_down" => "Page_Down",
        "up" | "arrowup" | "arrow up" => "Up",
        "down" | "arrowdown" | "arrow down" => "Down",
        "left" | "arrowleft" | "arrow left" => "Left",
        "right" | "arrowright" | "arrow right" => "Right",
        "capslock" | "caps lock" => "Caps_Lock",
        "space" | " " => "space",
        "-" | "minus" => "minus",
        "=" | "equal" => "equal",
        "`" | "grave" | "backquote" => "grave",
        "," | "comma" => "comma",
        "." | "period" => "period",
        "/" | "slash" => "slash",
        "\\" | "backslash" => "backslash",
        ";" | "semicolon" => "semicolon",
        "'" | "quote" | "apostrophe" => "apostrophe",
        "[" | "bracketleft" => "bracketleft",
        "]" | "bracketright" => "bracketright",
        _ => "",
    };
    if !mapped.is_empty() {
        return mapped.to_string();
    }
    // F1..F24
    if let Some(n) = lower.strip_prefix('f') {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return format!("F{n}");
        }
    }
    // Letters and digits are keysym names in lower case already.
    lower
}

/// Whether a converted trigger carries at least one modifier.
fn has_modifier(trigger: &str) -> bool {
    trigger.contains('+')
}

/// Validate a binding for the portal backend.
pub fn validate_shortcut(raw: &str) -> Result<(), String> {
    to_portal_trigger(raw).map(|_| ())
}

/// What to ask the portal for as the preferred trigger of a binding.
///
/// The portal grabs the key system-wide for as long as the session lives, and
/// a session cannot add or drop a grab per recording. A bare key such as the
/// default cancel key (`Escape`) would therefore stop working in every other
/// application, so unmodified keys are not offered; the user can still assign
/// one in system settings.
fn preferred_trigger(binding: &ShortcutBinding) -> Option<String> {
    let trigger = to_portal_trigger(&binding.current_binding).ok()?;
    if binding.id == "cancel" && !has_modifier(&trigger) {
        return None;
    }
    Some(trigger)
}

fn wanted_from(desired: &BTreeMap<String, ShortcutBinding>) -> Wanted {
    desired
        .values()
        .map(|b| (b.id.clone(), (b.name.clone(), preferred_trigger(b))))
        .collect()
}

// ============================================================================
// Backend entry points (mirror tauri_impl / handy_keys)
// ============================================================================

/// Try to bring the portal backend up. `Err` means "not available" and the
/// caller falls back to the X11-based backends.
pub fn init_shortcuts(app: &AppHandle) -> Result<(), String> {
    if std::env::var_os(DISABLE_ENV).is_some() {
        return Err(format!("disabled via {DISABLE_ENV}"));
    }
    if !wayland_session() {
        return Err("not a Wayland session".into());
    }
    if is_active(app) {
        return Ok(());
    }

    let app_id = app.config().identifier.clone();
    let app_name = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| app_id.clone());
    let (portal, session) = tauri::async_runtime::block_on(async {
        tokio::time::timeout(PROBE_TIMEOUT, probe(&app_id, &app_name))
            .await
            .map_err(|_| "timed out waiting for the desktop portal".to_string())?
    })?;

    let shared = Arc::new(Shared::default());
    *lock(&shared.desired) = initial_bindings(app)
        .into_iter()
        .map(|b| (b.id.clone(), b))
        .collect();
    lock(&shared.status).active = true;

    let (tx, rx) = mpsc::unbounded_channel();
    // The first sync happens as soon as the worker starts.
    let _ = tx.send(Cmd::Sync);
    app.manage(PortalState {
        tx,
        shared: shared.clone(),
    });
    tauri::async_runtime::spawn(worker(app.clone(), rx, shared, portal, session));

    info!("Using the XDG GlobalShortcuts portal for global shortcuts");
    Ok(())
}

/// Register the app with the portal and open the first session. Opening it
/// here (it never prompts the user) proves the portal accepts us, so that a
/// broken setup falls back to the other backends instead of leaving dead keys.
async fn probe(
    app_id: &str,
    app_name: &str,
) -> Result<(GlobalShortcuts, Session<GlobalShortcuts>), String> {
    // A non-sandboxed process has no app id the portal can see, and without
    // one `CreateSession` is refused ("An app id is required"). Registering
    // must precede any other portal call on this connection. The portal only
    // accepts ids that have a matching `<id>.desktop` file; it is rejected
    // when the desktop already knows an id for us (e.g. an app scope), which
    // is fine.
    let id: AppID = app_id
        .parse()
        .map_err(|e| format!("'{app_id}' is not a valid portal app id: {e}"))?;
    match ashpd::register_host_app(id.clone()).await {
        Ok(()) => debug!("Registered '{app_id}' with the host portal registry"),
        Err(e) if e.to_string().contains("App info not found") => {
            ensure_desktop_entry(app_id, app_name)?;
            ashpd::register_host_app(id)
                .await
                .map_err(|e| format!("Could not register '{app_id}' with the portal: {e}"))?;
            debug!("Registered '{app_id}' after creating its desktop entry");
        }
        Err(e) => debug!("Host portal registry registration skipped: {e}"),
    }

    let portal = GlobalShortcuts::new()
        .await
        .map_err(|e| format!("GlobalShortcuts portal unavailable: {e}"))?;
    let session = portal
        .create_session(Default::default())
        .await
        .map_err(|e| format!("Could not create a portal shortcut session: {e}"))?;
    Ok((portal, session))
}

/// Create `~/.local/share/applications/<app_id>.desktop` if it is missing.
///
/// xdg-desktop-portal maps a host app to an id only through a desktop file of
/// that exact name. The packaged bundles name theirs after the product, and
/// dev/AppImage runs have none, so write a hidden one for the portal. An
/// existing file is never touched.
fn ensure_desktop_entry(app_id: &str, app_name: &str) -> Result<(), String> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".local/share"))
        })
        .ok_or("Cannot locate the user's data directory")?;
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate the executable: {e}"))?;
    write_desktop_entry(&data_home.join("applications"), app_id, app_name, &exe)
}

fn write_desktop_entry(
    dir: &std::path::Path,
    app_id: &str,
    app_name: &str,
    exe: &std::path::Path,
) -> Result<(), String> {
    let path = dir.join(format!("{app_id}.desktop"));
    if path.exists() {
        return Ok(());
    }
    // Exec values are quoted; backslash, quote, `$`, backtick and `%` are
    // escaped per the Desktop Entry spec.
    let mut exec = String::new();
    for c in exe.to_string_lossy().chars() {
        match c {
            '\\' | '"' | '`' | '$' => {
                exec.push('\\');
                exec.push(c);
            }
            '%' => exec.push_str("%%"),
            _ => exec.push(c),
        }
    }
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName={app_name}\nExec=\"{exec}\"\nNoDisplay=true\n"
    );
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&path, entry))
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    info!(
        "Created {} so the portal can identify the app",
        path.display()
    );
    Ok(())
}

fn initial_bindings(app: &AppHandle) -> Vec<ShortcutBinding> {
    let user_settings = settings::load_or_create_app_settings(app);
    settings::get_default_settings()
        .bindings
        .into_iter()
        .filter(|(id, _)| {
            // Hide the post-processing shortcut while the feature is off.
            !(id == "transcribe_with_post_process" && !user_settings.post_process_enabled)
        })
        .map(|(id, default_binding)| {
            user_settings
                .bindings
                .get(&id)
                .cloned()
                .unwrap_or(default_binding)
        })
        .collect()
}

/// Ask the worker to bind (or re-bind) a binding.
pub fn register_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    validate_shortcut(&binding.current_binding)?;
    let state = app
        .try_state::<PortalState>()
        .ok_or("Portal shortcuts are not active")?;
    lock(&state.shared.desired).insert(binding.id.clone(), binding);
    state.nudge();
    Ok(())
}

/// Drop a binding from the desired set. The cancel binding is permanent.
pub fn unregister_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    if binding.id == "cancel" {
        return Ok(());
    }
    let state = app
        .try_state::<PortalState>()
        .ok_or("Portal shortcuts are not active")?;
    lock(&state.shared.desired).remove(&binding.id);
    state.nudge();
    Ok(())
}

/// Open the desktop's shortcut configuration UI (portal v2+).
pub fn configure(app: &AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<PortalState>()
        .ok_or("Portal shortcuts are not active")?;
    state
        .tx
        .send(Cmd::Configure)
        .map_err(|_| "Portal shortcut worker is gone".to_string())
}

// ============================================================================
// Worker
// ============================================================================

struct Worker {
    app: AppHandle,
    shared: Arc<Shared>,
    portal: GlobalShortcuts,
    session: Option<Session<GlobalShortcuts>>,
    /// What the current session was asked to bind.
    bound: Wanted,
}

async fn worker(
    app: AppHandle,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    shared: Arc<Shared>,
    portal: GlobalShortcuts,
    session: Session<GlobalShortcuts>,
) {
    // Subscribe before binding so no early signal is missed.
    let (mut activated, mut deactivated, mut changed) = match futures_util::try_join!(
        portal.receive_activated(),
        portal.receive_deactivated(),
        portal.receive_shortcuts_changed(),
    ) {
        Ok((a, d, c)) => (Box::pin(a), Box::pin(d), Box::pin(c)),
        Err(e) => {
            fail(
                &app,
                &shared,
                format!("Could not subscribe to portal signals: {e}"),
            );
            return;
        }
    };

    let mut w = Worker {
        app: app.clone(),
        shared,
        portal,
        session: Some(session),
        bound: Wanted::new(),
    };

    loop {
        tokio::select! {
            Some(a) = activated.next() => w.dispatch(a.shortcut_id(), true),
            Some(d) = deactivated.next() => w.dispatch(d.shortcut_id(), false),
            Some(c) = changed.next() => {
                info!("Portal shortcuts changed by the desktop");
                w.record_triggers(c.shortcuts());
            }
            cmd = rx.recv() => match cmd {
                Some(Cmd::Sync) => w.sync().await,
                Some(Cmd::Configure) => w.configure().await,
                None => break,
            },
        }
    }

    if let Some(session) = w.session.take() {
        let _ = session.close().await;
    }
}

fn fail(app: &AppHandle, shared: &Shared, message: String) {
    error!("{message}");
    lock(&shared.status).error = Some(message);
    publish(app, shared);
}

fn publish(app: &AppHandle, shared: &Shared) {
    let status = lock(&shared.status).clone();
    if let Err(e) = app.emit(PORTAL_STATUS_EVENT, status) {
        warn!("Failed to emit {PORTAL_STATUS_EVENT}: {e}");
    }
}

impl Worker {
    /// Route a portal signal into the shared shortcut handler.
    fn dispatch(&self, shortcut_id: &str, pressed: bool) {
        let trigger = lock(&self.shared.status)
            .triggers
            .get(shortcut_id)
            .cloned()
            .unwrap_or_default();
        debug!("Portal shortcut '{shortcut_id}' pressed={pressed} ({trigger})");
        handle_shortcut_event(&self.app, shortcut_id, &trigger, pressed);
    }

    fn record_triggers(&self, shortcuts: &[Shortcut]) {
        let triggers: HashMap<String, String> = shortcuts
            .iter()
            .map(|s| (s.id().to_string(), s.trigger_description().to_string()))
            .collect();
        for (id, trigger) in &triggers {
            info!("Portal shortcut '{id}' is bound to '{trigger}'");
        }
        {
            let mut status = lock(&self.shared.status);
            status.triggers = triggers;
            status.error = None;
        }
        publish(&self.app, &self.shared);
    }

    async fn configure(&self) {
        let Some(session) = &self.session else {
            warn!("Cannot open shortcut settings: no portal session yet");
            return;
        };
        if let Err(e) = self
            .portal
            .configure_shortcuts(session, None, Default::default())
            .await
        {
            warn!("ConfigureShortcuts failed: {e}");
        }
    }

    /// Make the portal session match the desired bindings.
    async fn sync(&mut self) {
        let wanted = wanted_from(&lock(&self.shared.desired));
        if wanted == self.bound && self.session.is_some() {
            return;
        }

        // Bound shortcuts cannot be removed or re-triggered in place, so
        // anything but a pure addition starts over with a fresh session.
        let additions_only = self.session.is_some()
            && self
                .bound
                .iter()
                .all(|(id, entry)| wanted.get(id) == Some(entry));
        let to_bind: Wanted = if additions_only {
            wanted
                .iter()
                .filter(|(id, _)| !self.bound.contains_key(*id))
                .map(|(id, entry)| (id.clone(), entry.clone()))
                .collect()
        } else {
            if let Some(old) = self.session.take() {
                if let Err(e) = old.close().await {
                    debug!("Closing the previous portal session failed: {e}");
                }
            }
            self.bound.clear();
            match self.portal.create_session(Default::default()).await {
                Ok(session) => self.session = Some(session),
                Err(e) => {
                    fail(
                        &self.app,
                        &self.shared,
                        format!("Could not create a portal shortcut session: {e}"),
                    );
                    return;
                }
            }
            wanted.clone()
        };

        let Some(session) = &self.session else {
            return;
        };
        let new_shortcuts: Vec<NewShortcut> = to_bind
            .iter()
            .map(|(id, (description, trigger))| {
                NewShortcut::new(id.as_str(), description.as_str())
                    .preferred_trigger(trigger.as_deref())
            })
            .collect();

        if !new_shortcuts.is_empty() {
            info!(
                "Binding {} portal shortcut(s): {:?}",
                new_shortcuts.len(),
                to_bind
                    .iter()
                    .map(|(id, (_, t))| (id.as_str(), t.as_deref()))
                    .collect::<Vec<_>>()
            );
            // May show a system dialog the user has to confirm; this waits
            // for the answer.
            let result = match self
                .portal
                .bind_shortcuts(session, &new_shortcuts, None, Default::default())
                .await
            {
                Ok(request) => request.response(),
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                fail(
                    &self.app,
                    &self.shared,
                    format!("BindShortcuts failed: {e}"),
                );
                return;
            }
        }
        self.bound = wanted;

        // Read back what the portal actually bound (the user may have
        // overridden our preferences).
        match self
            .portal
            .list_shortcuts(session, Default::default())
            .await
        {
            Ok(request) => match request.response() {
                Ok(list) => self.record_triggers(list.shortcuts()),
                Err(e) => warn!("ListShortcuts failed: {e}"),
            },
            Err(e) => warn!("ListShortcuts failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bindings_to_portal_triggers() {
        assert_eq!(to_portal_trigger("ctrl+space").unwrap(), "CTRL+space");
        assert_eq!(
            to_portal_trigger("Ctrl+Shift+Space").unwrap(),
            "CTRL+SHIFT+space"
        );
        assert_eq!(to_portal_trigger("option+space").unwrap(), "ALT+space");
        assert_eq!(to_portal_trigger("super+a").unwrap(), "LOGO+a");
        assert_eq!(to_portal_trigger("escape").unwrap(), "Escape");
        assert_eq!(to_portal_trigger("alt+f5").unwrap(), "ALT+F5");
        assert_eq!(to_portal_trigger("ctrl+pageup").unwrap(), "CTRL+Page_Up");
    }

    #[test]
    fn rejects_unusable_bindings() {
        assert!(to_portal_trigger("").is_err());
        assert!(to_portal_trigger("ctrl+shift").is_err());
        assert!(to_portal_trigger("ctrl+a+b").is_err());
        assert!(to_portal_trigger("fn+a").is_err());
    }

    #[test]
    fn desktop_entry_is_written_once_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("applications");
        let exe = std::path::Path::new("/opt/My App/fisilti");
        write_desktop_entry(&apps, "com.example.app", "Example", exe).unwrap();
        let file = apps.join("com.example.app.desktop");
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.contains("Name=Example\n"));
        assert!(written.contains("Exec=\"/opt/My App/fisilti\"\n"));
        assert!(written.contains("NoDisplay=true"));

        std::fs::write(&file, "custom").unwrap();
        write_desktop_entry(&apps, "com.example.app", "Example", exe).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "custom");
    }

    #[test]
    fn bare_cancel_key_is_not_offered_as_a_global_grab() {
        let mut cancel = settings::get_default_settings().bindings["cancel"].clone();
        assert_eq!(preferred_trigger(&cancel), None);
        cancel.current_binding = "ctrl+escape".into();
        assert_eq!(preferred_trigger(&cancel).as_deref(), Some("CTRL+Escape"));
    }
}
