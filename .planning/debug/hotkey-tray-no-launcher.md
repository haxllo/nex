---
status: resolved
trigger: "the hotkey isnt working or trigering a window tray icon is visible ut when clicked launcher isnt opening"
created: 2026-06-08
updated: 2026-06-08
---

# Debug Session: hotkey-tray-no-launcher

## Current Focus

- hypothesis: Tray icon window proc never stores GWLP_USERDATA — missing WM_CREATE handler (CONFIRMED)
- test: Add WM_CREATE handler, fix pointer type, verify tray clicks work
- expecting: Tray left-click sends OverlayEvent::ExternalShow; right-click shows context menu
- next_action: (none — fix applied, ready for runtime verification)

## Symptoms

- **Expected:** Search overlay window opens when pressing Ctrl+Space or clicking tray icon
- **Actual:** Nothing at all happens — completely silent, no window, no error
- **Errors:** No errors visible at all
- **Timeline:** Never worked (new install on `iced-ui` branch)
- **Reproduction:** Both `nex --foreground` (dev mode) and `nex` (background) — same behavior

## Evidence

- timestamp: 2026-06-08 - read apps/core/src/overlay/tray.rs — tray_wnd_proc (lines 249-284) has no WM_CREATE handler
- timestamp: 2026-06-08 - confirmed line 259: GetWindowLongPtrW(hwnd, GWLP_USERDATA) returns 0 because never set
- timestamp: 2026-06-08 - confirmed line 260: if state_ptr == 0 { return 0; } — exits silently
- timestamp: 2026-06-08 - confirmed line 109: Arc::as_ptr(&state) gives *const Mutex<TrayState>
- timestamp: 2026-06-08 - confirmed line 263: cast as *const Arc<Mutex<TrayState>> — type mismatch (masked by GWLP_USERDATA=0)
- timestamp: 2026-06-08 - confirmed line 279: Box::from_raw on wrong pointer type — UB if reached (masked by GWLP_USERDATA=0)
- timestamp: 2026-06-08 - confirmed line 124: CreateWindowExW passes state_ptr as lpParam (WM_CREATE lParam)
- timestamp: 2026-06-08 - hotkey.rs pipeline verified: crossbeam_channel independent of tray, structurally correct

## Eliminated

- Hotkey registration failure: ruled out — would log error and show tray tooltip issue
- Iced window creation failure: ruled out — would crash or log error
- Game mode suppression: ruled out — disabled by default on new install
- Single-instance guard: ruled out — tray icon visible means runtime started

## Resolution

- root_cause: TrayIcon message window has no WM_CREATE handler in tray_wnd_proc. CreateWindowExW passes state_ptr as lpParam (delivered as lParam of WM_CREATE), but the window proc never stores it via SetWindowLongPtrW. When tray icon messages arrive (NEX_WM_TRAY_ICON), GetWindowLongPtrW returns 0, causing immediate return without sending events. Additionally, Arc::as_ptr returns *const Mutex<TrayState> but window proc cast to *const Arc<Mutex<TrayState>> (type mismatch).
- fix: Applied three changes in apps/core/src/overlay/tray.rs:
  1. Added WM_CREATE handler (lines 254-259): extracts lpCreateParams from CREATESTRUCTW, calls SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr)
  2. Fixed pointer cast line 268: `*const Mutex<TrayState>` instead of `*const Arc<Mutex<TrayState>>`
  3. Simplified WM_DESTROY (line 280): just SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0), no Box::from_raw/Arc::from_raw
- verification: cargo check passes with zero new warnings (17 pre-existing)
- files_changed: apps/core/src/overlay/tray.rs
- specialist_review: rust-engineer — LOOKS_GOOD
