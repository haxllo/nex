# Using the Windows Key Alone as a Global Hotkey (Suppressing the Start Menu)
### Research report for a Rust + Wry / WebView desktop app

**Date:** 2026-07-25 · **Scope:** Windows 10/11 · **Verdict: ✅ Yes, this is doable** — and the
technique is well-established. Raycast proves the *UX* is possible; on Windows the *mechanism* is a
low-level keyboard hook. There are no academic papers on this narrow topic — the authoritative
literature is Microsoft's Win32 documentation and Raymond Chen's engineering blog, plus working
open-source implementations.

---

## 1. TL;DR — the recipe

1. Install a **`WH_KEYBOARD_LL` (id = 13)** global keyboard hook with `SetWindowsHookExW` on a
   **dedicated thread that runs a message loop** [^3][^5].
2. In the hook proc, watch for `VK_LWIN` (`0x5B`) / `VK_RWIN` (`0x5C`). Track whether the Win key
   was pressed **alone** (no other key went down while it was held) [^1].
3. **Suppress** the Win events you want to steal by **returning a nonzero value (`LRESULT(1)`) and
   *not* calling `CallNextHookEx`** — Microsoft's own docs confirm this "prevent[s] the system from
   passing the message to the rest of the hook chain or the target window procedure" [^2].
4. On a confirmed **Win-alone key-up**, **inject a dummy "mask" keystroke** (an inert virtual key
   such as `0xE8` "unassigned" or `0xFF` "no mapping") via `SendInput`, so the shell believes Win was
   released *together with another key* and therefore does **not** open the Start menu. This is
   exactly AutoHotkey's documented `A_MenuMaskKey` / `#MenuMaskKey vkE8` mechanism [^12].
5. Toggle / focus your Wry window from the same code path.

The single most important fact: **the Start menu opens on Win key *release*, not press** — because
"UI actions occur on release" (Raymond Chen, *The Old New Thing*, 2006). That is why you must handle
the key-up and mask it, not just swallow the key-down.

---

## 2. Why the Start menu opens (the underlying behavior)

Windows treats a *tap* of the Win key (down → up with no other key pressed in between) as the
"open Start menu" command, and it fires that command on the **key-up**. This is a general Windows
UI convention: menu/accelerator actions are committed on release so the user can cancel by dragging
off the key. Consequently:

- Swallowing only `WM_KEYDOWN` is **not enough** — the key-up still reaches the shell and opens Start.
- You must intercept the **key-up** and either suppress it *and* convince the shell that a chord
  (Win+something) occurred, which is what the dummy mask key does [^12].

Microsoft's official sample for games ("Disabling Shortcut Keys in Games") demonstrates eating
`VK_LWIN`/`VK_RWIN` in a `WH_KEYBOARD_LL` proc by returning `1`, gated on window activation via
`WM_ACTIVATEAPP` [^1].

---

## 3. The mechanism in detail

### 3.1 The hook
- `SetWindowsHookExW(WH_KEYBOARD_LL, hookProc, hInstance, 0)` — `WH_KEYBOARD_LL = 13`, installed
  globally (`dwThreadId = 0`) [^3].
- The proc receives `wParam` ∈ {`WM_KEYDOWN`, `WM_KEYUP`, `WM_SYSKEYDOWN`, `WM_SYSKEYUP`} and
  `lParam` → `KBDLLHOOKSTRUCT` [^2].
- **The hook runs in the context of the thread that installed it, delivered as a message — so that
  thread MUST have a message loop** [^2][^5].
- **Returning nonzero suppresses the event**; otherwise call `CallNextHookEx` and return its result
  (recommended so other hooks still fire) [^2].

### 3.2 `KBDLLHOOKSTRUCT` fields & flag bits (verified)
| Field / flag | Value | Meaning |
|---|---|---|
| `vkCode` | — | Virtual-key code (`VK_LWIN = 0x5B`, `VK_RWIN = 0x5C`) |
| `scanCode` | — | Hardware scan code |
| `flags` | — | Bit field (below) |
| `LLKHF_EXTENDED` | `0x01` | Extended key |
| `LLKHF_LOWER_IL_INJECTED` | `0x02` | Injected from a lower-IL process |
| `LLKHF_INJECTED` | `0x10` | Event was injected (use to **avoid re-processing your own SendInput**) |
| `LLKHF_ALTDOWN` | `0x20` | Alt is down |
| `LLKHF_UP` | `0x80` | Key is being released (key-up) |

The `LLKHF_INJECTED` bit is important: when you inject the mask key (or replay a Win+chord), tag it
and/or check this flag so your hook doesn't intercept its own synthetic events (recursion guard).

### 3.3 Detecting "Win alone" (tap vs. chord)
Maintain a small state machine:
- On Win **down**: set `winDown = true`, `chord = false`.
- On **any other key down** while `winDown`: set `chord = true` (let everything pass through — the
  user is doing Win+C, Win+V, etc.).
- On Win **up**: if `winDown && !chord` → it was a **solo tap** → suppress + inject mask key + fire
  your action. Otherwise pass through.

This is the same solo-vs-combo logic used by the open-source WinKey command-palette replacement
(see §6), which replays real Win+chords via `SendInput`/`keybd_event` while blocking the solo tap.

### 3.4 The mask-key injection
After suppressing the solo Win key-up, send one inert key event so the shell sees a chord:
- Use an **unassigned** VK so nothing else reacts: `0xE8` ("unassigned") is the community standard
  (AutoHotkey's default mask key); `0xFF` ("no mapping") also works [^12].
- **Do NOT use `VK 0x07`** — it was repurposed for the Game Bar since Windows 10 1909 and can trigger
  Game Bar behavior.
- Send it with `SendInput` (a key-down + key-up, or just a key-up), optionally setting
  `KEYEVENTF_EXTENDEDKEY`/`dwExtraInfo` tag for your recursion guard.

AutoHotkey exposes this directly: `~LWin::Send {Blind}{vkE8}` keeps Win usable as a modifier while
preventing the lone-tap from opening Start, and `#MenuMaskKey vkE8` changes the masking key [^12].

---

## 4. Exact Win32 constants & calls (quick reference)
| Symbol | Value | Notes |
|---|---|---|
| `WH_KEYBOARD_LL` | `13` | Global low-level keyboard hook [^3] |
| `VK_LWIN` | `0x5B` | Left Windows key |
| `VK_RWIN` | `0x5C` | Right Windows key |
| `VK 0xE8` | unassigned | Recommended mask key [^12] |
| `VK 0xFF` | no mapping | Alternative mask key |
| `HC_ACTION` | `0` | Only process when `nCode == HC_ACTION` [^2] |
| `WM_KEYDOWN/UP`, `WM_SYSKEYDOWN/UP` | — | `wParam` values [^2] |
| `LLKHF_INJECTED` | `0x10` | Recursion guard [^2] |
| APIs | `SetWindowsHookExW`, `CallNextHookEx`, `UnhookWindowsHookEx`, `SendInput`, `GetModuleHandleW` | |

---

## 5. Rust crate landscape (versions verified on crates.io)

| Crate | Version | Can detect Win-alone? | Can **suppress** Start menu? | Fit for Wry loop | Verdict |
|---|---|---|---|---|---|
| **`windows`** (Microsoft) [^17] | 0.62.x (updated 2025-10-06) | ✅ (raw API) | ✅ (return `LRESULT(1)`) | ✅ full control | **Recommended** — direct `WH_KEYBOARD_LL` |
| **`prevent-alt-win-menu`** [^16] | 0.2.2 (2025-06-23) | ✅ | ✅ (purpose-built: hook + dummy key-up on Win/Alt release) | ✅ `start(Config)` + callback | Easiest drop-in; young, ~1.9k downloads |
| `rdev` | 0.5.3 (2023, stale) | ✅ (`MetaLeft/Right`) | ⚠️ only via blocking `grab`/`unstable_grab` | ⚠️ blocking | Not ideal |
| `global-hotkey` (Tauri) | 0.8.0 | ❌ **cannot** — `HotKey::new(mods, key)` requires a `Code` key (RegisterHotKey-based) | ❌ | ✅ | **Confirmed: no lone-modifier support** |
| `willhook` | 0.6.3 | ✅ | ❌ listen-only (no documented suppression) | ✅ | Insufficient |
| `input-hook` | — | **does not exist on crates.io** | — | — | Name confusion; ignore |

**Recommendation:** use the **`windows`** crate directly for maximum control (you need the solo-tap
state machine + mask injection anyway), or **`prevent-alt-win-menu`** if you want the suppression
part pre-built and just need a callback. Required Cargo features for `windows`:
`Win32_Foundation`, `Win32_UI_WindowsAndMessaging` (holds `WH_KEYBOARD_LL`, `SetWindowsHookExW`,
`KBDLLHOOKSTRUCT`), and `Win32_UI_Input_KeyboardAndMouse` (holds `SendInput`, VK constants).

### 5.1 Grounded Rust sketch (combine Microsoft's C sample [^1] with the `windows`-rs API)
> Sketch for illustration — verify exact `windows`-rs type paths against docs.rs for your pinned
> version before shipping.

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use windows::Win32::Foundation::{LRESULT, WPARAM, LPARAM, HINSTANCE};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowsHookExW, CallNextHookEx, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, KBDLLHOOKSTRUCT, HC_ACTION,
    WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_LWIN, VK_RWIN,
};

static WIN_DOWN: AtomicBool = AtomicBool::new(false);
static CHORD:    AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn kb_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    let injected = kb.flags.0 & 0x10 != 0;          // LLKHF_INJECTED — skip our own events
    let vk = kb.vkCode;
    let is_win = vk == VK_LWIN.0 as u32 || vk == VK_RWIN.0 as u32;

    match wparam.0 as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if is_win && !injected { WIN_DOWN.store(true, Ordering::SeqCst); }
            else if WIN_DOWN.load(Ordering::SeqCst) { CHORD.store(true, Ordering::SeqCst); }
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if is_win && !injected {
                let solo = WIN_DOWN.load(Ordering::SeqCst) && !CHORD.load(Ordering::SeqCst);
                WIN_DOWN.store(false, Ordering::SeqCst);
                CHORD.store(false, Ordering::SeqCst);
                if solo {
                    inject_mask_key();          // 0xE8 so shell sees a chord, not a solo tap
                    on_win_tap();               // <-- show/focus your Wry window here
                    return LRESULT(1);          // SUPPRESS: do NOT call CallNextHookEx
                }
            }
        }
        _ => {}
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe fn inject_mask_key() {
    let mut inp = [INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: std::mem::zeroed(), // fill ki: wVk = 0xE8, dwFlags = 0 then KEYUP
    }];
    // ... set ki.wVk = 0xE8; send down+up via SendInput(&mut inp, size_of::<INPUT>())
    let _ = unsafe { SendInput(&inp, std::mem::size_of::<INPUT>() as i32) };
}

fn on_win_tap() { /* signal your UI thread to toggle the window */ }

pub fn start_hook_thread() {
    thread::spawn(|| unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook),
                                     HINSTANCE::default(), 0).unwrap();
        // MESSAGE LOOP REQUIRED on this thread [^2][^5]
        let mut msg = std::mem::zeroed();
        while windows::Win32::UI::WindowsAndMessaging::GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    });
}
```

---

## 6. Real-world implementations & repos (from the research sweep)

| Project | Lang | How it handles Win-alone |
|---|---|---|
| **Flow Launcher** (`Flow-Launcher/Flow.Launcher`, ~15k★) | C# | Since v1.20.0 supports a bare `LWin`/`RWin` hotkey + a "Win Hotkey" plugin; injects a dummy key event to suppress Start |
| **WinKey Command Palette Replacement** (`ArjunC1234/WinKey_CommandPalette_Replacement`, ~37★) | C#/.NET | `WH_KEYBOARD_LL` solo-tap-vs-chord state machine; blocks solo Win, replays Win+chords via `SendInput`/`keybd_event`; uses `LLKHF_INJECTED` + a custom `dwExtraInfo` tag to avoid recursion |
| **prevent-alt-win-menu** (`noriapi/prevent-alt-win-menu`) [^16] | Rust | Crate; sends a dummy key-up on Win/Alt release via `SetWindowsHookExW` |
| **win-hotkeys** (`iholston/win-hotkeys`) | Rust | `WH_KEYBOARD_LL` crate treating Win as a modifier |
| **PowerToys Run** (`microsoft/PowerToys`, `HotkeyManager`) | C# | Injects a dummy key event to avoid Start-menu false positives (see §8 race) |
| **Inhibit Windows Events gist** (`LiamKarlMitchell/...`) [^6] | C | Working `WH_KEYBOARD_LL` example suppressing `VK_LWIN` by `return 1`, with a message pump and `LowLevelHooksTimeout` discussion |

**AutoHotkey reference:** remap `LWin`/`RWin` directly (`LWin::`, `LWin Up::`), use `#` as the Win
modifier, and rely on `A_MenuMaskKey`/`#MenuMaskKey vkE8` to mask the keyup so Start doesn't open
while Win still works as a modifier [^12].

**PowerToys Keyboard Manager:** can *disable*/remap keys, but there is an **open feature request
(#35217)** for an official "Win-as-hotkey" toggle — i.e., even Microsoft's own tool doesn't ship a
clean per-app Win-alone hotkey yet, which is why everyone rolls the hook.

---

## 7. Wry / WebView (Tauri-style) integration notes

- **Run the hook on its own thread with its own message loop** (as in §5.1). Do **not** block the
  hook proc — Windows enforces `LowLevelHooksTimeout` and on Win7+ **silently removes** a slow hook
  with no notification [^2][^5]. Microsoft explicitly recommends the hook thread "pass the work off
  to a worker thread and then immediately return" [^5].
- **Don't fight Wry's UI thread.** Wry/WebView2 owns a COM STA message loop on the main thread. Keep
  the LL hook on a *separate* thread and communicate the "Win tapped" event to the UI thread via a
  channel / `Arc<AtomicBool>` / `wry`/`tao` event-loop proxy, then show/focus the window there.
- **Show/hide/focus pattern:** on Win-tap, if the window is hidden/unfocused → `show()` + `set_focus()`
  (and `SetForegroundWindow` — note foreground-lock rules, you may need `AllowSetForegroundWindow`
  or to attach thread input); if already focused → `hide()`/minimize. A system **tray icon** is the
  conventional way to keep the app resident while the window is hidden.
- **Gate on activation (optional):** Microsoft's game sample only eats Win when the app window is
  active (`WM_ACTIVATEAPP`) [^1]. For a *launcher* you usually want the opposite — eat Win globally
  so it always summons the app — but be aware this steals the Start menu system-wide while running,
  which is the intended Raycast-like behavior.
- **`global-hotkey` / `tauri-plugin-global-shortcut` won't work** for this — they're `RegisterHotKey`
  based and require a non-modifier key. You need the raw LL hook.

---

## 8. Caveats, edge cases & failure modes (be honest about these)

1. **`LowLevelHooksTimeout`** — `HKEY_CURRENT_USER\Control Panel\Desktop\LowLevelHooksTimeout` (ms).
   On Windows 7+ a hook that exceeds it is **silently removed**; Windows 10 1709+ clamps it to
   ~1000 ms. Keep the proc fast; offload work [^2][^5].
2. **Game Bar is NOT blockable** — Microsoft's sample explicitly notes the hook will **not** block
   `Win+G`, `Win+Alt+R`, etc. [^1]. Don't use `VK 0x07` as a mask key (repurposed for Game Bar since
   Win10 1909).
3. **UAC / secure desktop** — the hook is desktop-scoped; it **won't fire** on the secure desktop
   (UAC prompts) or in elevated processes running at a different integrity level.
4. **No admin required** for a normal user-session hook, but it can't intercept higher-IL windows.
5. **Timing race / false positives** — naive implementations sometimes let Start flash or break
   `Alt+Tab`/Win chords. PowerToys hit exactly this (#4578) and fixed it via dummy-key injection
   (#4710). The solo-vs-chord state machine + mask key is the fix.
6. **32/64-bit chains** can both fire if a 32-bit hook precedes a 64-bit one in the chain [^3].
7. **`GetAsyncKeyState` is stale** inside the LL proc — derive state from the events themselves [^2].
8. **Registry "NoWinKeys"/"DisableWinKeys"** Group Policy exists but is **system-wide, not per-app**,
   and unreliable for lone-tap suppression — not the right tool for a launcher.
9. **Works on Windows 11 22H2+** — the LL-hook suppression technique still functions on current
   Windows 11 (confirmed in the research sweep).

---

## 9. macOS vs Windows (why Raycast "just works" and Windows needs more)

- **macOS** has a first-class notion of intercepting modifier taps via `CGEventTap` / `NSEvent`
  global monitors; a tool can observe a lone-modifier tap and the OS doesn't reserve the key the way
  Windows reserves Win for Start. (Raycast's default is actually `⌥ Space`, but single-modifier
  triggers are supported by the platform event tap.)
- **Windows** has **no modifier-tap primitive** — Win is hard-reserved for the shell. You must
  *infer* a tap from down/up events in a `WH_KEYBOARD_LL` hook and *swallow + mask* the event. That
  is the entire reason this needs a low-level hook rather than a normal hotkey API.

---

## 10. Bottom line for your Rust + Wry app

- **Yes, it's doable**, and it's a solved pattern. Use a **`WH_KEYBOARD_LL`** hook (via the
  **`windows`** crate, or the **`prevent-alt-win-menu`** crate for the suppression part), run it on a
  **dedicated message-loop thread**, detect the **solo Win tap**, **suppress** it (`return 1`),
  **inject a `0xE8` mask key**, and signal your Wry UI thread to toggle the window.
- Expect to handle the caveats in §8 (fast proc, Game Bar, UAC, solo-vs-chord race).
- No academic papers exist on this specific topic; the authoritative sources are Microsoft Learn and
  Raymond Chen's blog, plus the working repos in §6.

---

## References (fetched/verified this session)
[^1]: Microsoft Learn — *Disabling Shortcut Keys in Games* (official `WH_KEYBOARD_LL` sample eating `VK_LWIN`/`VK_RWIN`, `WM_ACTIVATEAPP` gating, Game-Bar note). https://learn.microsoft.com/en-us/windows/win32/dxtecharts/disabling-shortcut-keys-in-games
[^2]: Microsoft Learn — *LowLevelKeyboardProc callback function* (return nonzero to suppress; message-loop requirement; `LowLevelHooksTimeout`). https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc
[^3]: Microsoft Learn — *SetWindowsHookExA/W function* (`WH_KEYBOARD_LL = 13`; 32/64-bit chain note). https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowshookexa
[^5]: Microsoft Learn (legacy) — *LowLevelKeyboardProc* (Win7+ silent hook removal; dedicated-thread guidance; raw-input alternative). https://learn.microsoft.com/en-us/previous-versions/windows/desktop/legacy/ms644985(v=vs.85)
[^6]: LiamKarlMitchell — *Inhibit Windows Events* gist (working C `WH_KEYBOARD_LL` suppressing `VK_LWIN`; `LowLevelHooksTimeout` discussion). https://gist.github.com/LiamKarlMitchell/89f7784de196d9491d5b3b0eef2a576e
[^12]: AutoHotkey docs — *A_MenuMaskKey* (`~LWin::Send {Blind}{vkE8}`, `#MenuMaskKey vkE8`; mask Win/Alt keyup to prevent menu while keeping modifier). https://www.autohotkey.com/docs/v2/lib/A_MenuMaskKey.htm
[^16]: crates.io — *prevent-alt-win-menu* v0.2.2 (2025-06-23), "Prevents the menu bar or Start menu from appearing when the Alt or Windows key is released". https://crates.io/crates/prevent-alt-win-menu
[^17]: crates.io — *windows* (Microsoft), updated 2025-10-06. https://crates.io/crates/windows

**Additional repos identified in the research sweep (verify before adopting):**
- Flow Launcher — https://github.com/Flow-Launcher/Flow.Launcher
- WinKey Command Palette Replacement — https://github.com/ArjunC1234/WinKey_CommandPalette_Replacement
- win-hotkeys (Rust) — https://github.com/iholston/win-hotkeys
- prevent-alt-win-menu (Rust) — https://github.com/noriapi/prevent-alt-win-menu
- Microsoft PowerToys — https://github.com/microsoft/PowerToys (Run `HotkeyManager`; Keyboard Manager; feature request #35217; issue #4578 / PR #4710)
- Raymond Chen, *The Old New Thing* — "UI actions occur on release" (2006) and related Win-key/menu-masking posts. https://devblogs.microsoft.com/oldnewthing/
