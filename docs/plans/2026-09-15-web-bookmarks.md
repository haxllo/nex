# Web Bookmarks Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add persistent web bookmarks that open from Nex and display each site’s favicon as its result icon.

**Architecture:** Extend existing Quick Launch persistence with a small bookmark collection in the TOML config, storing title and normalized URL. Convert bookmarks into `SearchItem` rows when the overlay is idle or when a bookmark query matches. Store downloaded favicon bytes in `%APPDATA%\Nex\bookmark-icons`, while passing cached local icon paths through the existing `IconCache` pipeline. Use the existing URL launch action path for opening bookmarks; do not create a second browser-launch implementation.

**Tech Stack:** Rust, serde/TOML config, existing SQLite/Tantivy search model, existing overlay icon cache, Windows filesystem/browser launch.

---

## Task 1: Define Bookmark Model And Config Persistence

**Files:**
- Modify: `apps/core/src/config.rs`
- Modify: `apps/core/src/settings_snapshot.rs` if settings exposure is needed
- Test: `apps/core/tests/config_test.rs`

**Steps:**

1. Add `WebBookmark { title, url, icon_path }` with serde defaults.
2. Add `web_bookmarks: Vec<WebBookmark>` to `Config` with default empty list.
3. Normalize/validate URLs as `http` or `https`; reject empty titles and malformed URLs.
4. Preserve unknown/old config fields through existing migration flow.
5. Add config round-trip and invalid-bookmark tests.
6. Commit: `feat: persist web bookmarks in config`.

## Task 2: Build Bookmark Search Items

**Files:**
- Create: `apps/core/src/bookmarks.rs`
- Modify: `apps/core/src/lib.rs`
- Modify: `apps/core/src/runtime_search_session.rs` or action composition path
- Test: `apps/core/src/bookmarks.rs`

**Steps:**

1. Add stable bookmark IDs based on normalized URL.
2. Convert bookmark records to `SearchItem` values with:
   - kind `bookmark`
   - title from bookmark title
   - path/subtitle as URL
   - icon path pointing to cached favicon file when present
3. Add matching by title, URL, and host.
4. Add idle-result ordering and query-result limits without changing existing app/file ranking.
5. Add tests for ID stability, normalization, duplicate URLs, and ranking.
6. Commit: `feat: add bookmark search items`.

## Task 3: Reuse URL Launch And Add Bookmark Actions

**Files:**
- Modify: `apps/core/src/runtime_actions.rs`
- Modify: `apps/core/src/runtime_loop.rs`
- Modify: `apps/core/src/overlay/model.rs` only if a dedicated bookmark action is needed
- Test: `apps/core/src/runtime.rs` or bookmark tests

**Steps:**

1. Route bookmark selection through existing `launch_open_target` URL handling.
2. Add command actions for add/remove bookmark only if current UI has a stable action affordance; do not add a new UI mode prematurely.
3. Preserve existing app pin/unpin behavior.
4. Ensure opening a bookmark records no fake app launch count.
5. Add tests for bookmark selection and browser URL dispatch.
6. Commit: `feat: open bookmarks from overlay`.

## Task 4: Download And Cache Favicons

**Files:**
- Modify: `apps/core/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `apps/core/src/bookmarks.rs`
- Modify: `apps/core/src/overlay/icons.rs` only if shared decoding helper is required
- Test: `apps/core/src/bookmarks.rs`

**Steps:**

1. Use an already available HTTP dependency if the workspace has one; otherwise use Windows WinHTTP or a minimal existing platform path before adding a dependency.
2. Try favicon sources in order:
   - `https://<host>/favicon.ico`
   - HTML `<link rel="icon">` only if a safe parser already exists; otherwise defer.
3. Accept only HTTP(S), bounded response size, and image content that existing PNG/icon decoding can handle.
4. Store favicon bytes under `%APPDATA%\Nex\bookmark-icons\<stable-hash>.png`.
5. Download asynchronously; never block overlay or runtime event loop.
6. Keep bookmark visible with fallback icon when download fails.
7. Add cache hit, failed download, size limit, and path traversal tests.
8. Commit: `feat: cache bookmark favicons`.

## Task 5: Add Bookmark Pinning UI

**Files:**
- Modify: `apps/core/assets/index.html`
- Modify: `apps/core/assets/app.js`
- Modify: `apps/core/assets/style.css`
- Modify: `apps/core/src/overlay/host.rs` or IPC handler
- Test: manual UI validation

**Steps:**

1. Add bookmark context actions only where selected result is a URL/bookmark.
2. Add bookmark form with URL and title fields.
3. Validate before sending IPC.
4. Persist through Rust config writer.
5. Refresh results and favicon asynchronously after save.
6. Preserve keyboard navigation, focus return, Escape, and screen-reader labels.
7. Do not change existing app/file result layout.
8. Commit: `feat: add bookmark controls to overlay`.

## Task 6: Settings Page For Bookmark Management

**Files:**
- Modify: `apps/core/assets/settings.html`
- Modify: `apps/core/assets/settings.js`
- Modify: `apps/core/src/overlay/host.rs`
- Modify: `apps/core/src/settings_snapshot.rs`
- Test: manual UI validation

**Steps:**

1. Add bookmark list with title, URL, favicon preview, edit, and delete controls.
2. Keep settings controls keyboard-accessible and focus-trapped.
3. Avoid downloading favicons on settings UI thread.
4. Persist edits atomically through existing settings save path.
5. Add empty state and failed-favicon fallback.
6. Commit: `feat: manage bookmarks in settings`.

## Task 7: Full Verification

**Steps:**

1. Run `cargo check -p nex`.
2. Run bookmark/config targeted tests.
3. Run existing action, search, and config tests.
4. Manually verify:
   - Add bookmark.
   - Favicon appears after download.
   - Offline/invalid favicon keeps fallback icon.
   - Open bookmark in default browser.
   - Restart app and bookmark persists.
   - Delete bookmark.
   - Duplicate URL handling.
   - Keyboard-only use.
   - Large favicon response rejection.
5. Commit test fixes separately.

## Deferred Until Needed

- Full HTML favicon `<link>` parsing.
- Bookmark folders/tags.
- Sync/export/import.
- Private/incognito browser launch.
- Multi-resolution favicon selection.
