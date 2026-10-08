// Nex — premium overlay UI controller.
//
// Owns the search input + result list locally so navigation has zero
// round-trip latency. Talks to Rust through `window.ipc.postMessage`
// (JSON) and receives state via WebView2 `message` events.

(function () {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const input = $("query");
  const list = $("list");
  const statusEl = $("status");
  const panel = $("panel");
  const searchIcon = $("search-icon");
  const bodyEl = $("body");
  const footerEl = $("footer");
  const powerBtnTop = $("power-btn-top");
  const powerMenuTop = $("power-menu-top");
  const powerConfirmTop = $("power-confirm-top");
  const powerConfirmTitleTop = $("power-confirm-title-top");
  const powerConfirmYesTop = $("power-confirm-yes-top");
  const powerPanelTop = $("power-panel-top");
  const contextMenu = $("context-menu");
  const completionEl = $("completion");
  const hintComplete = $("hint-complete");
  const updateNotice = $("update-notice");
  const updateBtn = $("update-btn");
  const updateBtnLabel = updateBtn.querySelector("span");
  const whatsNewTitle = $("whats-new-title");
  const whatsNewItems = $("whats-new-items");
  const whatsNewTips = $("whats-new-tips");
  const mediaArt = $("media-art");
  const mediaTitle = $("media-title");
  const mediaArtist = $("media-artist");
  const mediaProgressFill = $("media-progress-fill");
  const mediaProgressKnob = $("media-progress-knob");
  const mediaPos = $("media-pos");
  const mediaDur = $("media-dur");
  const mediaPlayIcon = $("media-play-icon");
  const mediaPauseIcon = $("media-pause-icon");
  const mediaVolumeInline = $("media-volume-inline");
  const mediaVolumeSlider = $("media-volume-slider");
  const mediaVolIcon = $("media-vol-icon");
  const mediaMuteIcon = $("media-mute-icon");
  const mediaDots = $("media-dots");
  const mediaLive = $("media-live");
  const chatView = $("chat-view");
  const chatScroll = $("chat-scroll");
  const chatMessagesEl = $("chat-messages");
  const chatEmpty = $("chat-empty");
  const chatInput = $("chat-input");
  const chatSendButton = $("chat-send-button");
  const chatNotice = $("chat-notice");
  const chatLiveStatus = $("chat-live-status");
  const chatSettings = $("chat-settings");
  const chatProviderInput = $("chat-provider");
  const chatProviderButton = $("chat-provider-button");
  const chatProviderChoice = $("chat-provider-choice");
  const chatProviderOptions = $("chat-provider-options");
  const chatModelInput = $("chat-model");
  const chatModelOptions = $("chat-model-options");
  const chatModelSearch = $("chat-model-search");
  const chatModelResults = $("chat-model-results");
  const chatBaseUrlInput = $("chat-base-url");
  const chatApiKeyInput = $("chat-api-key");
  const chatEndpointField = $("chat-endpoint-field");
  const chatKeyField = $("chat-key-field");
  const chatConnectButton = $("chat-connect-button");
  const chatDisconnectButton = $("chat-disconnect-button");
  const chatCheckButton = $("chat-check-button");
  const chatConnectionHint = $("chat-connection-hint");
  const chatHistory = $("chat-history");
  const CHAT_HISTORY_KEY = "nex.chat.history.v1";
  let chatOpen = false;
  let chatReturnToMedia = false;
  let chatStreaming = false;
  let chatConfig = { provider: "openai-compatible", baseUrl: "https://api.openai.com/v1", model: "gpt-4o-mini", configured: false, accountConnected: false };
  let chatModels = [];
  let chatModelsProvider = "";
  let chatModelsLoading = false;
  let chatModelsTimer = 0;
  let chatConnectionTimer = 0;
  let chatConnectTimer = 0;
  let chatDisconnectTimer = 0;
  let chatConnectionCheckPending = false;
  let chatConversations = loadChatConversations();
  let chatConversationId = "";
  let chatMessages = [];
  let chatRenderFrame = 0;
  let chatStepSeq = 0;
  let chatStreamMessageId = "";
  let chatStreamOffset = 0;
  let chatRecognition = null;
  let chatAutoSendVoice = false;
  let chatVoiceTranscript = "";

  // Now-playing media state pushed by Rust ({active,title,artist,...}).
  // Tab opens the media view only when a session is active; otherwise
  // Tab keeps its normal input-select behavior.
  let mediaState = null;
  let mediaOpen = false;
  let mediaRefreshTimer = 0;

  // Post-update What's New: Rust pushes `whatsNewPending` ("x.y.z") once
  // per installed version. While set, the update notice opens the
  // dedicated view instead of running an update check.
  let updateAvailable = false;
  let whatsNewPending = null;
  let whatsNewOpen = false;
  let whatsNewContent = null;

  // Local mirror of pushed state.
  let rows = [];
  // Rows snapshot for event handlers on reused nodes (they must read the
  // CURRENT row for the live index, not the row captured at creation).
  let currentRows = [];
  let selected = 0;
  let queryEcho = ""; // last query Rust pushed back (avoid input clobber)
  let lastQuerySent = "";
  let inCommandMode = false;
  let completion = ""; // Rust-pushed command-mode autofill title
  let pushedPlaceholder = ""; // last placeholder Rust pushed (null-safe)
  let rowMap = new Map(); // index → HTMLElement for O(1) selection toggle
  let lastRowSig = ""; // content signature — gates entrance stagger re-animation
  let hasAnimatedFirstShow = false; // stagger only fires for the first rows ever shown
  let quickLaunchItems = []; // Quick Launch items for idle state
  let pendingShow = false; // show occurred, waiting for first real results
  // Last cursor position — lets a re-render keep the selection under a
  // stationary mouse (hover only fires on mousemove).
  let lastMouseX = -1;
  let lastMouseY = -1;
  document.addEventListener(
    "mousemove",
    (e) => {
      lastMouseX = e.clientX;
      lastMouseY = e.clientY;
    },
    { passive: true }
  );

  // Blank panel space still focuses the search box, but never moves the
  // overlay. Interactive controls keep their own mouse behavior.
  const FOCUS_BLOCKED = "input, button, textarea, select, .row, [role='button'], #context-menu, #media-progress, .power-panel, .power-confirm";
  panel.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    if (!contextMenu.classList.contains("hidden") && !e.target.closest("#context-menu")) {
      hideContextMenu();
    }
    if (e.target.closest(FOCUS_BLOCKED) || e.target.isContentEditable) return;
    e.preventDefault();
    input.focus();
    input.select();
  });

  // Persistent icon cache — survives DOM rebuilds across state pushes.
  // Key: icon path (string), Value: data URI (string).
  const iconCache = new Map();
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Themed fallback shown while real icon loads (cold cache).
  // 128×128 app icons, base64-encoded PNGs.
  const PLACEHOLDER_ICON_LIGHT = "data:image/png;base64,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";
  const PLACEHOLDER_ICON_DARK = "data:image/png;base64,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";
  const FOLDER_ICON = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAIAAAACACAYAAADDPmHLAAAABHNCSVQICAgIfAhkiAAAAAlwSFlzAAADsQAAA7EB9YPtSQAAABl0RVh0U29mdHdhcmUAd3d3Lmlua3NjYXBlLm9yZ5vuPBoAAAOpSURBVHic7dhPa1xlGIbx63nndKympaQqiTaLWkNsaRrE9nOYSBpcCC5c6F6FgAspFMQ2rl24cuFC6miTqIjfoUEw/tmEuqmhG0VqoCaZnMfFOFWa2ARNzjtznvu3mhnO4h7mmjMvAyIiIiIiIiIiIiIiIiJST7bTi/79THOr5AXcJ914zmAEGKh4Wwn8ZvCTwzck+7rxyMAX9tSHf1S8o9a2BdD+bnoat6vAqQx7dvOLG+8XxcacnV78PfeYOrgXgF+babTP+BWDN3IO2qPbOK8UE62vcg/pd6n7oI8+fIBhjC83ly9eyj2k3xncu+23co/5L8x5uzHRupx7R7+yzoHPf6Q3f/P3wsFfLs59+lHuIf3I2sszL4J/nHvI/+Il6ecVbONu7iW9ZA24BSyBL3D72Ly9trR5/0UJfKr6bfvMEuXwSWgUuZf0kiPAaeAlsGsM3/nBPx/b9lknh/PVbzsARZNy6CTYjn9tCIxS+nWfH3vXL/19+E8GT+RctZ/88ADl8dq8nYNhPsuzY+90nyY6t4ra8GOP40eP557R28xnfXF0EsDayxc995595yVpdQVb16HwAW6y3jyTdr+uD1nqnAd0KHyQUzTXn69nAKBD4V5YmqxvAOhQuDu/UOsAQIfCXZyofQAA5WMn8Icezj2jFx0NEYAOhf8uRgDQORQ++TQUzdxLekqcAAA/dJitkWcoB4c7PwkW6u3vKN49MSV8cAgfHMq9pCfoKxCcAghOAQSnAIJTAMEpgOAUQHAKIDgFEJwCCE4BBKcAglMAwSmA4BRAcAogOAUQnAIITgEEpwCCUwDBKYDgFEBwCiA4BRCcAghOAQSnAIJTAMEpgOAUQHAKIDgFEJwCCE4BBKcAglMAwSmA4BRAcAogOAUQnAIITgEEpwCCUwDBKYDgFEBwCiA4BRCcAghOAQSnAIJTAMEpgOAUQHAKIDgFEJwCCE4BBKcAglMAwSmA4BRAcAogOAUQnAIITgEEpwCCUwDBJWAt9wjJ5k5y89XcKyQPh9VkpS3lHiJ5GNxImM/nHiKZJFtIjfVHrwMrubdI5W42YDHZhQ82gdnca6Rizut29pONBFCca33m8F7uTVINN64UE60F+Mf/AMX4+Kzjc/lmSRXcuVqcHX+r+9zuv6D97fQUZnPAaKXL5KCt4LzZ/eZ3bQsAwG+8emir+esUxqTDeYMR4EglM2W/rDncMlgi2Xzj7uDCX+c9EREREREREREREREREQnhTy52wK21rlFtAAAAAElFTkSuQmCC";
  const FILE_PLACEHOLDER_ICON = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAMAAACdt4HsAAAAA3NCSVQICAjb4U/gAAAACXBIWXMAAAG7AAABuwE67OPiAAAAGXRFWHRTb2Z0d2FyZQB3d3cuaW5rc2NhcGUub3Jnm+48GgAAAEtQTFRF////3ubv09zp4ufws7/Lz9rm4ufv4ufwy9fky9bknay6oa+9pLK/sLzJtcDNydXjztnmz9nm09zo1d7p2uDp3OPt3ePt3uTu4ufw0km01gAAAAp0Uk5TAB86p8Dh5vD9/i9D6pkAAADNSURBVFjD7dfLEsIgDAVQKhYrSlHrI///pS4c+7LkRjLVDXefMxMumxjTp7KOktkamGpHTDwWLLEAFhwAoEAIQAIGgCAAeEECsIII4AQZwAhCIC1IgaTAA0cs8EDrocAD1wMUCAgt2oJgujgkCxgLecBIyAQGAQLBzxKmQjbwFnJX6AUF8BI0AHVagC5a4KEFaF3g4wcs/IZ1gf+/QWmhtFBaKC2UFphHDL8GSgsKwMnn77H5+vSdzJ9jnTi+b6coyn6zeL5bJ5tv6tH8Ezzc5sExY4ClAAAAAElFTkSuQmCC";
  function folderIcon() { return FOLDER_ICON; }
  // "Show all apps" entry icon — apps grid with a plus, neutral gray.
  const SHOW_ALL_APPS_ICON = "data:image/svg+xml," + encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#8a8a93" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><path d="M17.5 14v7M14 17.5h7"/></svg>`
  );
  // Transparent 1px GIF — cold-cache icon slot. Avoids flashing a wrong
  // placeholder glyph; patchIcons() pops the real icon in when decoded.
  const BLANK_ICON = "data:image/gif;base64,R0lGODlhAQABAAAAACwAAAAAAQABAAA=";
  function filePlaceholderIcon() { return FILE_PLACEHOLDER_ICON; }
  function placeholderIcon() {
    return document.documentElement.dataset.theme === "light" ? PLACEHOLDER_ICON_DARK : PLACEHOLDER_ICON_LIGHT;
  }

  const WEB_ICON_LIGHT = "data:image/png;base64,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";
  const WEB_ICON_DARK = "data:image/png;base64,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";
  function webIcon() {
    return document.documentElement.dataset.theme === "light" ? WEB_ICON_DARK : WEB_ICON_LIGHT;
  }

  const FLAT_IPC_PAYLOADS = new Set(["chatConfigure", "chatFetchModels", "chatSend", "agentGoal", "agentApprove", "agentDeny"]);
  function post(t, v) {
    try {
      const message = v === undefined ? { t } : FLAT_IPC_PAYLOADS.has(t) ? { t, ...v } : { t, v };
      window.ipc.postMessage(JSON.stringify(message));
    } catch (_) {}
  }

  // Receive state from Rust via WebView2 PostWebMessageAsJson
  // (fire-and-forget, never blocks the host event loop). The
  // WebView2 runtime already parsed the JSON — e.data is a JS object.
  if (window.chrome?.webview) {
    window.chrome.webview.addEventListener("message", (e) => {
      try { nex.apply(e.data); } catch (_) {}
    });
  }

  // ── toast notification ────────────────────────────────────
  // ── pin/unpin icons ────────────────────────────────────────
  const pinIconSvg = `<svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="var(--text-faint)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 3L5 15L9 11L13 15L13 3Z"/></svg>`;
  const pinIconPinnedSvg = `<svg width="18" height="18" viewBox="0 0 18 18" fill="var(--accent)" stroke="none"><path d="M5 3L5 15L9 11L13 15L13 3Z"/></svg>`;
  const addIconSvg = `<svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="var(--text-faint)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 3L5 15L9 11L13 15L13 3Z"/></svg>`;

  function createPinIcon(item, index) {
    const pinIcon = document.createElement('div');
    pinIcon.className = 'pin-icon' + (item.pinned ? ' pinned' : '');
    pinIcon.innerHTML = item.pinned ? pinIconPinnedSvg : pinIconSvg;
    pinIcon.addEventListener('click', (e) => {
      e.stopPropagation();
      e.preventDefault();
      const target = item.url || item.path || item.title;
      if (item.pinned) {
        post('unpin', target);
      } else {
        post('pin', target);
      }
      input.focus();
    });
    return pinIcon;
  }

  function isItemPinned(filePath) {
    if (!filePath) return false;
    const normalized = normalizePinTarget(filePath);
    return quickLaunchItems.some(item => {
      const itemPath = normalizePinTarget(item.url || item.path || '');
      return itemPath === normalized && item.pinned;
    });
  }

  function normalizePinTarget(target) {
    const value = String(target || '').trim();
    if (/^https?:\/\//i.test(value)) {
      try { return new URL(value).toString().toLowerCase(); } catch (_) {}
    }
    return value.replace(/\\/g, '/').toLowerCase();
  }

  function createAddIcon(item) {
    const addIcon = document.createElement('div');
    const target = item.url || item.filePath || item.icon;
    const pinned = isItemPinned(target);
    addIcon.className = 'add-icon' + (pinned ? ' pinned' : '');
    addIcon.innerHTML = pinned ? pinIconPinnedSvg : addIconSvg;
    addIcon.addEventListener('click', (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (target) {
        if (pinned) {
          post('unpin', target);
        } else if (item.url) {
          post('pin', target);
        } else {
          post('addToQuickLaunch', target);
        }
      }
      input.focus();
    });
    return addIcon;
  }

  // ── render ───────────────────────────────────────────────
  function selectableIndices() {
    const out = [];
    rows.forEach((r, i) => {
      if (r.selectable) out.push(i);
    });
    return out;
  }

  function clampSelected() {
    const sel = selectableIndices();
    if (sel.length === 0) {
      selected = -1;
      return;
    }
    if (!sel.includes(selected)) selected = sel[0];
  }

  // Stable identity for a row. Includes icon path so same-title rows with
  // different targets (e.g. two setup.exe) never collide.
  function rowKey(r) {
    return `${r.role || ""}|${r.kind || ""}|${r.title || ""}|${r.subtitle || ""}|${r.icon || ""}`;
  }

  function rowClassName(r, isGridView) {
    if (r.role === "clipboard_history") {
      const size = r.tileSize || "small";
      return "row clipboard-tile tile-" + size;
    }
    return "row" + (r.role === "calculator" ? " calculator" : "") + (r.role === "quick_launch" ? " quick-launch" : "") + (isGridView ? ((r.kind === "app" || r.role === "quick_launch" || r.role === "show_all_apps") ? " row-grid" : " row-list") : "");
  }

  function buildSection(key, r) {
    const li = document.createElement("li");
    li.className = "section";
    li.dataset.key = key;
    li.textContent = r.title;
    if (r.role === "status") {
      li.style.textTransform = "none";
      li.style.color = "var(--text-faint)";
    }
    return li;
  }

  // Pin/add/kind trailing node. Refreshed on reuse because pinned state can
  // change while the row identity stays the same.
  function appendTrailing(li, r, i) {
    if (r.role === "quick_launch") {
      const quickLaunchItem = quickLaunchItems.find(item => item.title === r.title);
      if (quickLaunchItem) {
        li.appendChild(createPinIcon(quickLaunchItem, i));
      }
    } else if ((r.kind === "app" || isWebResult(r)) && r.role !== "calculator") {
      li.appendChild(createAddIcon(r));
    } else if (r.kind && r.role !== "calculator") {
      const kind = document.createElement("div");
      kind.className = "kind";
      kind.textContent = r.kind;
      li.appendChild(kind);
    }
  }

  function buildRow(key, r, i, isGridView, animDelay) {
    const li = document.createElement("li");
    li.className = rowClassName(r, isGridView);
    if (animDelay) li.style.animationDelay = animDelay;
    li.setAttribute("role", "option");
    li.id = `row-${i}`;
    li.dataset.key = key;
    li.dataset.index = String(i);
    if (i === selected) {
      li.classList.add("selected");
      li.setAttribute("aria-selected", "true");
    } else {
      li.setAttribute("aria-selected", "false");
    }

    // Clipboard history tile (image or text) — clicking copies the
    // entry to the system clipboard, exactly like Enter on the row.
    if (r.role === "clipboard_history") {
      if (r.clipboardThumbnail) {
        const img = document.createElement("img");
        img.className = "tile-image";
        img.src = r.clipboardThumbnail;
        li.appendChild(img);
      } else {
        const content = document.createElement("div");
        content.className = "tile-content";
        content.textContent = r.title;
        li.appendChild(content);
      }
      li.addEventListener("click", () => {
        const idx = Number(li.dataset.index);
        setSelected(idx, false);
        post("submit", idx);
      });
      li.addEventListener("mousemove", () => setSelected(Number(li.dataset.index), false));
      return li;
    }

    if (r.role !== "calculator") {
      if (r.role === "show_all_apps") {
        const img = document.createElement("img");
        img.className = "icon";
        img.src = SHOW_ALL_APPS_ICON;
        li.appendChild(img);
      } else if (r.kind === "folder") {
        const img = document.createElement("img");
        img.className = "icon";
        img.src = folderIcon();
        li.appendChild(img);
      } else if (isWebResult(r)) {
        const img = document.createElement("img");
        img.className = "icon";
        if (r.icon) {
          img.dataset.iconPath = r.icon;
          img.src = iconCache.get(r.icon) || webIcon();
        } else {
          img.src = webIcon();
        }
        img.onerror = () => { img.src = webIcon(); };
        li.appendChild(img);
      } else if (r.icon && r.kind !== "action") {
        const img = document.createElement("img");
        img.className = "icon" + (r.kind === "settings" ? " glyph" : "");
        img.dataset.iconPath = r.icon; // store path for patchIcons()
        if (iconCache.has(r.icon)) {
          img.src = iconCache.get(r.icon);
        } else {
          // Cold cache: blank slot, no wrong-glyph flash. patchIcons()
          // pops the real icon in (with pop-in) once decoded.
          img.src = BLANK_ICON;
        }
        // Don't add placeholder class here — patchIcons() will set
        // src and the browser handles loading. Only onerror triggers
        // placeholder.
        img.onerror = () => { if (r.kind === "file") { img.src = filePlaceholderIcon(); } else { img.classList.add("placeholder"); } };
        li.appendChild(img);
      } else if (r.kind !== "action") {
        const ph = document.createElement("div");
        ph.className = "icon placeholder";
        li.appendChild(ph);
      }
      // Web search row — use themed web icon
      if (r.kind === "action" && r.title && r.title.startsWith("Search Web for")) {
        const img = document.createElement("img");
        img.className = "icon";
        img.src = webIcon();
        li.appendChild(img);
      }
    }

    const text = document.createElement("div");
    text.className = "text";
    const title = document.createElement("div");
    title.className = "title";
    title.textContent = r.title;
    text.appendChild(title);
    if (r.subtitle && !r.url) {
      const sub = document.createElement("div");
      sub.className = "subtitle";
      sub.textContent = r.subtitle;
      text.appendChild(sub);
    }
    li.appendChild(text);

    appendTrailing(li, r, i);

    // Handlers read the live index + current row so reused nodes always
    // act on the row they currently render (not the one from creation).
    li.addEventListener("mousemove", () => setSelected(Number(li.dataset.index), false));
    li.addEventListener("click", () => {
      const idx = Number(li.dataset.index);
      setSelected(idx, false);
      post("submit", idx);
    });
    li.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      const idx = Number(li.dataset.index);
      setSelected(idx, false);
      showContextMenu(e.clientX, e.clientY, currentRows[idx]);
    });
    return li;
  }

  function render() {
    clampSelected();
    currentRows = rows;
    const frag = document.createDocumentFragment();
    const isGridView = list.classList.contains("grid-view");
    let animIdx = 0;
    // Entrance stagger: animate only the very first rows shown (initial
    // show); later keystrokes render instantly. Selection-only
    // re-renders (same sig) never animate.
    const sig = rows
      .map((r) => `${r.role || ""}|${r.kind || ""}|${r.title || ""}|${r.subtitle || ""}`)
      .join(";");
    const contentChanged = sig !== lastRowSig;
    const animating = !hasAnimatedFirstShow && contentChanged;
    if (sig) hasAnimatedFirstShow = true;
    lastRowSig = sig;
    // Kill the CSS entrance animation for non-first renders — fresh nodes
    // would otherwise re-animate on every keystroke.
    list.classList.toggle("no-anim", !animating);

    // Index live nodes by key so unchanged rows are reused in place —
    // no teardown, no image re-decode, no entrance re-animation.
    const live = new Map();
    for (const li of list.children) {
      const k = li.dataset.key;
      if (k && !live.has(k)) live.set(k, li);
    }

    const usedKeys = new Set();
    for (let i = 0; i < rows.length; i++) {
      const r = rows[i];
      const key = rowKey(r);
      const isSection = r.role === "header" || r.role === "status";
      let li = null;
      if (!usedKeys.has(key)) {
        li = live.get(key) || null;
      }
      if (li) {
        // Reuse.
        usedKeys.add(key);
        live.delete(key);
        if (!isSection) {
          // Refresh only what can change: layout class, live index,
          // selection, and the trailing pin/add/kind node. Text and
          // icon are identical by key — leave them untouched.
          li.className = rowClassName(r, isGridView);
          li.dataset.index = String(i);
           li.classList.toggle("selected", i === selected);
           li.setAttribute("aria-selected", i === selected ? "true" : "false");
          const trailing = li.querySelector(".pin-icon, .add-icon, .kind");
          if (trailing) trailing.remove();
          appendTrailing(li, r, i);
        }
      } else {
        // New row.
        usedKeys.add(key);
        if (isSection) {
          li = buildSection(key, r);
        } else {
          const delay = (animating && r.role !== "status" && r.role !== "quick_launch")
            ? Math.min(animIdx++, 8) * 7 + "ms"
            : undefined;
          li = buildRow(key, r, i, isGridView, delay);
        }
      }
      frag.appendChild(li);
    }

    // Rows that no longer exist.
    for (const li of live.values()) li.remove();

    // Atomic swap — reused nodes just move, new ones are added.
    list.replaceChildren(frag);

    // New result set → always start scrolled to the top; keeping the
    // previous query's scroll offset reads as broken rendering.
    if (contentChanged) list.scrollTop = 0;

    // Rebuild row map for O(1) selection toggles.
    rowMap = new Map();
    for (const li of list.children) {
      if (li.classList.contains("row")) rowMap.set(Number(li.dataset.index), li);
    }

    // The swap applied the pushed selected=0, but row hover only updates
    // on mousemove — re-select whatever row sits under the stationary
    // cursor so the highlight doesn't jump to the top. Must run AFTER the
    // rowMap rebuild above, or setSelected resolves stale nodes.
    // When the pointer rests over a gap (grid gutters, headers), snap to
    // the nearest row vertically — otherwise the stale row-0 selection
    // stays visible until the mouse moves.
    if (contentChanged && lastMouseX >= 0) {
      let target = null;
      const el = document.elementFromPoint(lastMouseX, lastMouseY);
      const hovered = el && el.closest ? el.closest(".row") : null;
      if (hovered) {
        target = Number(hovered.dataset.index);
      } else {
        const lr = list.getBoundingClientRect();
        if (lastMouseY >= lr.top && lastMouseY <= lr.bottom) {
          let bestLi = null;
          let bestDy = Infinity;
          for (const li of list.children) {
            if (!li.classList.contains("row")) continue;
            const r = li.getBoundingClientRect();
            if (r.height === 0) continue;
            const dy = Math.abs(r.top + r.height / 2 - lastMouseY);
            if (dy < bestDy) {
              bestDy = dy;
              bestLi = li;
            }
          }
          if (bestLi) target = Number(bestLi.dataset.index);
        }
      }
      if (target !== null && target !== selected) setSelected(target, false);
    }

    // Status / empty state.
    const hasRows = rows.some((r) => r.role !== "status");
    const hasStatusRows = rows.some((r) => r.role === "status");
    if (!hasRows && statusEl.dataset.text) {
      statusEl.textContent = statusEl.dataset.text;
      statusEl.classList.remove("hidden");
    } else {
      statusEl.classList.add("hidden");
    }

    // Idle state: hide divider + list area and footer when no rows.
    // Keep the body visible when status rows are present (e.g.
    // "Clipboard history is empty") so the message is actually rendered.
    bodyEl.classList.toggle("idle", !hasRows && !hasStatusRows);
    // Footer hints only with regular results — hidden in the idle
    // window and in the quick-launch (pinned items) view.
    const qlOnly =
      hasRows && rows.every((r) => r.role === "quick_launch" || r.role === "header" || r.role === "status");
    footerEl.classList.toggle("idle", hasRows && !qlOnly);
    footerEl.classList.toggle("ql", hasRows && qlOnly);
    footerEl.classList.toggle(
      "list-results",
      hasRows && !qlOnly && !list.classList.contains("grid-view") && !list.classList.contains("bento-view")
    );
    list.classList.toggle(
      "list-results",
      hasRows && !qlOnly && !list.classList.contains("grid-view") && !list.classList.contains("bento-view")
    );
    list.classList.toggle(
      "grid-results",
      hasRows && !qlOnly && list.classList.contains("grid-view") && !list.classList.contains("bento-view")
    );

    // Menu tied to a hidden area must close (its trigger vanished).
    if (!hasRows) topPower.closeMenu();

    measure();
  }

  function setSelected(i, scroll) {
    if (i === selected) {
      if (scroll) scrollToSelected();
      return;
    }
    selected = i;
    for (const row of rowMap.values()) {
      row.classList.remove("selected");
      row.setAttribute("aria-selected", "false");
    }
    const nextEl = rowMap.get(selected);
    if (nextEl) {
      nextEl.classList.add("selected");
      nextEl.setAttribute("aria-selected", "true");
      list.setAttribute("aria-activedescendant", nextEl.id || `row-${nextEl.dataset.index}`);
      if (!nextEl.id) nextEl.id = `row-${nextEl.dataset.index}`;
    } else {
      list.removeAttribute("aria-activedescendant");
    }
    if (scroll) scrollToSelected();
    post("select", selected);
  }

  // Helper: set scrollTop but bypass CSS scroll-behavior (smooth)
  // so the reset is instant, while user-initiated scrolls stay smooth.
  function scrollToInstant(y) {
    const prev = list.style.scrollBehavior;
    list.style.scrollBehavior = "auto";
    list.scrollTop = y;
    // Restore after a microtask — the scroll has already been applied.
    requestAnimationFrame(() => { list.style.scrollBehavior = prev; });
  }

  function scrollToSelected() {
    const el = rowMap.get(selected);
    if (!el) return;
    const top = el.offsetTop;
    const bot = top + el.offsetHeight;
    if (top < list.scrollTop || bot > list.scrollTop + list.clientHeight) {
      el.scrollIntoView({ block: "nearest" });
    }
  }

  function moveSelection(delta) {
    const sel = selectableIndices();
    if (sel.length === 0) return;
    let pos = sel.indexOf(selected);
    if (pos === -1) pos = 0;
    else pos = Math.min(sel.length - 1, Math.max(0, pos + delta));
    setSelected(sel[pos], true);
  }

  // Grid row grouping + current-position search (shared by vertical and
  // horizontal grid navigation).  Returns { rows, cr, cc } where rows
  // is an array of selectable-index arrays grouped by offsetTop, cr/cc
  // is the current selection's row/column, or null if not found.
  function gridRowsAndPos() {
    const sel = selectableIndices();
    if (sel.length === 0) return null;
    const gridRows = [];
    const placed = new Set();
    for (const idx of sel) {
      if (placed.has(idx)) continue;
      const el = rowMap.get(idx);
      if (!el) continue;
      const baseTop = el.offsetTop;
      const row = [];
      for (const otherIdx of sel) {
        if (placed.has(otherIdx)) continue;
        const otherEl = rowMap.get(otherIdx);
        if (otherEl && otherEl.offsetTop === baseTop) {
          row.push(otherIdx);
          placed.add(otherIdx);
        }
      }
      if (row.length > 0) gridRows.push(row);
    }
    let cr = -1, cc = -1;
    for (let r = 0; r < gridRows.length; r++) {
      const c = gridRows[r].indexOf(selected);
      if (c !== -1) { cr = r; cc = c; break; }
    }
    if (cr === -1) return null;
    return { rows: gridRows, cr, cc };
  }

  // Grid-aware navigation: vertical (dy) and horizontal (dx).
  // Horizontal uses row-wrapping: right past end → next row first col,
  // left before start → prev row last col. Stops at grid edges (no wrap-around).
  function moveSelectionGrid(dx, dy) {
    const info = gridRowsAndPos();
    if (!info) return;
    const { rows, cr, cc } = info;
    let tr = cr, tc = cc;
    if (dy !== 0) {
      tr = cr + dy;
      if (tr < 0 || tr >= rows.length) return;
      tc = Math.min(cc, rows[tr].length - 1);
    } else if (dx !== 0) {
      tc = cc + dx;
      if (dx > 0 && tc >= rows[cr].length) {
        tr = cr + 1;
        if (tr >= rows.length) return;
        tc = 0;
      } else if (dx < 0 && tc < 0) {
        tr = cr - 1;
        if (tr < 0) return;
        tc = rows[tr].length - 1;
      }
    }
    setSelected(rows[tr][tc], true);
  }

  // Grid-aware vertical navigation: jump to same column in prev/next row.
  function moveSelectionGridDown(dy) {
    moveSelectionGrid(0, dy);
  }

  // ── icon patching ─────────────────────────────────────────
  // Called after icon data arrives. Updates <img> elements from cache.
  // Does NOT skip placeholder elements — on cold cache, render() creates
  // icons without src, and patchIcons() must update them all.
  //
  // No pop-in animation here: assigning src to a data: URI decodes
  // synchronously, so the new pixels are already in place when the
  // swap is batched in a single rAF below. The old per-batch
  // `nexIconIn` restart (opacity 0 → 1) made every cold icon visibly
  // blink — twice when head + tail arrived as separate pushes.
  function patchIcons() {
    const pending = [];
    for (const li of list.children) {
      const img = li.querySelector("img.icon");
      if (!img) continue;
      if (img.src === folderIcon()) continue;
      const path = img.dataset.iconPath;
      if (path && iconCache.has(path)) {
        const dataUri = iconCache.get(path);
        if (img.src !== dataUri) pending.push([img, dataUri]);
      }
    }
    if (!pending.length) return;
    const apply = () => {
      for (const [img, dataUri] of pending) {
        if (img.isConnected && img.src !== dataUri) img.src = dataUri;
      }
    };
    if (typeof requestAnimationFrame === "function") {
      requestAnimationFrame(apply);
    } else {
      apply();
    }
  }

  // ── command mode ───────────────────────────────────────────
  function updateSearchIcon() {
    searchIcon.style.opacity = "0";
    setTimeout(() => {
      if (inCommandMode) {
        // stroke="none" is required: #search-icon carries fill:none +
        // stroke-width 2, which would outline-draw the @ glyph and make
        // it look thick/squashed.
        searchIcon.innerHTML =
          '<text x="12" y="10" font-size="20" font-weight="400" fill="var(--text-faint)" stroke="none" text-anchor="middle" dominant-baseline="central" font-family="InterVariable, Inter, system-ui, -apple-system, sans-serif">@</text>';
      } else {
        searchIcon.innerHTML =
          '<circle cx="11" cy="11" r="7" fill="none" stroke="var(--text-faint)" stroke-width="2" stroke-linecap="round"></circle><line x1="21" y1="21" x2="16.65" y2="16.65" stroke="var(--text-faint)" stroke-width="2" stroke-linecap="round"></line>';
      }
      searchIcon.style.opacity = "1";
    }, 130);
  }

  // Measure text width using a reusable canvas context.
  const _measureCanvas = document.createElement("canvas");
  const _measureCtx = _measureCanvas.getContext("2d");
  function measureTextWidth(text) {
    const cs = getComputedStyle(input);
    _measureCtx.font = cs.fontWeight + " " + cs.fontSize + " " + cs.fontFamily;
    return _measureCtx.measureText(text).width;
  }

  // ── command autofill (dim remainder + Tab completion) ──────
  // Rust pushes `completion` = the full title of the best matching
  // command. Only the untyped remainder is rendered dimmed inside the
  // input; Tab fills the full title in. Both are gated on the typed
  // text being a case-insensitive prefix of the completion, so a stale
  // completion can never clobber what the user actually typed.
  //
  // This also owns the input placeholder — one source of truth so the
  // mode-specific text can't race with Rust state pushes. While a
  // completion suffix is showing, the placeholder is cleared entirely
  // (the suffix replaces it); otherwise command mode gets its own hint
  // and normal mode uses the pushed placeholder or the default.
  function renderCompletion() {
    const showSuffix =
      !!completion &&
      completion.toLowerCase().startsWith(input.value.toLowerCase()) &&
      input.value.length < completion.length;
    if (!showSuffix) {
      completionEl.textContent = "";
      hintComplete.classList.add("hidden");
      input.placeholder = inCommandMode
        ? "Type a command…"
        : pushedPlaceholder || "Search for apps, files and actions…";
      return;
    }
    completionEl.textContent = completion.slice(input.value.length);
    // Position the suffix right after the typed text so it reads as a
    // continuous hint — measure the typed width and offset the element.
    const typedWidth = measureTextWidth(input.value);
    completionEl.style.left = typedWidth + "px";
    completionEl.style.transform =
      `translateY(calc(-50% - 1px)) translateX(${-input.scrollLeft}px)`;
    hintComplete.classList.remove("hidden");
    input.placeholder = "";
  }

  function tryCompleteCommand() {
    if (!completion) return false;
    const typed = input.value;
    if (
      !completion.toLowerCase().startsWith(typed.toLowerCase()) ||
      typed.length >= completion.length
    ) {
      return false;
    }
    input.value = completion;
    queryEcho = completion;
    lastQuerySent = completion.startsWith("@") ? completion : "@" + completion;
    post("query", lastQuerySent);
    renderCompletion();
    return true;
  }

  // ── height measurement + painted notification ──
  // Sends resize IPC on first content paint so Rust expands the window
  // to match the panel's content height. The panel is already rendered
  // at full height (clipped by overflow:hidden) — no DWM acrylic flash.
  // The first measurement (idle, ~109px) records the height but does NOT
  // send resize — only the transition to real content triggers expansion.
  // All resizes post immediate:true (host applies synchronously); the
  // host keeps the WebView viewport oversized so reveals are pre-painted
  // and shrinks just clip. lastH dedupes repeated measurements.
  let lastH = 0;
  let needsPainted = false;
  // Holds the post-show reveal until first content paints (or 500ms),
  // so the window never flashes its idle mid-state before results land.
  let showRevealTimer = 0;
  function fireShowReveal() {
    showRevealTimer = 0;
    needsPainted = true;
    measure();
  }
  function measure() {
    const h = Math.ceil(panel.getBoundingClientRect().height);
    // Shrink path: apply immediately. The WebView viewport is pinned at
    // max height (host keeps it oversized), so shrinking just clips
    // already-rasterized content — no blank region, no acrylic gap.
    if (h > 0 && h < lastH) {
      lastH = h;
      post("resize", { v: h, immediate: true });
      if (needsPainted) {
        needsPainted = false;
        requestAnimationFrame(() => { scrollToInstant(0); post("painted"); });
      }
      return;
    }
    // Grow / equal path: defer two frames so newly rendered rows are
    // painted before the window reveals them.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const h = Math.ceil(panel.getBoundingClientRect().height);
        if (h > 0 && h !== lastH) {
          lastH = h;
          post("resize", { v: h, immediate: true });
        }
        if (needsPainted) {
          needsPainted = false;
          scrollToInstant(0); // fresh show = fresh scroll, after paint
          post("painted");
        }
      });
    });
  }

  // ── keyboard ─────────────────────────────────────────────
  window.addEventListener(
    "keydown",
    (e) => {
      if (chatOpen) {
        if (e.key === "Escape") {
          e.preventDefault();
          closeChatView(true);
          return;
        }
        if (e.key === "Tab") {
          e.preventDefault();
          closeChatView(false);
          input.focus();
          return;
        }
        if (e.target === chatInput && e.key === "Enter" && !e.shiftKey && !e.isComposing) {
          e.preventDefault();
          sendChatMessage();
        }
        return;
      }
      // Media view open: transport keys act on playback; typing
      // returns to search (the key lands in the refocused input).
      if (mediaOpen && !e.ctrlKey && !e.altKey && !e.metaKey) {
        if (e.key === "Tab") {
          e.preventDefault();
          openChatView(true);
          return;
        }
        if (e.key === " ") {
          e.preventDefault();
          post("mediaToggle");
          return;
        }
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          post("mediaPrev");
          return;
        }
        if (e.key === "ArrowRight") {
          e.preventDefault();
          post("mediaNext");
          return;
        }
        if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          nudgeVolume(e.key === "ArrowUp" ? 5 : -5);
          return;
        }
        if (e.key.length === 1) {
          closeMediaView();
          input.focus();
          return;
        }
      }
      // What's New open: typing returns to search (the key lands in the
      // refocused input), like the media view above.
      if (whatsNewOpen && !e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1) {
        closeWhatsNew();
        input.focus();
        return;
      }
      // ── command mode: `@` to enter (legacy `>` still accepted),
      // backspace-on-empty to exit ──
      if ((e.key === "@" || e.key === ">") && !inCommandMode && document.activeElement === input) {
        e.preventDefault();
        inCommandMode = true;
        input.value = "";
        queryEcho = "";
        updateSearchIcon();
        renderCompletion();
        post("query", "@");
        return;
      }
      if (e.key === "Backspace" && inCommandMode && input.value === "") {
        e.preventDefault();
        inCommandMode = false;
        updateSearchIcon();
        renderCompletion();
        post("query", "");
        return;
      }

      if (e.key === "ArrowDown" || (e.ctrlKey && (e.key === "j" || e.key === "J"))) {
        e.preventDefault();
        document.body.classList.add("keyboard-nav");
        if (list.classList.contains("grid-view") || list.classList.contains("bento-view")) {
          moveSelectionGridDown(1);
        } else {
          moveSelection(1);
        }
      } else if (e.key === "ArrowUp" || (e.ctrlKey && (e.key === "k" || e.key === "K"))) {
        e.preventDefault();
        document.body.classList.add("keyboard-nav");
        if (list.classList.contains("grid-view") || list.classList.contains("bento-view")) {
          moveSelectionGridDown(-1);
        } else {
          moveSelection(-1);
        }
      } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        document.body.classList.add("keyboard-nav");
        if (list.classList.contains("grid-view") || list.classList.contains("bento-view")) {
          moveSelectionGrid(e.key === "ArrowLeft" ? -1 : 1, 0);
        }
      } else if (e.key === "Enter") {
        e.preventDefault();
        if (selected >= 0) post("submit", selected);
      } else if (e.key === "Tab") {
        // Dedicated views open: Tab closes them and returns to search.
        if (mediaOpen) {
          e.preventDefault();
          closeMediaView();
          openChatView(true);
          return;
        }
        if (whatsNewOpen) {
          e.preventDefault();
          closeWhatsNew();
          return;
        }
        if (tryCompleteCommand()) {
          e.preventDefault();
          return;
        }
        // First Tab opens Media when available, then Chat. Without an
        // active media session, the first Tab opens Chat directly.
        if (mediaState && mediaState.active) {
          e.preventDefault();
          openMediaView();
          return;
        }
        e.preventDefault();
        openChatView(false);
        return;
        // Tab focuses the search input and selects its content so
        // typing replaces the existing query — no manual erase.
        e.preventDefault();
        input.focus();
        input.select();
      } else if (e.key === "Escape") {
        // Dedicated views open: Escape closes them locally, overlay stays.
        if (mediaOpen) {
          e.preventDefault();
          closeMediaView();
          input.focus();
          return;
        }
        if (whatsNewOpen) {
          e.preventDefault();
          closeWhatsNew();
          return;
        }
        if (topPower.hasConfirm()) {
          topPower.closeConfirm();
          input.focus();
          return;
        }
        if (topPower.isOpen()) {
          topPower.closeMenu();
          return;
        }
        e.preventDefault();
        post("escape");
      } else if (e.key === "Home" && e.ctrlKey) {
        e.preventDefault();
        const sel = selectableIndices();
        if (sel.length) setSelected(sel[0], true);
      } else if (e.key === "End" && e.ctrlKey) {
        e.preventDefault();
        const sel = selectableIndices();
        if (sel.length) setSelected(sel[sel.length - 1], true);
      }
    },
    true
  );

  // Any keyboard interaction abandons the row context menu. This covers
  // navigation and command-mode keys before they produce an input event.
  window.addEventListener("keydown", (e) => {
    if (!contextMenu.classList.contains("hidden") && e.key !== "Escape") {
      hideContextMenu();
    }
  }, true);

  // ── query input (adaptive debounce) ──────────────────────
  // First char of each typing burst fires immediately (0ms).
  // Subsequent rapid chars coalesce at 40ms so SearchWorker
  // drains stale requests from its mpsc channel.
  let debounce = null;
  let lastInputTime = 0;
  input.addEventListener("input", () => {
    if (!contextMenu.classList.contains("hidden")) hideContextMenu();
    let raw = input.value;
    // In command mode the `@` prefix is kept out of the display
    // input — keydown handles enter/exit, `input` just sends
    // the text content (paste / IME also land here).
    if (raw.startsWith("@") || raw.startsWith(">")) {
      inCommandMode = true;
      raw = raw.slice(1);
      input.value = raw;
    }
    const query = inCommandMode ? "@" + raw : raw;
    renderCompletion();
    if (raw === queryEcho && query === lastQuerySent) return;
    lastQuerySent = query;

    const now = performance.now();
    const delay = (now - lastInputTime > 300) ? 0 : 40;
    lastInputTime = now;
    clearTimeout(debounce);
    debounce = setTimeout(() => post("query", query), delay);
  });
  // The dim completion suffix must track the input's horizontal scroll
  // (selection drag / arrow keys scroll long queries).
  input.addEventListener("scroll", renderCompletion, { passive: true });

  // ── power panel (circle row) ────────────────────────────────
  // Factory wires one power button + in-flow panel with menu row +
  // confirm row. The panel lives in the document flow so measure()
  // grows/shrinks the overlay window with it.
  function makePowerUi(btn, panel, menu, confirm, title, yes) {
    let open = false;
    let confirmAction = null; // "shutdown" | "restart" | null
    const api = {
      closeMenu() {
        open = false;
        menu.classList.add("hidden");
        btn.classList.remove("open");
        panel.closest("#panel").classList.remove("power-menu-open");
        if (!confirmAction) panel.classList.add("hidden");
        input.focus();
        measure();
      },
      closeConfirm() {
        if (!confirmAction) return;
        confirmAction = null;
        confirm.classList.add("hidden");
        if (open) menu.classList.remove("hidden");
        else panel.classList.add("hidden");
        measure();
      },
      isOpen() { return open; },
      hasConfirm() { return confirmAction !== null; },
      isConfirmTarget(el) { return confirm.contains(el); },
      isMenuTarget(el) { return btn.contains(el) || panel.contains(el); },
    };

    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      if (api.hasConfirm()) {
        // Confirm panel is showing — the power icon dismisses it.
        api.closeConfirm();
        input.focus();
        return;
      }
      open = !open;
      panel.closest("#panel").classList.toggle("power-menu-open", open);
      panel.classList.toggle("hidden", !open);
      menu.classList.toggle("hidden", !open);
      btn.classList.toggle("open", open);
      measure();
    });

    menu.addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (!b) return;
      const action = b.dataset.power;
      if (!action) return;
      // Back button — close the menu and return to the overlay.
      if (action === "back") {
        e.stopPropagation();
        api.closeMenu();
        return;
      }
      // Destructive actions need an in-overlay confirm panel first.
      if (action === "shutdown" || action === "restart") {
        e.stopPropagation(); // keep this click from closing the panel we're about to open
        confirmAction = action;
        menu.classList.add("hidden");
        const isShutdown = action === "shutdown";
        title.textContent = isShutdown ? "Shut down now?" : "Restart now?";
        yes.querySelector("span").textContent = isShutdown ? "Shut Down" : "Restart";
        confirm.classList.remove("hidden");
        measure();
        return;
      }
      post("powerAction", action);
      api.closeMenu();
    });

    confirm.addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (!b) return;
      if (b.dataset.confirm === "yes") {
        const action = confirmAction;
        api.closeConfirm();
        post("powerAction", action);
      } else {
        api.closeConfirm();
        input.focus();
      }
    });

    return api;
  }

  const topPower = makePowerUi(powerBtnTop, powerPanelTop, powerMenuTop, powerConfirmTop, powerConfirmTitleTop, powerConfirmYesTop);

  // Close the panel / confirm when clicking anywhere outside
  document.addEventListener("click", (e) => {
    if (topPower.hasConfirm() && !topPower.isConfirmTarget(e.target)) topPower.closeConfirm();
    if (topPower.isOpen() && !topPower.isMenuTarget(e.target)) topPower.closeMenu();
    if (!contextMenu.classList.contains("hidden") && !contextMenu.contains(e.target)) {
      hideContextMenu();
    }
  });

  // ── context menu ──────────────────────────────────────────
  let ctxRow = null; // the row the context menu was opened on

  function isWebResult(row) {
    const target = row.url || row.filePath || row.icon || row.subtitle || "";
    return /^(https?:\/\/|ms-settings:|sysdm\.cpl$)/i.test(target)
      && (row.kind === "action" || row.kind === "bookmark" || row.role === "quick_launch");
  }

  function itemTarget(row) {
    if (row.role === "quick_launch" && /^(ms-settings:|sysdm\.cpl$)/i.test(row.subtitle || "")) {
      return row.subtitle;
    }
    return row.url || row.filePath || row.icon || row.subtitle || "";
  }

  // Capture before WebView2/Chromium can open its native menu. Row-local
  // handlers remain for selection, but this guarantees reused/child nodes
  // cannot leak the browser context menu.
  document.addEventListener("contextmenu", (e) => {
    const row = e.target.closest?.(".row");
    if (!row) return;
    e.preventDefault();
    e.stopPropagation();
    const idx = Number(row.dataset.index);
    setSelected(idx, false);
    showContextMenu(e.clientX, e.clientY, currentRows[idx]);
  }, true);

  function showContextMenu(x, y, row) {
    // Synthetic entry — no context actions.
    if (row.role === "show_all_apps") return;
    ctxRow = row;
    // Determine which actions are relevant
    const isWebAction = row.kind === "action" && isWebResult(row);
    const isApp = row.kind === "app" || row.role === "quick_launch" || (row.kind === "action" && !isWebAction && !row.title.startsWith("Search Web"));
    const isFile = row.kind === "file" || row.kind === "folder" || (row.subtitle && row.subtitle.length > 0 && row.kind !== "action");
    const isBookmark = row.kind === "bookmark";

    const el = contextMenu;
    const btns = el.querySelectorAll("button");

    // Show/hide buttons based on item kind
    btns.forEach(b => {
      const action = b.dataset.action;
      if (action === "open") b.classList.toggle("hidden", false);
      else if (action === "runas") b.classList.toggle("hidden", isWebAction || (!isApp && !isFile) || row.kind === "folder");
      else if (action === "openfolder") b.classList.toggle("hidden", !row.subtitle);
      else if (action === "copypath") b.classList.toggle("hidden", !row.subtitle);
      else if (action === "pin") {
        const target = itemTarget(row);
        const pinned = isItemPinned(target);
        b.textContent = pinned ? "Unpin from Quick Launch" : "Pin to Quick Launch";
        b.classList.toggle("hidden", row.kind !== "app" && !isWebAction && !isBookmark);
      }
      else if (action === "uninstall") b.classList.toggle("hidden", row.kind !== "app");
      if ((isBookmark || isWebAction) && action !== "open" && action !== "pin") b.classList.add("hidden");
    });

    // Hide dividers whose adjacent sections are empty (e.g. pin/uninstall
    // are app-only, so files/folders must not show trailing gaps).
    const pinVisible = el.querySelector('button[data-action="pin"]')?.classList.contains("hidden") === false;
    const uninstallVisible = el.querySelector('button[data-action="uninstall"]')?.classList.contains("hidden") === false;
    el.querySelector('hr[data-divider="1"]')?.classList.toggle("hidden", !pinVisible && !uninstallVisible);
    el.querySelector('hr[data-divider="2"]')?.classList.toggle("hidden", !uninstallVisible);

    // Temporarily show to measure actual layout, then position. Measuring
    // while `hidden` returns zero height and always places menu below cursor,
    // where short overlay windows clip it at footer.
    el.classList.remove("hidden");
    const viewportW = window.visualViewport?.width || window.innerWidth;
    // The host keeps WebView's viewport at MAX_HEIGHT for fast resizes, while
    // the native window is only as tall as the current panel.
    const panelRect = panel.getBoundingClientRect();
    const viewportH = Math.min(
      window.visualViewport?.height || window.innerHeight,
      panelRect.bottom,
    );
    const pad = 8;
    el.style.maxHeight = `${Math.max(0, viewportH - pad * 2)}px`;
    const menuW = el.offsetWidth || 180;
    const menuH = el.offsetHeight || 0;

    let left = x + pad;
    if (left + menuW > viewportW - pad) {
      left = x - menuW - pad;
    }
    const below = y + pad;
    const above = y - menuH - pad;
    const top = below + menuH <= viewportH - pad ? below : above;
    el.style.left = `${Math.max(pad, Math.min(left, viewportW - menuW - pad))}px`;
    el.style.top = `${Math.max(pad, Math.min(top, viewportH - menuH - pad))}px`;
  }

  function hideContextMenu() {
    contextMenu.classList.add("hidden");
    ctxRow = null;
  }

  contextMenu.addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b || !ctxRow) return;
    const action = b.dataset.action;
    if (!action) return;

    const title = ctxRow.title || "";
    const path = itemTarget(ctxRow);
    const pinned = isItemPinned(path);

    if (action === "open") {
      hideContextMenu();
      post("submit", selected);
    } else if (action === "pin") {
      hideContextMenu();
      if (pinned) {
        post("unpin", path || title);
      } else {
        post("pin", path || title);
      }
    } else {
      // All other actions sent to Rust
      hideContextMenu();
      post("contextAction", { action, title, path });
    }
    input.focus();
  });

  // Close context menu on Escape
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !contextMenu.classList.contains("hidden")) {
      hideContextMenu();
      input.focus();
    }
  }, true);

  // ── Chat view ─────────────────────────────────────────────
  function loadChatConversations() {
    try {
      const stored = JSON.parse(localStorage.getItem(CHAT_HISTORY_KEY) || "[]");
      return Array.isArray(stored) ? stored.filter((item) => item && typeof item.id === "string" && Array.isArray(item.messages)).slice(0, 30) : [];
    } catch (_) { return []; }
  }

  function persistChat() {
    if (!chatConversationId) return;
    const conversation = chatConversations.find((item) => item.id === chatConversationId);
    if (conversation) {
      conversation.messages = chatMessages.slice(-100);
      conversation.updatedAt = Date.now();
      conversation.provider = chatConfig.provider;
      conversation.model = chatConfig.model;
    }
    chatConversations = chatConversations.slice(0, 30);
    try { localStorage.setItem(CHAT_HISTORY_KEY, JSON.stringify(chatConversations)); } catch (_) {}
  }

  function newChatConversation() {
    if (chatStreaming) post("chatCancel");
    chatStreaming = false;
    chatConversationId = "chat-" + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
    chatMessages = [];
    chatConversations.unshift({ id: chatConversationId, title: "New conversation", messages: [], updatedAt: Date.now(), provider: chatConfig.provider, model: chatConfig.model });
    setChatHistoryOpen(false);
    chatHistory.replaceChildren();
    renderChatMessages();
    updateChatStreamingState();
    chatLiveStatus.textContent = "";
    chatNotice.textContent = "";
    chatNotice.classList.remove("error");
    persistChat();
    chatInput.focus();
    resizeChatInput();
    postChatResize();
  }

  function saveChatTitle() {
    const conversation = chatConversations.find((item) => item.id === chatConversationId);
    if (!conversation || conversation.title !== "New conversation") return;
    const first = chatMessages.find((message) => message.role === "user");
    if (first) conversation.title = first.content.trim().replace(/\s+/g, " ").slice(0, 54) || "New conversation";
  }

  function renderChatHistory() {
    chatHistory.replaceChildren();
    if (!chatConversations.length) {
      const empty = document.createElement("div");
      empty.className = "chat-history-empty";
      empty.textContent = "Your conversations will appear here.";
      chatHistory.appendChild(empty);
      return;
    }
    for (const conversation of chatConversations.slice().sort((a, b) => b.updatedAt - a.updatedAt)) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "chat-history-item" + (conversation.id === chatConversationId ? " active" : "");
      const title = document.createElement("strong");
      title.textContent = conversation.title || "New conversation";
      const date = document.createElement("span");
      date.className = "chat-date";
      date.textContent = new Date(conversation.updatedAt || Date.now()).toLocaleDateString();
      const del = document.createElement("button");
      del.type = "button";
      del.className = "chat-delete";
      del.title = "Delete conversation";
      del.setAttribute("aria-label", "Delete conversation");
      del.innerHTML = '<svg viewBox="0 0 24 24"><path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2m3 0-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/></svg>';
      del.addEventListener("click", (event) => {
        event.stopPropagation();
        deleteChatConversation(conversation.id);
      });
      button.append(title, date, del);
      button.addEventListener("click", () => {
        if (chatStreaming) post("chatCancel");
        chatConversationId = conversation.id;
        chatMessages = conversation.messages.slice(-100);
        chatConfig.provider = conversation.provider || chatConfig.provider;
        chatConfig.model = conversation.model || chatConfig.model;
        chatModelInput.value = chatConfig.model;
        setChatHistoryOpen(false);
        renderChatMessages();
        updateChatStreamingState(false);
        chatLiveStatus.textContent = "";
        chatNotice.textContent = "";
        chatNotice.classList.remove("error");
        chatInput.focus();
      });
      chatHistory.appendChild(button);
    }
  }

  function deleteChatConversation(id) {
    chatConversations = chatConversations.filter((item) => item.id !== id);
    if (id === chatConversationId) newChatConversation();
    else persistChat();
    renderChatHistory();
  }

  function appendChatInline(parent, value) {
    const token = /(`[^`]+`|\*\*[^*]+\*\*|\*[^*]+\*|\[[^\]]+\]\(https?:\/\/[^)\s]+\))/g;
    let start = 0;
    for (const match of value.matchAll(token)) {
      if (match.index > start) parent.appendChild(document.createTextNode(value.slice(start, match.index)));
      const raw = match[0];
      let node;
      if (raw.startsWith("`")) {
        node = document.createElement("code"); node.textContent = raw.slice(1, -1);
      } else if (raw.startsWith("**")) {
        node = document.createElement("strong"); node.textContent = raw.slice(2, -2);
      } else if (raw.startsWith("*")) {
        node = document.createElement("em"); node.textContent = raw.slice(1, -1);
      } else {
        const link = /^\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)$/.exec(raw);
        node = document.createElement("a");
        if (link) { node.textContent = link[1]; node.href = link[2]; node.target = "_blank"; node.rel = "noopener noreferrer"; }
        else node.textContent = raw;
      }
      parent.appendChild(node);
      start = match.index + raw.length;
    }
    if (start < value.length) parent.appendChild(document.createTextNode(value.slice(start)));
  }

  function chatTableCells(line) {
    return line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((cell) => cell.trim());
  }

  function renderChatMarkdown(target, markdown) {
    target.replaceChildren();
    const lines = String(markdown || "").replace(/\r/g, "").split("\n");
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];
      if (!line.trim()) { i++; continue; }
      if (line.trim().startsWith("```")) {
        const language = line.trim().slice(3).trim().slice(0, 24) || "Code";
        const source = [];
        i++;
        while (i < lines.length && !lines[i].trim().startsWith("```")) source.push(lines[i++]);
        if (i < lines.length) i++;
        const block = document.createElement("div"); block.className = "chat-code-block";
        const head = document.createElement("div"); head.className = "chat-code-header";
        const lang = document.createElement("span"); lang.textContent = language;
        const copy = document.createElement("button"); copy.type = "button"; copy.textContent = "Copy"; copy.title = "Copy code";
        copy.addEventListener("click", async () => {
          try { await navigator.clipboard.writeText(source.join("\n")); copy.textContent = "Copied"; setTimeout(() => { copy.textContent = "Copy"; }, 1100); }
          catch (_) { copy.textContent = "Unavailable"; }
        });
        head.append(lang, copy);
        const pre = document.createElement("pre"); const code = document.createElement("code"); code.textContent = source.join("\n"); pre.appendChild(code);
        block.append(head, pre); target.appendChild(block); continue;
      }
      if (i + 1 < lines.length && line.includes("|") && /^\s*\|?\s*:?-{3,}/.test(lines[i + 1])) {
        const table = document.createElement("table"); table.className = "chat-table";
        const wrap = document.createElement("div"); wrap.className = "chat-table-wrap";
        const header = document.createElement("tr");
        for (const cell of chatTableCells(line)) { const th = document.createElement("th"); appendChatInline(th, cell); header.appendChild(th); }
        const thead = document.createElement("thead"); thead.appendChild(header); table.appendChild(thead);
        i += 2;
        const body = document.createElement("tbody");
        while (i < lines.length && lines[i].includes("|")) {
          const row = document.createElement("tr");
          for (const cell of chatTableCells(lines[i])) { const td = document.createElement("td"); appendChatInline(td, cell); row.appendChild(td); }
          body.appendChild(row); i++;
        }
        table.appendChild(body); wrap.appendChild(table); target.appendChild(wrap); continue;
      }
      const heading = /^(#{1,3})\s+(.+)$/.exec(line);
      if (heading) {
        const node = document.createElement("h" + heading[1].length); appendChatInline(node, heading[2]); target.appendChild(node); i++; continue;
      }
      if (/^\s*>/.test(line)) {
        const quote = document.createElement("blockquote");
        while (i < lines.length && /^\s*>/.test(lines[i])) { const p = document.createElement("p"); appendChatInline(p, lines[i++].replace(/^\s*>\s?/, "")); quote.appendChild(p); }
        target.appendChild(quote); continue;
      }
      const listMatch = /^\s*([-*+] |\d+\. )(.*)$/.exec(line);
      if (listMatch) {
        const ordered = /^\s*\d+\./.test(line); const listNode = document.createElement(ordered ? "ol" : "ul");
        while (i < lines.length) {
          const item = /^\s*([-*+] |\d+\. )(.*)$/.exec(lines[i]);
          if (!item || /^\s*\d+\./.test(lines[i]) !== ordered) break;
          const li = document.createElement("li"); appendChatInline(li, item[2]); listNode.appendChild(li); i++;
        }
        target.appendChild(listNode); continue;
      }
      const paragraph = document.createElement("p");
      const paragraphLines = [];
      while (i < lines.length && lines[i].trim() && !lines[i].trim().startsWith("```") && !/^(#{1,3})\s/.test(lines[i]) && !/^\s*>/.test(lines[i]) && !/^\s*([-*+] |\d+\. )/.test(lines[i])) paragraphLines.push(lines[i++]);
      appendChatInline(paragraph, paragraphLines.join("\n")); target.appendChild(paragraph);
    }
  }

  function renderChatMessages(streamOnly = false) {
    const atBottom = chatScroll.scrollHeight - chatScroll.scrollTop - chatScroll.clientHeight < 42;
    chatEmpty.hidden = chatMessages.length > 0;
    if (!streamOnly) {
      chatStreamMessageId = "";
      chatStreamOffset = 0;
      chatMessagesEl.replaceChildren();
      for (const message of chatMessages) {
        if (message.role === "step") {
          const article = document.createElement("article");
          article.className = "agent-" + (message.kind === "done" ? "done" : "step");
          article.dataset.messageId = message.id;
          article.textContent = message.content;
          chatMessagesEl.appendChild(article);
          continue;
        }
        const article = document.createElement("article");
        article.className = "chat-message " + (message.role === "user" ? "user" : "assistant") + (message.error ? " error" : "");
        article.dataset.messageId = message.id;
        const content = document.createElement("div"); content.className = "chat-message-content";
        renderChatMarkdown(content, message.content);
        article.appendChild(content); chatMessagesEl.appendChild(article);
        if (message.role === "assistant" && message.content && !message.error) {
          const actions = document.createElement("div"); actions.className = "chat-message-actions";
          const copy = document.createElement("button"); copy.type = "button"; copy.textContent = "Copy"; copy.title = "Copy response";
          copy.addEventListener("click", async () => {
            try { await navigator.clipboard.writeText(message.content); copy.textContent = "Copied"; setTimeout(() => { copy.textContent = "Copy"; }, 1100); }
            catch (_) { copy.textContent = "Unavailable"; }
          });
          actions.appendChild(copy);
          if (message.id === chatMessages.filter((item) => item.role === "assistant").slice(-1)[0]?.id) {
            const regenerate = document.createElement("button"); regenerate.type = "button"; regenerate.textContent = "Regenerate"; regenerate.title = "Try another response";
            regenerate.disabled = chatStreaming;
            regenerate.addEventListener("click", () => regenerateChatResponse(message.id));
            actions.appendChild(regenerate);
          }
          article.appendChild(actions);
        }
        if (message.role === "user" && message.content && !message.error) {
          const actions = document.createElement("div"); actions.className = "chat-message-actions";
          const rerun = document.createElement("button"); rerun.type = "button"; rerun.textContent = "Rerun"; rerun.title = "Run this message again";
          rerun.disabled = chatStreaming;
          rerun.addEventListener("click", () => runUserMessage(message.content));
          actions.appendChild(rerun);
          article.appendChild(actions);
        }
      }
    } else {
      const message = chatMessages[chatMessages.length - 1];
      if (message) {
        const article = chatMessagesEl.querySelector('[data-message-id="' + CSS.escape(message.id) + '"]');
        if (article) {
          const content = article.querySelector(".chat-message-content");
          if (chatStreamMessageId !== message.id) {
            chatStreamMessageId = message.id;
            chatStreamOffset = 0;
            content.classList.add("chat-streaming-text");
            content.replaceChildren(document.createTextNode(""));
          }
          if (message.content.length > chatStreamOffset) {
            content.firstChild.appendData(message.content.slice(chatStreamOffset));
            chatStreamOffset = message.content.length;
          }
        }
      }
    }
    if (atBottom) chatScroll.scrollTop = chatScroll.scrollHeight;
  }

  function postChatResize() {
    requestAnimationFrame(() => {
      const height = Math.ceil(panel.getBoundingClientRect().height);
      if (height > 0) { lastH = height; post("resize", { v: height, immediate: true }); }
    });
  }

  function resizeChatInput() {
    chatInput.style.height = "auto";
    chatInput.style.height = Math.min(112, Math.max(27, chatInput.scrollHeight)) + "px";
    chatInput.style.overflowY = chatInput.scrollHeight > 112 ? "auto" : "hidden";
  }

  function chatProviderName(provider) {
    return provider === "codex" ? "ChatGPT" : "OpenAI compatible";
  }

  function syncChatConfig() {
    const provider = chatConfig.provider || "openai-compatible";
    chatProviderInput.value = provider;
    chatProviderChoice.textContent = provider === "codex" ? "ChatGPT account" : "OpenAI-compatible API";
    for (const option of chatProviderOptions.querySelectorAll("[data-chat-provider]")) {
      option.setAttribute("aria-selected", String(option.dataset.chatProvider === provider));
    }
    chatProviderButton.setAttribute("aria-expanded", String(!chatProviderOptions.hidden));
    chatModelInput.value = chatConfig.model || (provider === "codex" ? (chatModels[0]?.id || "") : "gpt-4o-mini");
    chatBaseUrlInput.value = chatConfig.baseUrl || "https://api.openai.com/v1";
    chatEndpointField.hidden = provider !== "openai-compatible";
    chatKeyField.hidden = provider !== "openai-compatible";
    chatConnectButton.hidden = provider !== "codex" || !!chatConfig.accountConnected;
    chatDisconnectButton.hidden = provider !== "codex" || !chatConfig.accountConnected;
    chatCheckButton.hidden = provider !== "codex";
    chatCheckButton.textContent = chatConfig.accountConnected ? "Refresh status" : "Check connection";
    chatConnectionHint.textContent = provider === "openai-compatible"
      ? (chatConfig.configured ? "A key is saved securely on this device. Leave blank to keep it." : "The API key is encrypted for your Windows account and cleared from the page after saving.")
      : (chatConfig.accountConnected ? "ChatGPT account connected. Sign-in is saved on this device." : "Not connected. Choose Connect account below to sign in with your browser.");
    $("chat-provider-label").textContent = chatProviderName(provider);
    $("chat-model-label").textContent = chatConfig.model || "Choose a model";
  }

  function setChatSettingsOpen(open) {
    chatSettings.hidden = !open;
    panel.classList.toggle("chat-settings-open", open);
    if (open) setChatHistoryOpen(false);
    else { closeChatModelOptions(); closeChatProviderOptions(); }
  }

  function setChatHistoryOpen(open) {
    chatHistory.hidden = !open;
    panel.classList.toggle("chat-history-open", open);
  }

  function openChatView(fromMedia = false) {
    if (chatOpen) return;
    chatReturnToMedia = !!fromMedia;
    if (whatsNewOpen) closeWhatsNew();
    if (mediaOpen) closeMediaView();
    topPower.closeMenu(); topPower.closeConfirm(); hideContextMenu();
    chatOpen = true;
    chatView.setAttribute("aria-hidden", "false");
    panel.classList.add("chat-open");
    if (!chatConversationId) {
      // Reuse a fresh empty conversation instead of stacking a new one
      // on every restart.
      const recent = chatConversations.slice().sort((a, b) => b.updatedAt - a.updatedAt)[0];
      if (recent && recent.messages.length === 0) {
        chatConversationId = recent.id;
        chatMessages = [];
      } else newChatConversation();
    } else if (!chatConversations.some((conversation) => conversation.id === chatConversationId)) newChatConversation();
    renderChatMessages();
    post("chatState");
    resizeChatInput();
    postChatResize();
    chatInput.focus();
  }

  function closeChatView(restorePrevious) {
    if (!chatOpen) return;
    chatOpen = false;
    chatView.setAttribute("aria-hidden", "true");
    setChatSettingsOpen(false);
    setChatHistoryOpen(false);
    panel.classList.remove("chat-open");
    if (chatRecognition) { try { chatRecognition.stop(); } catch (_) {} chatRecognition = null; }
    postChatResize();
    if (restorePrevious && chatReturnToMedia && mediaState && mediaState.active) openMediaView();
    else input.focus();
    chatReturnToMedia = false;
  }

  function updateChatStreamingState(active = chatStreaming) {
    chatStreaming = !!active;
    chatView.classList.toggle("is-streaming", chatStreaming);
    chatSendButton.title = chatStreaming ? "Stop response" : "Send message";
    chatSendButton.setAttribute("aria-label", chatStreaming ? "Stop response" : "Send message");
    chatSendButton.disabled = !chatStreaming && !chatInput.value.trim();
  }

  function startChatRequest(message, reuseUserTurn = false) {
    if (chatStreaming) post("chatCancel");
    if (!reuseUserTurn) chatMessages.push({ id: "m-" + Date.now() + "-u", role: "user", content: message });
    const history = chatMessages.slice(0, reuseUserTurn ? -1 : undefined).filter((turn) => !turn.error && (turn.role === "user" || turn.role === "assistant")).slice(-24).map((turn) => ({ role: turn.role, content: turn.content.slice(0, 6000) }));
    const assistant = { id: "m-" + Date.now() + "-a", role: "assistant", content: "" };
    chatMessages.push(assistant);
    saveChatTitle(); persistChat();
    renderChatMessages();
    chatLiveStatus.textContent = "Thinking…";
    chatNotice.textContent = ""; chatNotice.classList.remove("error");
    updateChatStreamingState(true);
    post("chatSend", { message, history });
    chatInput.focus(); postChatResize();
  }

  function sendChatMessage() {
    const message = chatInput.value.trim();
    if (!message) { chatInput.focus(); return; }
    chatInput.value = ""; resizeChatInput();
    runUserMessage(message);
  }

  // Rerun affordance under each sent message: same thread, fresh attempt.
  // Chat/Agent mode: Chat answers with read-only tools, Agent runs
  // the full tool loop. Persisted across restarts, Chat by default.
  const CHAT_MODE_KEY = "nex.chat.mode.v1";
  let agentMode = false;
  try { agentMode = localStorage.getItem(CHAT_MODE_KEY) === "agent"; } catch (_) {}
  function syncChatMode() {
    $("chat-mode-chat").setAttribute("aria-selected", String(!agentMode));
    $("chat-mode-agent").setAttribute("aria-selected", String(agentMode));
    chatInput.placeholder = agentMode ? "Give me a task…" : "Ask anything…";
  }

  function runUserMessage(message) {
    // Every message runs the agent loop; Chat mode just limits it to
    // read-only tools plus answers. Legacy "!" prefix still stripped.
    const text = message.startsWith("!") ? message.slice(1).trim() : message;
    const retry = text.match(/^retry\s+(\S+)\s*$/i);
    if (retry) { startAgentGoal("", retry[1]); return; }
    if (!text) { chatInput.focus(); return; }
    startAgentGoal(text);
  }

  function startAgentGoal(goal, resumeRunId) {
    if (chatStreaming) post("chatCancel");
    chatMessages.push({ id: "m-" + Date.now() + "-u", role: "user", content: resumeRunId ? "!retry " + resumeRunId : goal });
    const assistant = { id: "m-" + Date.now() + "-a", role: "assistant", content: "" };
    chatMessages.push(assistant);
    saveChatTitle(); persistChat();
    renderChatMessages();
    chatLiveStatus.textContent = "Working…";
    chatNotice.textContent = ""; chatNotice.classList.remove("error");
    updateChatStreamingState(true);
    const mode = agentMode ? "agent" : "chat";
    post("agentGoal", resumeRunId ? { goal: goal || "", resume_run_id: resumeRunId, mode } : { goal, mode });
    chatInput.focus(); postChatResize();
  }

  function regenerateChatResponse(messageId) {
    if (chatStreaming) return;
    const assistantIndex = chatMessages.findIndex((message) => message.id === messageId);
    if (assistantIndex < 1) return;
    const previousUser = chatMessages.slice(0, assistantIndex).reverse().find((message) => message.role === "user");
    if (!previousUser) return;
    const userIndex = chatMessages.findIndex((message) => message.id === previousUser.id);
    chatMessages = chatMessages.slice(0, userIndex + 1);
    startChatRequest(previousUser.content, true);
  }

  function startChatVoice() {
    const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
    if (!SpeechRecognition) {
      chatNotice.textContent = "Voice input isn’t available in this Windows WebView. You can type your message instead.";
      chatNotice.classList.add("error");
      return;
    }
    if (chatRecognition) { try { chatRecognition.stop(); } catch (_) {} chatRecognition = null; return; }
    const recognition = new SpeechRecognition();
    chatRecognition = recognition; chatVoiceTranscript = ""; chatAutoSendVoice = true;
    recognition.lang = navigator.language || "en-US"; recognition.interimResults = true; recognition.continuous = false;
    chatView.classList.add("is-listening");
    chatLiveStatus.textContent = "Listening…";
    chatNotice.textContent = "Speak your message. It will send when transcription finishes.";
    chatNotice.classList.remove("error");
    recognition.onresult = (event) => {
      let interim = "";
      for (let i = event.resultIndex; i < event.results.length; i++) {
        const text = event.results[i][0].transcript;
        if (event.results[i].isFinal) chatVoiceTranscript += text;
        else interim += text;
      }
      chatLiveStatus.textContent = "Transcribing…";
      chatInput.value = (chatVoiceTranscript + interim).trimStart(); resizeChatInput(); updateChatStreamingState();
    };
    recognition.onerror = (event) => {
      chatAutoSendVoice = false;
      chatNotice.textContent = event.error === "not-allowed" ? "Allow microphone access in Windows to use voice input." : "Voice input stopped. You can continue by typing.";
      chatNotice.classList.add("error");
    };
    recognition.onend = () => {
      chatRecognition = null; chatView.classList.remove("is-listening");
      chatLiveStatus.textContent = "";
      const shouldSend = chatAutoSendVoice && chatVoiceTranscript.trim().length > 0;
      chatAutoSendVoice = false;
      chatNotice.textContent = shouldSend ? "Sending transcription…" : "";
      if (shouldSend) sendChatMessage();
      else { chatInput.focus(); updateChatStreamingState(); }
    };
    try { recognition.start(); }
    catch (_) { chatRecognition = null; chatView.classList.remove("is-listening"); chatNotice.textContent = "Voice input could not start. Check microphone access and try again."; chatNotice.classList.add("error"); }
  }

  let chatNoticeTimer = 0;
  // Transient notices clear themselves after a few seconds. The timer
  // only clears its own text, so a newer notice is never wiped early.
  // Errors use direct assignment and stay until replaced.
  function flashChatNotice(text) {
    window.clearTimeout(chatNoticeTimer);
    chatNotice.textContent = text;
    chatNotice.classList.remove("error");
    chatNoticeTimer = window.setTimeout(() => {
      if (chatNotice.textContent === text) chatNotice.textContent = "";
    }, 3500);
  }

  function saveChatProvider() {
    const payload = {
      provider: chatProviderInput.value,
      baseUrl: chatBaseUrlInput.value.trim(),
      model: chatModelInput.value.trim(),
      apiKey: chatApiKeyInput.value,
    };
    post("chatConfigure", payload);
    chatApiKeyInput.value = "";
    flashChatNotice("Saving provider settings…");
  }

  function renderChatModelOptions(open = !chatModelOptions.hidden) {
    chatModelResults.replaceChildren();
    if (chatModelsLoading) {
      const loading = document.createElement("div");
      loading.className = "chat-model-empty";
      loading.textContent = "Loading available models…";
      chatModelResults.append(loading);
      if (open) showChatModelOptions();
      return;
    }
    const query = chatModelSearch.value.trim().toLowerCase();
    const matches = chatModelsProvider === chatProviderInput.value
      ? chatModels.filter((model) => !query || `${model.id} ${model.name} ${model.description}`.toLowerCase().includes(query))
      : [];
    if (!matches.length) {
      const empty = document.createElement("div");
      empty.className = "chat-model-empty";
      empty.textContent = chatModels.length
        ? "No matching models. You can still enter a model ID."
        : (chatProviderInput.value === "codex" && !chatConfig.accountConnected
          ? "Connect your ChatGPT account to load models."
          : "Switch provider or save settings to load available models.");
      chatModelResults.append(empty);
      if (open) showChatModelOptions();
      return;
    }
    for (const model of matches) {
      const option = document.createElement("button");
      option.type = "button";
      option.className = "chat-model-option";
      option.setAttribute("role", "option");
      option.setAttribute("aria-selected", String(chatModelInput.value === model.id));
      option.textContent = model.name || model.id;
      option.addEventListener("click", () => {
        chatModelInput.value = model.id;
        chatConfig.model = model.id;
        chatModelInput.focus();
        closeChatModelOptions();
        syncChatConfig();
      });
      chatModelResults.append(option);
    }
    if (open) showChatModelOptions();
  }

  function showChatModelOptions() {
    chatModelOptions.hidden = false;
  }

  function closeChatModelOptions() {
    chatModelOptions.hidden = true;
  }

  function closeChatProviderOptions() {
    chatProviderOptions.hidden = true;
    chatProviderButton.setAttribute("aria-expanded", "false");
  }

  function fetchChatModels(showMenu = true) {
    if (chatModelsLoading) return;
    if (chatProviderInput.value === "codex" && !chatConfig.accountConnected) {
      chatNotice.textContent = "Connect your ChatGPT account before loading models.";
      chatNotice.classList.add("error");
      renderChatModelOptions(showMenu);
      return;
    }
    if (chatProviderInput.value === "openai-compatible" && !chatApiKeyInput.value && !chatConfig.configured) {
      chatNotice.textContent = "Add and save an API key before loading models.";
      chatNotice.classList.add("error");
      if (!chatModelOptions.hidden) renderChatModelOptions();
      return;
    }
    const payload = {
      provider: chatProviderInput.value,
      baseUrl: chatBaseUrlInput.value.trim(),
      model: chatModelInput.value.trim(),
      apiKey: chatApiKeyInput.value,
    };
    setChatModelsLoading(true);
    chatNotice.textContent = "Loading models from your provider…";
    chatNotice.classList.remove("error");
    renderChatModelOptions(showMenu);
    post("chatFetchModels", payload);
    chatModelsTimer = window.setTimeout(() => {
      setChatModelsLoading(false);
      closeChatModelOptions();
      chatNotice.textContent = "Model loading timed out. Check the provider connection and try again.";
      chatNotice.classList.add("error");
    }, 30000);
  }

  function setChatModelsLoading(loading) {
    chatModelsLoading = loading;
    window.clearTimeout(chatModelsTimer);
    chatModelsTimer = 0;
    chatProviderInput.disabled = loading;
  }

  function checkCodexConnection() {
    if (chatConnectionCheckPending) return;
    chatConnectionCheckPending = true;
    chatCheckButton.disabled = true;
    chatCheckButton.textContent = "Checking…";
    chatConnectionHint.textContent = "Checking the ChatGPT sign-in status…";
    chatNotice.textContent = "Checking your ChatGPT account…";
    chatNotice.classList.remove("error");
    post("chatState");
    window.clearTimeout(chatConnectionTimer);
    chatConnectionTimer = window.setTimeout(() => {
      chatConnectionCheckPending = false;
      chatCheckButton.disabled = false;
      chatCheckButton.textContent = chatConfig.accountConnected ? "Refresh status" : "Check connection";
      chatConnectionHint.textContent = chatConfig.accountConnected
        ? "ChatGPT account connected. Sign-in is saved on this device."
        : "Not connected. Choose Connect account below to sign in with your browser.";
      chatNotice.textContent = "Nex couldn’t verify ChatGPT right now. Try checking again.";
      chatNotice.classList.add("error");
    }, 8000);
  }

  $("chat-voice-entry").addEventListener("click", () => { openChatView(false); requestAnimationFrame(startChatVoice); });
  $("chat-back").addEventListener("click", () => closeChatView(true));
  $("chat-new-button").addEventListener("click", newChatConversation);
  $("chat-mode-chat").addEventListener("click", () => {
    agentMode = false;
    try { localStorage.setItem(CHAT_MODE_KEY, "chat"); } catch (_) {}
    syncChatMode();
    chatInput.focus();
  });
  $("chat-mode-agent").addEventListener("click", () => {
    agentMode = true;
    try { localStorage.setItem(CHAT_MODE_KEY, "agent"); } catch (_) {}
    syncChatMode();
    chatInput.focus();
  });
  syncChatMode();
  $("chat-transcript-button").addEventListener("click", async () => {    const lines = [];
    for (const message of chatMessages) {
      if (message.role === "user") lines.push("You: " + message.content);
      else if (message.role === "assistant") lines.push("Nex: " + message.content);
      else if (message.role === "step") lines.push("• " + message.content);
    }
    try {
      await navigator.clipboard.writeText(lines.join("\n\n"));
      chatNotice.textContent = "Transcript copied to clipboard.";
      chatNotice.classList.remove("error");
      window.setTimeout(() => {
        if (chatNotice.textContent === "Transcript copied to clipboard.") chatNotice.textContent = "";
      }, 1500);
    } catch (_) {
      chatNotice.textContent = "Copy unavailable in this view.";
      chatNotice.classList.add("error");
    }
  });
  $("chat-history-button").addEventListener("click", () => { renderChatHistory(); setChatHistoryOpen(chatHistory.hidden); });
  $("chat-model-button").addEventListener("click", (event) => {
    event.currentTarget.blur();
    const open = chatSettings.hidden;
    setChatSettingsOpen(open);
    if (open) {
      post("chatState");
      if (chatProviderInput.value === "codex" && chatConfig.accountConnected && !chatModels.length) fetchChatModels(false);
    }
    postChatResize();
  });
  $("chat-save-button").addEventListener("click", () => {
    saveChatProvider();
    setChatSettingsOpen(false);
    postChatResize();
  });
  chatProviderInput.addEventListener("change", () => {
    closeChatProviderOptions();
    chatModels = [];
    chatModelsProvider = "";
    chatModelInput.value = chatProviderInput.value === "codex"
      ? (chatModels.find((model) => model.default)?.id || chatModels[0]?.id || "gpt-6-luna")
      : (chatConfig.provider === "openai-compatible" ? (chatConfig.model || "gpt-4o-mini") : "gpt-4o-mini");
    chatConfig.provider = chatProviderInput.value;
    chatConfig.model = chatModelInput.value;
    syncChatConfig(); fetchChatModels(false); postChatResize();
  });
  chatProviderButton.addEventListener("click", () => {
    const open = chatProviderOptions.hidden;
    closeChatModelOptions();
    if (open) {
      chatProviderOptions.hidden = false;
      chatProviderButton.setAttribute("aria-expanded", "true");
      chatProviderOptions.querySelector('[aria-selected="true"]')?.focus();
    } else closeChatProviderOptions();
  });
  for (const option of chatProviderOptions.querySelectorAll("[data-chat-provider]")) {
    option.addEventListener("click", () => {
      chatProviderInput.value = option.dataset.chatProvider;
      chatProviderInput.dispatchEvent(new Event("change", { bubbles: true }));
    });
  }
  chatConnectButton.addEventListener("click", () => {
    if (chatConnectButton.disabled) return;
    chatConnectButton.disabled = true;
    chatConnectButton.textContent = "Opening…";
    chatConnectButton.setAttribute("aria-busy", "true");
    window.clearTimeout(chatConnectTimer);
    chatConnectTimer = window.setTimeout(() => {
      chatConnectButton.disabled = false;
      chatConnectButton.textContent = "Connect account";
      chatConnectButton.setAttribute("aria-busy", "false");
      chatNotice.textContent = "Nex didn’t hear back from ChatGPT. Check the sign-in window and try again.";
      chatNotice.classList.add("error");
    }, 10000);
    saveChatProvider();
    post("chatConnect", chatProviderInput.value);
  });
  chatDisconnectButton.addEventListener("click", () => {
    if (chatDisconnectButton.disabled) return;
    chatDisconnectButton.disabled = true;
    chatDisconnectButton.textContent = "Logging out…";
    chatDisconnectButton.setAttribute("aria-busy", "true");
    window.clearTimeout(chatDisconnectTimer);
    chatDisconnectTimer = window.setTimeout(() => {
      chatDisconnectButton.disabled = false;
      chatDisconnectButton.textContent = "Log out";
      chatDisconnectButton.setAttribute("aria-busy", "false");
      chatNotice.textContent = "Nex didn’t hear back from ChatGPT. Try again.";
      chatNotice.classList.add("error");
    }, 10000);
    post("chatDisconnect");
  });
  chatCheckButton.addEventListener("click", () => {
    checkCodexConnection();
  });
  chatModelInput.addEventListener("focus", () => {
    if (chatModelOptions.hidden) {
      chatModelSearch.value = "";
      renderChatModelOptions(true);
    }
  });
  chatModelSearch.addEventListener("input", renderChatModelOptions);
  // Hover-select like result rows: the highlight follows the cursor.
  function trackMenuHover(container) {
    container.addEventListener("mousemove", (event) => {
      const option = event.target.closest ? event.target.closest("button") : null;
      if (!option || !container.contains(option)) return;
      for (const item of container.querySelectorAll("button")) {
        item.setAttribute("aria-selected", String(item === option));
      }
    }, { passive: true });
  }
  trackMenuHover(chatProviderOptions);
  trackMenuHover(chatModelResults);
  trackMenuHover(chatHistory);
  // History scrollbar stays hidden until actually scrolled.
  let chatHistoryScrollTimer = 0;
  chatHistory.addEventListener("scroll", () => {
    chatHistory.classList.add("scrolling");
    window.clearTimeout(chatHistoryScrollTimer);
    chatHistoryScrollTimer = window.setTimeout(() => {
      chatHistory.classList.remove("scrolling");
    }, 1200);
  }, { passive: true });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if (!chatModelOptions.hidden) {
      closeChatModelOptions();
      chatModelInput.focus();
    } else if (!chatProviderOptions.hidden) {
      closeChatProviderOptions();
      chatProviderButton.focus();
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (!event.target.closest(".chat-model-picker")) closeChatModelOptions();
    if (!event.target.closest(".chat-provider-picker")) closeChatProviderOptions();
    if (!chatSettings.hidden && !event.target.closest("#chat-settings") && !event.target.closest("#chat-model-button")) setChatSettingsOpen(false);
    if (!chatHistory.hidden && !event.target.closest("#chat-history") && !event.target.closest("#chat-history-button")) setChatHistoryOpen(false);
  });
  chatSendButton.addEventListener("click", () => chatStreaming ? post("chatCancel") : sendChatMessage());
  // Model links open in the default browser (WebView2 blocks target=_blank).
  chatMessagesEl.addEventListener("click", (event) => {
    const link = event.target.closest ? event.target.closest("a[href]") : null;
    if (!link || !chatMessagesEl.contains(link)) return;
    event.preventDefault();
    post("openExternal", link.href);
  });
  $("chat-voice-button").addEventListener("click", startChatVoice);
  chatInput.addEventListener("input", () => { resizeChatInput(); updateChatStreamingState(); });
  chatInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) { event.preventDefault(); sendChatMessage(); }
  });
  for (const button of document.querySelectorAll("[data-chat-prompt]")) {
    button.addEventListener("click", () => { chatInput.value = button.dataset.chatPrompt; resizeChatInput(); sendChatMessage(); });
  }

  function applyChatUpdate(state) {
    // Agent loop pushes raw AgentEvent JSON ({t:"agentStep"|...}) while
    // later turns use the wrapped chat shape ({agentStep:{...}}) — accept both.
    if (state && typeof state.t === "string" && state.t.slice(0, 5) === "agent") {
      if (state.t === "agentStep") state = { agentStep: { step: state.step, tool: state.tool, state: state.state, detail: state.detail, run_id: state.run_id } };
      else if (state.t === "agentApproval") state = { agentApproval: { call_id: state.call_id, tool: state.tool, args_summary: state.args_summary } };
      else if (state.t === "agentDone") state = { agentDone: { summary: state.summary, run_id: state.run_id } };
    }
    if (state.chatConfig) {
      const wasCodexConnected = chatConfig.provider === "codex" && chatConfig.accountConnected;
      chatConfig = { ...chatConfig, ...state.chatConfig };
      syncChatConfig();
      if (chatConnectionCheckPending) {
        window.clearTimeout(chatConnectionTimer);
        chatConnectionCheckPending = false;
        chatCheckButton.disabled = false;
        chatCheckButton.textContent = chatConfig.accountConnected ? "Refresh status" : "Check connection";
        if (chatConfig.accountConnected) flashChatNotice("ChatGPT account connected.");
        else { chatNotice.textContent = "ChatGPT is not connected. Choose Connect account to sign in."; chatNotice.classList.add("error"); }
      }
      if (!wasCodexConnected && chatConfig.provider === "codex" && chatConfig.accountConnected && !chatSettings.hidden) fetchChatModels(false);
      if (chatConfig.provider === "openai-compatible" && chatConfig.configured && chatModelsProvider !== "openai-compatible" && !chatModelsLoading) fetchChatModels(false);
    }
    if (typeof state.chatConnecting === "boolean") {
      window.clearTimeout(chatConnectTimer);
      chatConnectTimer = 0;
      chatConnectButton.disabled = state.chatConnecting;
      chatConnectButton.textContent = state.chatConnecting ? "Waiting for sign-in…" : "Connect account";
      chatConnectButton.setAttribute("aria-busy", String(state.chatConnecting));
    }
    if (typeof state.chatDisconnecting === "boolean") {
      window.clearTimeout(chatDisconnectTimer);
      chatDisconnectTimer = 0;
      chatDisconnectButton.disabled = state.chatDisconnecting;
      chatDisconnectButton.textContent = state.chatDisconnecting ? "Logging out…" : "Log out";
      chatDisconnectButton.setAttribute("aria-busy", String(state.chatDisconnecting));
      if (!state.chatDisconnecting) {
        chatConnectButton.disabled = false;
        chatConnectButton.textContent = "Connect account";
        chatConnectButton.setAttribute("aria-busy", "false");
      }
    }
    if (state.chatModels) {
      setChatModelsLoading(false);
      if (state.chatModels.provider === chatProviderInput.value) {
        chatModels = Array.isArray(state.chatModels.models) ? state.chatModels.models : [];
        chatModelsProvider = state.chatModels.provider;
        if (chatProviderInput.value === "codex" && chatModels.length && !chatModels.some((model) => model.id === chatModelInput.value)) {
          const recommended = chatModels.find((model) => model.default || model.featured) || chatModels[0];
          chatModelInput.value = recommended.id;
          chatConfig.model = recommended.id;
          post("chatConfigure", { provider: "codex", baseUrl: chatBaseUrlInput.value, model: recommended.id, apiKey: "" });
        } else if (!chatModelInput.value && chatModels.length) chatModelInput.value = chatModels[0].id;
        renderChatModelOptions();
        flashChatNotice(chatModels.length ? `${chatModels.length} models loaded.` : "No models were returned by this provider.");
      }
    }
    if (state.chatNotice) flashChatNotice(state.chatNotice);
    if (state.chatError) {
      setChatModelsLoading(false);
      if (!chatModelOptions.hidden && !chatStreaming) renderChatModelOptions();
      window.clearTimeout(chatConnectTimer);
      chatConnectTimer = 0;
      chatConnectButton.disabled = false;
      chatConnectButton.textContent = "Connect account";
      chatConnectButton.setAttribute("aria-busy", "false");
      chatNotice.textContent = state.chatError; chatNotice.classList.add("error");
      const last = chatMessages[chatMessages.length - 1];
      if (chatStreaming && last && last.role === "assistant" && !last.content) { last.content = state.chatError; last.error = true; }
      if (chatStreaming) { renderChatMessages(); updateChatStreamingState(false); chatLiveStatus.textContent = ""; persistChat(); }
    }
    if (state.chatDelta) {
      const last = chatMessages[chatMessages.length - 1];
      if (last && last.role === "assistant") {
        if (state.chatDelta.text) last.content += state.chatDelta.text;
        if (state.chatDelta.status !== undefined) chatLiveStatus.textContent = state.chatDelta.status || "";
        if (state.chatDelta.text && !chatRenderFrame) {
          chatRenderFrame = requestAnimationFrame(() => { chatRenderFrame = 0; renderChatMessages(true); });
        }
      }
      persistChat();
    }
    if (state.chatDone) { chatLiveStatus.textContent = ""; updateChatStreamingState(false); renderChatMessages(); persistChat(); }
    if (state.chatCancelled) { chatLiveStatus.textContent = "Response stopped"; updateChatStreamingState(false); renderChatMessages(); persistChat(); }
    if (state.agentApproval) renderAgentApproval(state.agentApproval);
    if (state.agentStep) {
      const s = state.agentStep;
      const text = "step " + (s.step ?? "?") + " · " + (s.tool || "tool") + " · " + (s.state || "") + (s.detail ? " — " + s.detail : "");
      chatMessages.push({ id: "s-" + (chatStepSeq++).toString(36), role: "step", kind: "step", content: text });
      persistChat();
      const line = document.createElement("article");
      line.className = "agent-step";
      line.textContent = text;
      appendAgentNode(line);
    }
    if (state.agentDone) {
      const d = typeof state.agentDone === "string" ? { summary: state.agentDone } : state.agentDone;
      const text = (d.summary || "Done") + (d.run_id ? " · run " + d.run_id : "");
      chatMessages.push({ id: "s-" + (chatStepSeq++).toString(36), role: "step", kind: "done", content: text });
      persistChat();
      const done = document.createElement("article");
      done.className = "agent-done";
      done.textContent = text;
      appendAgentNode(done);
      chatLiveStatus.textContent = "";
      updateChatStreamingState(false);
    }
  }

  // Step/done lines persist via chatMessages; approval cards and
  // history lines stay ephemeral.
  function appendAgentNode(node) {
    const atBottom = chatScroll.scrollHeight - chatScroll.scrollTop - chatScroll.clientHeight < 42;
    chatEmpty.hidden = true;
    chatMessagesEl.appendChild(node);
    if (atBottom) chatScroll.scrollTop = chatScroll.scrollHeight;
  }

  function renderAgentApproval(payload) {
    const data = payload && typeof payload === "object" ? payload : {};
    const callId = String(data.call_id || data.callId || "");
    if (!callId || chatMessagesEl.querySelector('[data-agent-call="' + CSS.escape(callId) + '"]')) return;
    const card = document.createElement("article");
    card.className = "agent-card";
    card.dataset.agentCall = callId;
    const head = document.createElement("div");
    head.className = "agent-card-head";
    const tool = document.createElement("span");
    tool.className = "agent-tool";
    tool.textContent = String(data.tool || "tool");
    const actions = document.createElement("div");
    actions.className = "agent-actions";
    const approve = document.createElement("button");
    approve.type = "button";
    approve.className = "agent-approve";
    approve.textContent = "Approve";
    const deny = document.createElement("button");
    deny.type = "button";
    deny.className = "agent-deny";
    deny.textContent = "Deny";
    const decided = document.createElement("div");
    decided.className = "agent-decided";
    decided.hidden = true;
    const settle = (ok) => {
      approve.disabled = true;
      deny.disabled = true;
      post(ok ? "agentApprove" : "agentDeny", { call_id: callId });
      decided.textContent = ok ? "Approved" : "Denied";
      decided.hidden = false;
      card.dataset.decided = ok ? "approved" : "denied";
    };
    approve.addEventListener("click", () => settle(true));
    deny.addEventListener("click", () => settle(false));
    actions.append(approve, deny);
    head.append(tool, actions);
    const args = document.createElement("div");
    args.className = "agent-args";
    args.textContent = String(data.args_summary || data.argsSummary || "");
    card.append(head, args, decided);
    appendAgentNode(card);
  }

  // ── Rust → JS bridge ─────────────────────────────────────
  window.nex = {
    apply(state) {
      if (!Array.isArray(state.rows) && (state.chatConfig || state.chatModels || state.chatDelta || state.chatDone || state.chatError || state.chatNotice || state.chatCancelled || state.agentApproval || state.agentStep || state.agentDone || state.t === "agentStep" || state.t === "agentApproval" || state.t === "agentDone" || typeof state.chatConnecting === "boolean" || typeof state.chatDisconnecting === "boolean")) {
        applyChatUpdate(state);
        return;
      }
      // Media state message: {"media": {...}} — no rows, no re-render.
      if (state.media && typeof state.media === "object" && !Array.isArray(state.rows)) {
        mediaState = state.media;
        renderMedia();
        return;
      }

      // What's New content: {"whatsNew": {"version","markdown"}} — renders
      // into the open view without touching search state.
      if (state.whatsNew && typeof state.whatsNew === "object" && !Array.isArray(state.rows)) {
        // Stored even while closed: a close-then-reopen during the fetch
        // must still render the late arrival instead of refetching.
        whatsNewContent = state.whatsNew;
        if (whatsNewOpen) renderWhatsNew();
        return;
      }

      // Icon data message: {"icons": {"path": "data:...", ...}}
      // Sent as a separate PostWebMessageAsJson after the state message.
      // Early return before closing footer menu — icons-only pushes must not
      // interfere with the power dropup the user may be interacting with.
      if (state.icons && typeof state.icons === "object" && !state.rows) {
        for (const [path, dataUri] of Object.entries(state.icons)) {
          iconCache.set(path, dataUri);
        }
        patchIcons();
        return;
      }

      // Close the power panel / confirm whenever Rust pushes a fresh state
      // (show, hide, query change, etc.)
      topPower.closeMenu();
      topPower.closeConfirm();
      hideContextMenu();

      // Lightweight selection-only update (no rows = incremental).
      if (!Array.isArray(state.rows) && typeof state.selected === "number") {
        setSelected(state.selected, true);
        return;
      }

      // Lightweight status-only update — apply without re-rendering rows.
      if (!Array.isArray(state.rows) && typeof state.status === "string") {
        statusEl.dataset.text = state.status || "";
        if (state.status.startsWith("Updated")) {
          updateBtn.disabled = true;
          updateBtnLabel.textContent = "Updated";
        } else if (state.status.startsWith("Up to date")) {
          updateBtn.disabled = false;
          updateBtnLabel.textContent = "Up to date";
        } else if (state.status.startsWith("Update failed") || state.status.startsWith("Could not")) {
          updateBtn.disabled = false;
          updateBtnLabel.textContent = "Retry";
        }
        return;
      }

      if (state.theme) document.documentElement.dataset.theme = state.theme;

      // System accent (DWM AccentColor) → CSS var; falls back to theme palette.
      if (state.accent) {
        document.documentElement.style.setProperty("--accent", state.accent);
      }

      // Toggle grid/list layout based on Rust config
      if (typeof state.gridView === "boolean") {
        list.classList.toggle("grid-view", state.gridView);
      }

      // Toggle bento view for clipboard history
      if (typeof state.bentoView === "boolean") {
        list.classList.toggle("bento-view", state.bentoView);
      }

      // Only overwrite the input if Rust changed it out from under us
      // (e.g. clear on hide, quick-shortcut expansion).
      if (typeof state.query === "string") {
        let display = state.query;
        let wasCmd = inCommandMode;
        if (display.startsWith("@") || display.startsWith(">")) {
          inCommandMode = true;
          display = display.slice(1);
        } else {
          inCommandMode = false;
        }
        if (wasCmd !== inCommandMode) updateSearchIcon();
        if (display !== input.value) {
          queryEcho = display;
          input.value = display;
        }
      }

      // Command-mode autofill title pushed by Rust (null outside
      // command mode). renderCompletion gates it against the typed
      // text, so stale values can never overwrite the input.
      completion = typeof state.completion === "string" ? state.completion : "";

      // Update availability: show/hide update notice. A pending
      // post-update version takes over the notice: it opens the
      // What's New view instead of running an update check.
      if (typeof state.updateAvailable === "boolean" || typeof state.whatsNewPending !== "undefined") {
        if (typeof state.updateAvailable === "boolean") {
          updateAvailable = state.updateAvailable;
        }
        if (typeof state.whatsNewPending !== "undefined") {
          whatsNewPending = typeof state.whatsNewPending === "string" ? state.whatsNewPending : null;
        }
        syncUpdateNotice();
      }

      // Native glass below the page: drop painted backgrounds so the
      // refracted layer shows through instead of acrylic.
      if (typeof state.glassNative === "boolean") {
        document.documentElement.classList.toggle("glass-native", state.glassNative);
      }

      // Track QL presence before overwriting rows — used to detect
      // quick-launch → results transition for immediate resize.
      const prevHadQuickLaunch = rows.some(r => r.role === "quick_launch");
      const prevHadAction = rows.some(r => r.kind === "action");
      const wasIdle = bodyEl.classList.contains("idle");
      const prevRowCount = rows.length;
      const prevHadInline = rows.some(r => r.kind === "file" || r.kind === "folder");
      const prevHadApp = rows.some(r => r.kind === "app");

      rows = Array.isArray(state.rows) ? state.rows : [];
      selected = typeof state.selected === "number" ? state.selected : 0;

      // Store Quick Launch items if provided
      if (Array.isArray(state.quickLaunch)) {
        quickLaunchItems = state.quickLaunch;
      }

      // Placeholder is applied by renderCompletion() (single source of
      // truth, mode-aware) — here we only remember what Rust pushed.
      pushedPlaceholder = typeof state.placeholder === "string" ? state.placeholder : "";

      statusEl.dataset.text = state.status || "";

      // Signal that the next render should fire post("painted")
      // so the Rust side can show + focus the window. Only set on
      // show (when Rust sends showPending=true in the state JSON).
      // Also reset scroll position — otherwise scrollTop survives
      // across hide/show and new queries start at old scroll depth.
      const isShow = state.showPending;
      if (isShow) {
        pendingShow = true;
        lastH = 0; // fresh show cycle: trigger resize on first content paint
        scrollToInstant(0);
        // Reveal waits for content (or the cap below) — never the idle
        // mid-state. needsPainted is consumed by measure's paint path.
        needsPainted = false;
        window.clearTimeout(showRevealTimer);
        showRevealTimer = window.setTimeout(fireShowReveal, 500);
        // Transient model-settings state must not survive hide/show.
        setChatSettingsOpen(false);
      }
      render();

      // Structural transitions → post immediate resize so the window
      // follows right away instead of waiting for the debounced growth
      // path (2x rAF in measure() + 100ms Rust debounce): QL → results,
      // idle/empty → content, >= 2-row jumps (row-count based so
      // mixed grid/row-list content triggers regardless of pixel delta),
      // first inline (file/folder) row appearing under an app grid, and
      // app-grid presence flipping on/off. The pixel-delta bigJump (70px)
      // misses single 46px row growth and equal-height layout flips,
      // which then pop after the 100ms debounce.
      const nowHasInline = rows.some(r => r.kind === "file" || r.kind === "folder");
      const nowHasApp = rows.some(r => r.kind === "app");
      if ((prevHadQuickLaunch
        || wasIdle
        || rows.length - prevRowCount >= 2
        || (!prevHadAction && rows.some(r => r.kind === "action"))
        || (!prevHadInline && nowHasInline)
        || (prevHadApp !== nowHasApp))
        && rows.length > 0) {
        const h = Math.ceil(panel.getBoundingClientRect().height);
        if (h > 0) {
          lastH = h;
          post("resize", { v: h, immediate: true });
        }
      }

      // On fresh show, the Show push has empty rows (hide cleared them).
      // Real results arrive on a later Apply push with showPending=false.
      // The pendingShow flag bridges this gap — consumed here when the
      // first non-empty rows arrive after a show cycle.
      if (pendingShow && rows.length > 0) {
        pendingShow = false;
        window.clearTimeout(showRevealTimer);
        showRevealTimer = 0;
        // First content painted: arm the reveal. The double-rAF queued
        // by render()'s measure() above reads this flag at fire time.
        needsPainted = true;
        scrollToInstant(0);
        requestAnimationFrame(() => { scrollToInstant(0); });
        // Scroll to top — selected item starts at index 0, already in view.
      }

      renderCompletion();
    },

    focus() {
      // Called by Rust via evaluate_script after every Show + painted.
      // Reset scroll here too — covers any case where the state-push
      // reset was dropped (race, coalesced render, etc).
      scrollToInstant(0);
      input.focus();
      input.select();
    },
  };

  // Tell Rust the page is ready to receive state.
  // Do NOT call measure() here — it posts "painted" which races with
  // the first push_state.  painted must only fire after nex.apply()
  // renders the pushed state, otherwise the window appears blank.
  post("ready");

  // ── settings button ─────────────────────────────────────────
  document.getElementById("footer-settings-btn").addEventListener("click", () => {
    post("settings");
  });

  // ── update button ───────────────────────────────────────────
  function syncUpdateNotice() {
    const show = updateAvailable || whatsNewPending !== null;
    updateNotice.classList.toggle("hidden", !show);
    updateNotice.classList.toggle("whats-new", whatsNewPending !== null);
    if (whatsNewPending !== null) {
      updateBtn.disabled = false;
      updateBtn.title = "See what's new";
      updateBtn.setAttribute("aria-label", "See what's new in version " + whatsNewPending);
      updateBtnLabel.textContent = "What's new";
    } else if (!updateBtn.disabled || updateBtnLabel.textContent === "What's new") {
      updateBtn.title = "Update available";
      updateBtn.setAttribute("aria-label", "Update available");
      updateBtnLabel.textContent = "Update";
    }
  }

  updateBtn.addEventListener("click", () => {
    // Post-update, once per version: open the What's New view instead
    // of the normal update flow. Rust marks the version seen on open.
    if (whatsNewPending !== null) {
      openWhatsNew();
      return;
    }
    updateBtn.disabled = true;
    updateBtnLabel.textContent = "Updating...";
    post("checkUpdates");
  });

  // ── what's new view ─────────────────────────────────────────
  // Static interaction tips: the hidden gestures users otherwise never
  // discover. Always shown under the version highlights.
  const WHATS_NEW_TIPS = [
    ["Tab", "opens the media view while music plays"],
    ["@", "enters command mode"],
    ["↑ ↓", "move through results"],
    ["Enter", "launches the selected item"],
    ["Esc", "closes the overlay"],
  ];

  function escapeHtml(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  // Tiny markdown subset for release notes: ### headings, - bullets with
  // **bold** labels, [links](url), `code`. Raw HTML never passes through.
  function renderInlineMd(s) {
    let out = escapeHtml(s);
    out = out.replace(/`([^`]+)`/g, "<code>$1</code>");
    out = out.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    out = out.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<span class="wn-link">$1</span>');
    return out;
  }

  function renderWhatsNewMarkdown(md) {
    const items = [];
    let count = 0;
    for (const raw of md.split("\n")) {
      const line = raw.trim();
      if (!line || count >= 10) continue;
      if (/^#{2,4}\s/.test(line)) {
        items.push({ h: line.replace(/^#{2,4}\s*/, "") });
        continue;
      }
      if (/^[-*]\s/.test(line)) {
        items.push({ li: line.replace(/^[-*]\s*/, "") });
        count++;
      }
    }
    if (!items.length) return "";
    return items
      .map((it) =>
        it.h
          ? `<h4>${renderInlineMd(it.h)}</h4>`
          : `<div class="whats-new-item"><span>${renderInlineMd(it.li)}</span></div>`
      )
      .join("");
  }

  function postWhatsNewResize() {
    requestAnimationFrame(() => {
      const h = Math.ceil(panel.getBoundingClientRect().height);
      if (h > 0) {
        lastH = h;
        post("resize", { v: h, immediate: true });
      }
    });
  }

  function renderWhatsNew() {
    if (!whatsNewOpen) return;
    const version = whatsNewContent?.version || whatsNewPending || "";
    whatsNewTitle.textContent = version ? "Nex v" + version + " is here" : "Nex updated";
    const md = whatsNewContent && typeof whatsNewContent.markdown === "string"
      ? whatsNewContent.markdown.trim()
      : "";
    whatsNewItems.innerHTML = md
      ? renderWhatsNewMarkdown(md)
      : `<div class="whats-new-item"><span>This release brings fixes and polish. You're all set.</span></div>`;
    whatsNewTips.innerHTML =
      `<div class="tips-heading">Good to know</div>` +
      WHATS_NEW_TIPS.map(([k, v]) => `<div><kbd>${escapeHtml(k)}</kbd> ${escapeHtml(v)}</div>`).join("");
    postWhatsNewResize();
  }

  function openWhatsNew() {
    if (whatsNewOpen) return;
    if (chatOpen) closeChatView(false);
    if (mediaOpen) closeMediaView();
    topPower.closeMenu();
    topPower.closeConfirm();
    hideContextMenu();
    whatsNewOpen = true;
    panel.classList.add("whats-new-open");
    document.getElementById("whats-new-view").scrollTop = 0;
    // Fetch only without cached notes: a close-then-reopen while the
    // first fetch is in flight must not spend a second fetch, and the
    // late arrival renders via nex.apply below.
    if (!whatsNewContent) {
      post("whatsNew");
      // The pending flag is spent locally — Rust confirms via snapshot.
      whatsNewPending = null;
      syncUpdateNotice();
    }
    renderWhatsNew();
    postWhatsNewResize();
  }

  function closeWhatsNew() {
    if (!whatsNewOpen) return;
    whatsNewOpen = false;
    // Cached notes stay: reopening renders instantly without refetching.
    panel.classList.remove("whats-new-open");
    postWhatsNewResize();
    input.focus();
  }

  document.getElementById("whats-new-dismiss").addEventListener("click", () => {
    closeWhatsNew();
  });

  // ── media view ──────────────────────────────────────────────
  function fmtTime(secs) {
    if (!isFinite(secs) || secs < 0) secs = 0;
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return m + ":" + String(s).padStart(2, "0");
  }

  function postMediaResize() {
    // The panel height changed (search UI hidden/shown) — tell Rust to
    // hug the new content height immediately, like structural resizes.
    requestAnimationFrame(() => {
      const h = Math.ceil(panel.getBoundingClientRect().height);
      if (h > 0) {
        lastH = h;
        post("resize", { v: h, immediate: true });
      }
    });
  }

  // Calculated playback clock. Rust pushes jitter by fractions of a
  // second, so mid-track pushes are never adopted — adopting any of them
  // yanks the bar and timer back and forth. The anchor is set on genuine
  // events only (track change, seek, play/pause flip, user drag); frames
  // derive position from the monotonic wall clock, so the bar can never
  // move backward except on a real discontinuity.
  let mediaAnchor = { key: "", status: "", pos: 0, at: 0 };
  let mediaRaf = 0;
  let mediaLastPushAt = 0;
  let mediaLastPushPos = 0;

  function mediaTrackKey(m) {
    // Normalized + duration rounded: SMTC metadata can flicker cosmetically
    // (whitespace, float dust) without the track actually changing.
    const norm = (s) => (s || "").trim().replace(/\s+/g, " ");
    return [m.session_key || "", norm(m.title), norm(m.artist), norm(m.album), Math.round(Number(m.duration_secs) || 0)].join("");
  }

  function mediaShownPos() {
    return window.NexMediaClock.shownPosition(
      mediaAnchor,
      performance.now(),
      mediaLastPushAt,
      mediaLastPushPos,
      stallHold,
    );
  }

  // Awaiting-confirmation jump: a single far-off push may be a stale
  // SMTC read, so large jumps only apply after two consecutive pushes
  // agree. Genuine seeks/track changes confirm within ~1s.
  let pendingJump = null;
  // Seek round-trip: after a drag, pushes sampled before the seek
  // completes are stale — coast optimistically until this wall-clock
  // deadline passes. Set on every drag move so it covers the release.
  let settleUntil = 0;
  // Sample floor (Rust unix-millis): pushes sampled before the last
  // drag/switch are discarded in renderMedia — stale in-flight
  // snapshots must not move the bar backward.
  let anchorFloor = 0;
  // Stall tracking: a source frozen while claiming playing (buffering)
  // would otherwise arm the confirm path and yank the bar backward.
  // Frozen 3s+ → hold the displayed spot until the source moves again.
  let stallPos = null;
  let stallAt = 0;
  let stallHold = null;

  function syncMediaAnchor(m) {
    // Seek round-trip in flight: coast on the optimistic drag target.
    if (performance.now() < settleUntil) return;
    const key = mediaTrackKey(m);
    const pushed = Number(m.position_secs) || 0;
    const now = performance.now();
    const sourceWasStale = mediaLastPushAt > 0 && now - mediaLastPushAt > 4000;
    // Mid-drag: the user owns the bar — only adopt identity changes,
    // never yank the position back to a stale push.
    if (seeking && key === mediaAnchor.key) {
      mediaAnchor.status = m.status;
      return;
    }
    // New track, status flip, or first sighting. The bar is pure
    // calculation: only a new track adopts the pushed position. Status
    // flips keep the calculated spot — pushes jitter and any adoption
    // yanks the bar and timer.
    if (key !== mediaAnchor.key || m.status !== mediaAnchor.status) {
      // Displayed spot before clearing stall state — pausing mid-stall
      // must freeze where the bar visibly was, not the coasted anchor.
      const shownBefore = mediaShownPos();
      pendingJump = null;
      stallPos = null;
      stallHold = null;
      if (key !== mediaAnchor.key) {
        mediaAnchor = { key, status: m.status, pos: pushed, at: now };
      } else if (m.status === "playing") {
        // Resumed: continue from the frozen spot, fresh clock base.
        mediaAnchor = { key, status: m.status, pos: mediaAnchor.pos, at: now };
      } else {
        // Paused: freeze exactly where the bar was, not where the
        // stale push claims.
        mediaAnchor = { key, status: m.status, pos: Math.max(shownBefore, 0), at: now };
      }
      return;
    }
    if (m.status !== "playing") {
      // Paused: frozen. User drags already moved the anchor
      // optimistically; external seeks apply on resume.
      return;
    }
    if (sourceWasStale) {
      mediaAnchor = window.NexMediaClock.resumeFromHold(
        { ...mediaAnchor, key, status: m.status },
        mediaLastPushPos,
        pushed,
        now,
      );
      stallPos = pushed;
      stallAt = now;
      stallHold = null;
      pendingJump = null;
      return;
    }
    // Stall: source frozen while claiming playing (buffering, slow
    // network). Without this the frozen pair agrees, the confirm path
    // adopts it, and the bar yanks backward — then forward on recovery.
    // Instead hold the displayed spot until the source moves again.
    if (stallPos !== null && Math.abs(pushed - stallPos) < 0.5) {
      if (now - stallAt > 3000) {
        if (stallHold === null) stallHold = Math.max(mediaShownPos(), 0);
        pendingJump = null;
        return;
      }
    } else {
      const held = stallHold;
      stallPos = pushed;
      stallAt = now;
      stallHold = null;
      if (held !== null) {
        mediaAnchor = window.NexMediaClock.resumeFromHold(
          { ...mediaAnchor, key, status: m.status },
          held,
          pushed,
          now,
        );
      }
    }
    // Same track, still playing: a large jump is only trusted once a
    // second consecutive push confirms it (real seek or track restart).
    // A lone far-off push is treated as a stale SMTC read and ignored,
    // so one glitchy sample can never yank the bar back and forth.
    // Anything within 3s is ignored outright — the wall-clock calculation
    // coasts through push jitter instead of adopting it.
    if (Math.abs(pushed - mediaShownPos()) > 3) {
      if (pendingJump && Math.abs(pushed - pendingJump.pos) <= 1) {
        pendingJump.hits += 1;
        if (pendingJump.hits >= 2) {
          // Backward-adopt veto: agreeing yet mutually frozen pushes
          // while playing = a stalled source winning the race against
          // the stall block above — not a seek. Hold the displayed
          // spot instead of yanking back. Forward adopts always proceed
          // (recovery, restarts); genuine seek-backs advance ~1s per
          // push and never trip the frozen test.
          if (pushed < mediaShownPos() && Math.abs(pushed - pendingJump.pos) < 0.25) {
            if (stallHold === null) stallHold = Math.max(mediaShownPos(), 0);
            stallPos = pushed;
            stallAt = now;
            pendingJump = null;
            return;
          }
          mediaAnchor = { key, status: m.status, pos: pushed, at: now };
          // Fresh anchor = fresh stall tracking; stale baselines here
          // caused instant false stalls after every adopt.
          stallPos = pushed;
          stallAt = now;
          pendingJump = null;
        }
      } else {
        pendingJump = { pos: pushed, hits: 1 };
      }
    } else {
      // Coast: never adopt mid-track pushes. Each SMTC read jitters and
      // any adoption moves bar + timer backward/forward visibly.
      pendingJump = null;
    }
  }

  function paintMediaProgress() {
    const m = mediaState;
    if (!m) return;
    const dur = Number(m.duration_secs) || 0;
    let pos = Math.min(Math.max(mediaShownPos(), 0), dur || Infinity);
    // Track tail: pushes lag ~1s behind reality, so without this the bar
    // never visually completes before the next track starts. The label
    // floors to whole seconds, so snapping the last second is invisible.
    if (m.status === "playing" && dur > 0 && pos > dur - 1) pos = dur;
    const pct = dur > 0 ? (pos / dur * 100).toFixed(1) + "%" : "0%";
    mediaProgressFill.style.width = pct;
    mediaProgressKnob.style.left = pct;
    const label = fmtTime(pos);
    if (mediaPos.textContent !== label) mediaPos.textContent = label;
  }

  function mediaFrame() {
    mediaRaf = 0;
    if (!mediaOpen) return;
    paintMediaProgress();
    if (mediaState && mediaState.status === "playing") {
      mediaRaf = requestAnimationFrame(mediaFrame);
    }
  }

  function kickMediaFrame() {
    if (mediaOpen && !mediaRaf) mediaRaf = requestAnimationFrame(mediaFrame);
  }

  function renderMedia() {
    if (!mediaOpen) return;
    const m = mediaState;
    if (!m || !m.active) {
      closeMediaView();
      return;
    }
    // Discard pushes sampled before the last drag/switch — stale
    // in-flight snapshots must not move the bar backward.
    const sample = Number(m.sampled_ms) || 0;
    if (sample > 0 && sample < anchorFloor) return;
    // Reopening may render the cached snapshot before its refresh returns.
    // Keep the existing clock until a current sample arrives.
    if (!sample || Date.now() - sample <= 4000) {
      syncMediaAnchor(m);
      mediaLastPushAt = performance.now();
      mediaLastPushPos = Math.max(mediaShownPos(), 0);
    }
    mediaTitle.textContent = m.title || "Unknown track";
    mediaArtist.textContent = [m.artist, m.album].filter(Boolean).join(" — ");
    if (m.art) {
      if (mediaArt.getAttribute("src") !== m.art) mediaArt.setAttribute("src", m.art);
    } else {
      mediaArt.removeAttribute("src");
    }
    const dur = Number(m.duration_secs) || 0;
    const live = !!m.live;
    const seekable = !!m.seekable && !live;
    mediaLive.classList.toggle("hidden", !live);
    mediaDur.textContent = live ? "" : fmtTime(dur);
    mediaProgress.classList.toggle("locked", !seekable);
    const playing = m.status === "playing";
    mediaPlayIcon.classList.toggle("hidden", playing);
    mediaPauseIcon.classList.toggle("hidden", !playing);
    syncVolumeUi(m);
    renderMediaDots(m);
    paintMediaProgress();
    kickMediaFrame();
  }

  // Session switcher dots: one per live app, rebuilt only when the set
  // changes; active/playing classes refresh on every push.
  function renderMediaDots(m) {
    const list = Array.isArray(m.sessions) ? m.sessions : [];
    if (list.length < 2) {
      mediaDots.classList.add("hidden");
      mediaDots.replaceChildren();
      delete mediaDots.dataset.identity;
      return;
    }
    mediaDots.classList.remove("hidden");
    const identity = list.map((s) => s.key).join("|");
    if (mediaDots.dataset.identity !== identity) {
      mediaDots.dataset.identity = identity;
      mediaDots.replaceChildren();
      for (const s of list) {
        const dot = document.createElement("button");
        dot.type = "button";
        dot.dataset.key = s.key;
        dot.setAttribute("role", "tab");
        dot.addEventListener("click", (e) => {
          e.stopPropagation();
          // Fresh anchor on switch — the session-keyed track identity
          // re-adopts on the next push; clear now so nothing glides.
          // Pre-switch snapshots are stale: floor them out by sample time.
          mediaLastPushPos = mediaShownPos();
          mediaLastPushAt = performance.now();
          mediaAnchor.key = "";
          anchorFloor = Date.now();
          stallPos = null;
          stallHold = null;
          pendingJump = null;
          post("mediaSession", s.key);
        });
        mediaDots.appendChild(dot);
      }
    }
    for (const dot of mediaDots.children) {
      const entry = list.find((s) => s.key === dot.dataset.key);
      const label = entry ? (entry.title || entry.source || "Media") : "Media";
      dot.classList.toggle("dot-active", !!entry && entry.key === m.session_key);
      dot.classList.toggle("dot-playing", !!entry && !!entry.playing);
      dot.title = label + (entry && entry.playing ? " (playing)" : "");
      dot.setAttribute("aria-label", "Control " + (entry ? entry.source : "media"));
    }
  }

  function openMediaView() {
    if (mediaOpen || !mediaState || !mediaState.active) return;
    topPower.closeMenu();
    topPower.closeConfirm();
    hideContextMenu();
    mediaOpen = true;
    panel.classList.add("media-open");
    renderMedia();
    kickMediaFrame();
    postMediaResize();
    // Keep playback state fresh while the progress clock is visible.
    post("mediaRefresh");
    if (!mediaRefreshTimer) {
      mediaRefreshTimer = window.setInterval(() => {
        if (mediaOpen) post("mediaRefresh");
      }, 1000);
    }
  }

  function closeMediaView() {
    if (!mediaOpen) return;
    mediaOpen = false;
    if (mediaRefreshTimer) {
      window.clearInterval(mediaRefreshTimer);
      mediaRefreshTimer = 0;
    }
    if (mediaRaf) {
      cancelAnimationFrame(mediaRaf);
      mediaRaf = 0;
    }
    panel.classList.remove("media-open");
    postMediaResize();
  }

  // Click/drag-to-seek on the progress bar. The anchor jumps
  // optimistically; the Rust push after the seek confirms it.
  const mediaProgress = $("media-progress");
  let seeking = false;

  function seekFromEvent(e) {
    const r = mediaProgress.getBoundingClientRect();
    if (!(r.width > 0)) return;
    // Locked bar (ad, live, unseekable): nothing to drag. The class
    // already blocks the pointer; this is belt-and-braces for keyboard.
    if (!mediaState?.seekable) return;
    const ratio = Math.min(Math.max((e.clientX - r.left) / r.width, 0), 1);
    const lo = Number(mediaState.seek_min_secs) || 0;
    const hi = Number(mediaState.seek_max_secs) || 0;
    if (!(hi > lo)) return;
    const target = lo + ratio * (hi - lo);
    mediaAnchor = {
      key: mediaState ? mediaTrackKey(mediaState) : "",
      status: mediaState?.status || "",
      pos: target,
      at: performance.now(),
    };
    mediaLastPushAt = mediaAnchor.at;
    mediaLastPushPos = target;
    // Pre-seek snapshots are stale the moment the drag moves: floor them
    // out by sample time, coast optimistically through the round trip,
    // and drop any stall state from the old spot.
    anchorFloor = Date.now();
    settleUntil = performance.now() + 1200;
    stallPos = null;
    stallHold = null;
    pendingJump = null;
    paintMediaProgress();
    post("mediaSeek", Math.round(target * 1000));
  }

  mediaProgress.addEventListener("pointerdown", (e) => {
    if (!mediaState?.active) return;
    e.preventDefault();
    seeking = true;
    mediaProgress.classList.add("seeking");
    try { mediaProgress.setPointerCapture(e.pointerId); } catch (_) {}
    seekFromEvent(e);
  });
  mediaProgress.addEventListener("pointermove", (e) => {
    if (seeking) seekFromEvent(e);
  });
  const endSeek = () => {
    seeking = false;
    mediaProgress.classList.remove("seeking");
  };
  mediaProgress.addEventListener("pointerup", endSeek);
  mediaProgress.addEventListener("pointercancel", endSeek);

  document.getElementById("media-prev").addEventListener("click", () => post("mediaPrev"));
  document.getElementById("media-toggle").addEventListener("click", () => {
    // Optimistic flip — the Rust refresh corrects the icon within ~1s.
    const playing = mediaPlayIcon.classList.contains("hidden");
    mediaPlayIcon.classList.toggle("hidden", !playing);
    mediaPauseIcon.classList.toggle("hidden", playing);
    post("mediaToggle");
  });
  document.getElementById("media-next").addEventListener("click", () => post("mediaNext"));

  // ── output volume (device endpoint; SMTC has no per-session knob) ──
  let volDragging = false;
  let volPostAt = 0;
  let volPending = null;

  function volumePct(m) {
    const v = Number(m?.volume);
    return Math.min(Math.max(Math.round((isFinite(v) ? v : 1) * 100), 0), 100);
  }

  function paintVolumeSlider(pct) {
    mediaVolumeSlider.value = String(pct);
    mediaVolumeSlider.title = "Volume " + pct;
    mediaVolumeSlider.style.setProperty("--vol", pct + "%");
  }

  function syncVolumeUi(m) {
    if (!m.volume_supported) {
      mediaVolumeInline.classList.add("hidden");
      return;
    }
    mediaVolumeInline.classList.remove("hidden");
    if (!volDragging) {
      paintVolumeSlider(volumePct(m));
    }
    const muted = !!m.muted;
    mediaVolIcon.classList.toggle("hidden", muted);
    mediaMuteIcon.classList.toggle("hidden", !muted);
  }

  function sendVolume(pct) {
    const now = performance.now();
    // Live-drag posts throttled to ~8/s; the trailing value always sends.
    if (now - volPostAt >= 120) {
      volPostAt = now;
      volPending = null;
      post("mediaVolume", pct);
    } else {
      volPending = pct;
      setTimeout(() => {
        if (volPending !== null && performance.now() - volPostAt >= 120) {
          volPostAt = performance.now();
          post("mediaVolume", volPending);
          volPending = null;
        }
      }, 130);
    }
  }

  function nudgeVolume(delta) {
    if (!mediaState?.volume_supported) return;
    const pct = Math.min(Math.max(Number(mediaVolumeSlider.value || 0) + delta, 0), 100);
    paintVolumeSlider(pct);
    sendVolume(pct);
  }

  mediaVolumeSlider.addEventListener("pointerdown", () => { volDragging = true; });
  window.addEventListener("pointerup", () => { volDragging = false; });
  mediaVolumeSlider.addEventListener("input", () => {
    const pct = Math.min(Math.max(Math.round(Number(mediaVolumeSlider.value) || 0), 0), 100);
    paintVolumeSlider(pct);
    sendVolume(pct);
  });
  mediaVolumeSlider.addEventListener("change", () => {
    volDragging = false;
    const pct = Math.min(Math.max(Math.round(Number(mediaVolumeSlider.value) || 0), 0), 100);
    volPostAt = 0;
    volPending = null;
    post("mediaVolume", pct);
  });
  document.getElementById("media-mute").addEventListener("click", () => {
    // Optimistic flip — the Rust refresh corrects the icon within ~1s.
    const nowMuted = mediaVolIcon.classList.contains("hidden");
    mediaVolIcon.classList.toggle("hidden", !nowMuted);
    mediaMuteIcon.classList.toggle("hidden", nowMuted);
    post("mediaMute");
  });

  // ── scrollbar idle fade ────────────────────────────────────
  // Thumb fades out after 1.4s without scroll/hover over the list;
  // reappears on activity.
  let scrollFadeTimer = null;
  function armScrollFade() {
    list.classList.remove("scroll-idle");
    clearTimeout(scrollFadeTimer);
    scrollFadeTimer = setTimeout(() => {
      list.classList.add("scroll-idle");
    }, 1400);
  }
  list.addEventListener("scroll", armScrollFade, { passive: true });
  list.addEventListener("mousemove", () => {
    document.body.classList.remove("keyboard-nav");
    armScrollFade();
  }, { passive: true });
  list.classList.add("scroll-idle");
})();
