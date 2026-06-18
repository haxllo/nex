# 06-01 — Crash Resilience

## Changes Made

### Task 1: `search_worker.rs` — catch_unwind for Tantivy panics
- Wrapped `service.lock()` + `search_overlay_results_with_session()` in `std::panic::catch_unwind(AssertUnwindSafe(...))`
- Mutex poisoning returns error via `res_tx` with message "search index is locked (internal error)" instead of panicking
- Panic payload decoded via `downcast_ref::<&str>()` / `downcast_ref::<String>()` and included in error message

### Task 2: `overlay/icons.rs` — catch_unwind for corrupt icon decoding
- Wrapped `image::load_from_memory()` in `std::panic::catch_unwind(AssertUnwindSafe(...))` in the `decode()` function
- Corrupt files return `None` gracefully (existing fallback handles decode failures)

### Task 3: `overlay/hotkey.rs` + `runtime_loop.rs` — hotkey thread crash detection and auto-restart
- Added `HotkeyListener::is_alive()` method that checks thread liveness via `JoinHandle::is_finished()`
- Added `logging::warn()` when the hotkey message loop exits unexpectedly (not via `should_exit` flag)
- Wrapped `hotkey_listener` in `Arc<Mutex<Option<HotkeyListener>>>` shared with `RuntimeWorker`
- Added `event_tx`, `hotkey_check_counter` fields to `RuntimeWorker`
- Added periodic health check in `RuntimeWorker::on_event()` (every 32 events): detects dead listener thread and attempts restart via `HotkeyListener::start()`
- Main thread shutdown properly takes the listener via `lock().unwrap().take()` before joining the worker

### Incidental fix: `overlay/boot.rs`
- Moved `GetMonitorInfoW`, `MonitorFromPoint`, `MONITORINFO`, `MONITOR_DEFAULTTONEAREST` imports from `Win32::UI::WindowsAndMessaging` to `Win32::Graphics::Gdi` (windows-sys API relocation)

## Status: COMPLETE

## Verdict: PASS — `cargo check -p nex-cli` passes with zero errors (19 pre-existing warnings only)
