const FIELDS = [
  ["gridView", "checked"],
  ["showFiles", "checked"],
  ["showFolders", "checked"],
  ["launchAtStartup", "checked"],
  ["quickLaunchAutoFill", "checked"],
  ["maxResults", "valueNumber"],
  ["quickLaunchMaxItems", "valueNumber"],
  ["indexMaxItemsTotal", "valueNumber"],
  ["searchModeDefault", "valueSelect"],
  ["panelBackground", "valueSelect"],
  ["viewMode", "valueSelect"],
  ["searchDslEnabled", "checked"],
  ["webSearchProvider", "valueSelect"],
];

window.discoveryRoots = [];

function rootKey(value) {
  return value.trim().replace(/[\\/]+$/, "").toLowerCase();
}

function renderDiscoveryRoots() {
  const list = document.getElementById("discoveryRootsList");
  if (!list) return;
  list.innerHTML = "";
  if (!window.discoveryRoots.length) {
    const empty = document.createElement("div");
    empty.className = "root-empty";
    empty.textContent = "No folders configured yet.";
    list.appendChild(empty);
    return;
  }
  window.discoveryRoots.forEach((root, index) => {
    const row = document.createElement("div");
    row.className = "row root-row";
    const label = document.createElement("div");
    label.className = "row-label";
    const name = document.createElement("span");
    name.className = "name root-path";
    name.textContent = root;
    label.appendChild(name);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "btn root-remove";
    remove.textContent = "Remove";
    remove.addEventListener("click", () => removeDiscoveryRoot(index));
    row.appendChild(label);
    row.appendChild(remove);
    list.appendChild(row);
  });
}

function addDiscoveryRoot() {
  const input = document.getElementById("newDiscoveryRoot");
  if (!input) return;
  const value = input.value.trim().replace(/^["']+|["']+$/g, "").trim();
  if (!value) return;
  const key = rootKey(value);
  if (window.discoveryRoots.some((root) => rootKey(root) === key)) {
    input.select();
    return;
  }
  window.discoveryRoots.push(value);
  input.value = "";
  renderDiscoveryRoots();
  refreshDirty();
}

function removeDiscoveryRoot(index) {
  if (index < 0 || index >= window.discoveryRoots.length) return;
  window.discoveryRoots.splice(index, 1);
  renderDiscoveryRoots();
  refreshDirty();
}

// ── Live indexing status ────────────────────────────────────────────
window.indexingPoll = null;
window.indexingMode = null;
window.indexingSeen = false;
window.indexingStartedAt = 0;

function startIndexingPoll(mode) {
  stopIndexingPoll();
  window.indexingMode = mode;
  window.indexingSeen = false;
  window.indexingStartedAt = Date.now();
  window.indexingPoll = setInterval(() => {
    window.chrome.webview.postMessage(JSON.stringify({ t: "indexingStatus" }));
  }, 1500);
}

function stopIndexingPoll() {
  if (window.indexingPoll) {
    clearInterval(window.indexingPoll);
    window.indexingPoll = null;
  }
}

window.indexingStatus = function (s) {
  if (!s || typeof s.indexing !== "boolean") return;
  if (s.indexing) {
    window.indexingSeen = true;
    return;
  }
  const waited = Date.now() - window.indexingStartedAt;
  // The indexer polls the config file every 2 s, so a false right after the
  // save means "hasn't started yet", not "done".
  if (!window.indexingSeen && waited < 8000) return;
  stopIndexingPoll();
  document.getElementById("rootsIndexNote")?.classList.add("hidden");
  if (waited <= 300000) {
    const status = document.getElementById("status");
    if (status) {
      status.classList.remove("error");
      status.classList.add("ok");
      status.querySelector(".status-text").textContent =
        window.indexingMode === "removing" ? "Saved — folder removed" : "Saved — index updated";
    }
  }
  window.indexingMode = null;
};

// ── Dropdowns ───────────────────────────────────────────────────────
const CHECK_SVG = '<svg class="dd-check" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 6.5l2.5 2.5L10 3.5"/></svg>';

function closeAllDropdowns(except) {
  document.querySelectorAll(".dropdown.open").forEach((d) => {
    if (d !== except) d.classList.remove("open");
  });
}

function initDropdown(root) {
  const hidden = root.querySelector('input[type="hidden"]');
  const trigger = root.querySelector(".dd-trigger");
  const labelEl = root.querySelector(".dd-label");
  const popup = root.querySelector(".dd-popup");
  let options = [];
  try { options = JSON.parse(root.dataset.options || "[]"); } catch (e) { options = []; }

  // Build items
  popup.innerHTML = options
    .map(
      (o) =>
        `<div class="dd-item" role="option" data-value="${o.v}">${CHECK_SVG}<span class="dd-label-text">${o.l}</span></div>`
    )
    .join("");

  function syncLabel() {
    const v = hidden.value;
    const opt = options.find((o) => o.v === v);
    if (opt) labelEl.textContent = opt.l;
    const items = popup.querySelectorAll(".dd-item");
    items.forEach((it) => {
      const match = it.dataset.value === v;
      it.classList.toggle("selected", match);
      it.setAttribute("aria-selected", match ? "true" : "false");
    });
  }

  function setActiveItem(items, idx) {
    items.forEach((it, i) => it.classList.toggle("active", i === idx));
    if (idx >= 0 && items[idx]) {
      items[idx].scrollIntoView({ block: "nearest" });
    }
  }

  function open() {
    closeAllDropdowns(root);
    root.classList.add("open");
    // Position popup under the trigger (fixed positioning escapes overflow:hidden ancestors)
    const r = trigger.getBoundingClientRect();
    const popupWidth = Math.max(r.width, 160);
    popup.style.left = Math.round(r.right - popupWidth) + "px";
    popup.style.top = Math.round(r.bottom + 6) + "px";
    popup.style.minWidth = popupWidth + "px";
    // Highlight currently selected item
    const items = popup.querySelectorAll(".dd-item");
    const curIdx = Array.from(items).findIndex((it) => it.dataset.value === hidden.value);
    setActiveItem(items, curIdx);
    if (curIdx >= 0) items[curIdx].scrollIntoView({ block: "nearest" });
  }

  function close() {
    root.classList.remove("open");
  }

  function selectValue(v) {
    if (hidden.value === v) {
      close();
      return;
    }
    hidden.value = v;
    // Notify the change so any listeners (e.g. live preview) can react
    hidden.dispatchEvent(new Event("change", { bubbles: true }));
    syncLabel();
    close();
    trigger.focus();
  }

  // Click trigger
  trigger.addEventListener("click", (e) => {
    e.stopPropagation();
    if (root.classList.contains("open")) close();
    else open();
  });

  // Item clicks (delegated)
  popup.addEventListener("click", (e) => {
    const item = e.target.closest(".dd-item");
    if (!item) return;
    selectValue(item.dataset.value);
  });

  // Hover sets active
  popup.addEventListener("mousemove", (e) => {
    const item = e.target.closest(".dd-item");
    if (!item) return;
    const items = popup.querySelectorAll(".dd-item");
    const idx = Array.from(items).indexOf(item);
    setActiveItem(items, idx);
  });

  // Keyboard
  trigger.addEventListener("keydown", (e) => {
    const items = Array.from(popup.querySelectorAll(".dd-item"));
    let activeIdx = items.findIndex((it) => it.classList.contains("active"));
    if (activeIdx < 0) {
      activeIdx = items.findIndex((it) => it.dataset.value === hidden.value);
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!root.classList.contains("open")) {
        open();
        return;
      }
      const dir = e.key === "ArrowDown" ? 1 : -1;
      const next = (activeIdx + dir + items.length) % items.length;
      setActiveItem(items, next);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (!root.classList.contains("open")) open();
      else if (activeIdx >= 0) selectValue(items[activeIdx].dataset.value);
    } else if (e.key === "Escape") {
      close();
    }
  });

  root.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && root.classList.contains("open")) close();
  });

  // Expose for applySettings to call after setting .value
  root._syncLabel = syncLabel;
  syncLabel();
}

function initDropdowns() {
  document.querySelectorAll(".dropdown[data-options]").forEach(initDropdown);
}

// Close popups on outside click
document.addEventListener("mousedown", (e) => {
  if (!e.target.closest(".dropdown")) closeAllDropdowns(null);
});
document.addEventListener("keydown", (e) => {
  const dialog = document.getElementById("unsaved-dialog");
  if (dialog && !dialog.classList.contains("hidden") && e.key === "Tab") {
    const focusable = [...dialog.querySelectorAll("button")];
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }
  if (e.key === "Escape") closeAllDropdowns(null);
});
// ───────────────────────────────────────────────────────────────────

window.applySettings = function (s) {
  document.documentElement.dataset.theme = s.theme || "dark";
  document.getElementById("app-version").textContent = s.version ? `v${s.version}` : "";
  window.currentSettings = s;
  window.pendingHotkey = null;
  for (const [id, kind] of FIELDS) {
    const el = document.getElementById(id);
    if (!el || s[id] === undefined) continue;
    if (kind === "checked") el.checked = s[id];
    else if (kind === "valueNumber") el.value = s[id];
    else if (kind === "valueSelect") {
      // For dropdowns, write the hidden input and refresh the label
      const hidden = el.querySelector('input[type="hidden"]');
      if (hidden) {
        hidden.value = s[id];
        if (typeof el._syncLabel === "function") el._syncLabel();
      } else {
        el.value = s[id];
      }
    } else el.value = s[id];
  }
  window.discoveryRoots = Array.isArray(s.discoveryRoots) ? s.discoveryRoots.slice() : [];
  renderDiscoveryRoots();
  document.getElementById("rootsIndexNote")?.classList.add("hidden");
  window.rootsChanged = false;
  window.rootsAdded = 0;
  window.rootsRemoved = 0;
  stopIndexingPoll();
  const statusEl = document.getElementById("status");
  statusEl.classList.remove("error", "ok");
  statusEl.querySelector(".status-text").textContent = "Ready";
  showHotkey();
  window.settingsBaseline = settingsState();
  window.settingsDirty = false;
  window.closeAfterSave = false;
};

function settingsState() {
  const cfg = {};
  for (const [id, kind] of FIELDS) {
    const el = document.getElementById(id);
    if (kind === "checked") cfg[id] = el.checked;
    else if (kind === "valueNumber") cfg[id] = Number(el.value);
    else if (kind === "valueSelect") cfg[id] = el.querySelector('input[type="hidden"]')?.value || el.value;
  }
  cfg.hotkey = window.pendingHotkey || window.currentSettings?.hotkey || "";
  cfg.discoveryRoots = window.discoveryRoots.slice();
  return JSON.stringify(cfg);
}

function refreshDirty() {
  window.settingsDirty = window.settingsBaseline !== settingsState();
}

window.requestCloseSettings = function () {
  refreshDirty();
  if (window.settingsDirty) {
    window.settingsDialogReturnFocus = document.activeElement;
    document.getElementById("unsaved-dialog")?.classList.remove("hidden");
    document.querySelector("#unsaved-dialog button")?.focus();
    return;
  }
  window.chrome.webview.postMessage(JSON.stringify({ t: "close" }));
};

function discardSettings() {
  window.closeAfterSave = false;
  document.getElementById("unsaved-dialog")?.classList.add("hidden");
  window.settingsDialogReturnFocus?.focus();
  window.settingsDialogReturnFocus = null;
  window.chrome.webview.postMessage(JSON.stringify({ t: "close" }));
}

function save(closeAfterSave = false) {
  const cfg = {};
  for (const [id, kind] of FIELDS) {
    const el = document.getElementById(id);
    if (kind === "checked") cfg[id] = el.checked;
    else if (kind === "valueNumber") cfg[id] = Number(el.value);
    else if (kind === "valueSelect") {
      // Read from the hidden input inside the dropdown root
      const hidden = el.querySelector ? el.querySelector('input[type="hidden"]') : null;
      cfg[id] = hidden ? hidden.value : el.value;
    } else cfg[id] = el.value.trim();
  }
  cfg.hotkey = window.pendingHotkey || window.currentSettings.hotkey;
  cfg.discoveryRoots = window.discoveryRoots.slice();
  const previousRoots = new Set((window.currentSettings?.discoveryRoots || []).map(rootKey));
  const nextRoots = new Set(cfg.discoveryRoots.map(rootKey));
  const addedRoots = [...nextRoots].filter((key) => !previousRoots.has(key));
  const removedRoots = [...previousRoots].filter((key) => !nextRoots.has(key));
  window.rootsAdded = addedRoots.length;
  window.rootsRemoved = removedRoots.length;
  window.rootsChanged = addedRoots.length > 0 || removedRoots.length > 0;
  stopIndexingPoll();
  window.closeAfterSave = closeAfterSave;
  window.chrome.webview.postMessage(JSON.stringify({ t: "save", cfg }));
}

window.saveResult = function (r) {
  const el = document.getElementById("status");
  el.classList.remove("error", "ok");
  if (r.saved) {
    el.classList.add("ok");
    const note = document.getElementById("rootsIndexNote");
    const noteText = document.getElementById("rootsIndexNoteText");
    if (window.rootsAdded > 0) {
      noteText.textContent = window.rootsAdded === 1
        ? "Indexing new folder — usually a few seconds."
        : `Indexing ${window.rootsAdded} new folders — usually a few seconds.`;
      note.title =
        "Large drives or slow USB/exFAT folders can take a minute or more. Existing results stay searchable; you can close Settings.";
      note.classList.remove("hidden");
      el.querySelector(".status-text").textContent = "Saved — indexing";
      startIndexingPoll("adding");
    } else if (window.rootsRemoved > 0) {
      noteText.textContent = window.rootsRemoved === 1
        ? "Removing folder from the index — about a minute."
        : `Removing ${window.rootsRemoved} folders from the index — about a minute.`;
      note.title =
        "The indexer rescans the remaining roots before pruning. Existing results stay searchable until the cycle finishes.";
      note.classList.remove("hidden");
      el.querySelector(".status-text").textContent = "Saved — removing";
      startIndexingPoll("removing");
    } else {
      stopIndexingPoll();
      note.classList.add("hidden");
      el.querySelector(".status-text").textContent = "Saved";
    }
    window.currentSettings = { ...window.currentSettings, ...readSettingsForBaseline() };
    window.settingsBaseline = settingsState();
    window.settingsDirty = false;
    if (window.closeAfterSave) {
      window.closeAfterSave = false;
      window.chrome.webview.postMessage(JSON.stringify({ t: "close" }));
    }
  } else {
    el.classList.add("error");
    el.querySelector(".status-text").textContent = "Error: " + r.error;
  }
};

function readSettingsForBaseline() {
  const raw = JSON.parse(settingsState());
  return raw;
}

window.confirmSaveAndClose = function () {
  document.getElementById("unsaved-dialog")?.classList.add("hidden");
  save(true);
};

document.getElementById("newDiscoveryRoot")?.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    addDiscoveryRoot();
  }
});
window.addEventListener("beforeunload", stopIndexingPoll);

initDropdowns();
window.chrome.webview.postMessage(JSON.stringify({ t: "ready" }));

window.pendingHotkey = null;
const MODS = new Set(["Control", "Alt", "Shift", "Meta"]);
let recording = false;
const hkbox = document.getElementById("hotkeyBox");
const hkHint = document.getElementById("hotkeyHint");
const HINT_IDLE = "Click the chip, then press your desired combo";
const HINT_RECORDING = "Listening for keys\u2026 press Esc to cancel";
const HINT_CAPTURED = "New shortcut captured. Click Save changes to apply.";

const WIN_SVG =
  '<svg viewBox="0 0 14 14" aria-hidden="true">' +
  '<path d="M0 1.4h6.2V6.6H0zM7.8 1.4H14V6.6H7.8zM0 7.4h6.2V12.6H0zM7.8 7.4H14V12.6H7.8z"/>' +
  "</svg>";

const KEY_LABELS = {
  Control: "Ctrl",
  Meta: "Win",
  " ": "Space",
  ArrowUp: "\u2191",
  ArrowDown: "\u2193",
  ArrowLeft: "\u2190",
  ArrowRight: "\u2192",
  Enter: "\u21B5",
  Tab: "Tab",
  Escape: "Esc",
  Backspace: "\u232B",
  Delete: "Del",
  Home: "Home",
  End: "End",
  PageUp: "PgUp",
  PageDown: "PgDn",
};

function labelFor(part) {
  if (KEY_LABELS[part]) return KEY_LABELS[part];
  if (part === "Space") return "Space";
  if (/^F\d{1,2}$/.test(part)) return part;
  return part;
}

function makeCap(part) {
  const el = document.createElement("span");
  el.className = "hk-cap";
  if (part === "Win") {
    el.classList.add("win");
    el.innerHTML = WIN_SVG;
  } else {
    el.textContent = labelFor(part);
  }
  return el;
}

function renderCombo(combo) {
  hkbox.replaceChildren();
  if (!combo) {
    const el = document.createElement("span");
    el.className = "hk-cap empty";
    el.textContent = "\u2014";
    hkbox.appendChild(el);
    return;
  }
  const parts = combo.split("+").map(p => p.trim()).filter(Boolean);
  for (const part of parts) {
    hkbox.appendChild(makeCap(part));
  }
}

function showHotkey() {
  renderCombo(window.pendingHotkey || window.currentSettings?.hotkey);
}

hkbox.addEventListener("click", () => {
  recording = true;
  hkbox.classList.add("recording");
  renderCombo(null);
  hkHint.textContent = HINT_RECORDING;
  hkbox.focus({ preventScroll: true });
  window.chrome.webview.postMessage(JSON.stringify({ t: "recordHotkey" }));
});

hkbox.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    hkbox.click();
  }
});

document.addEventListener("keydown", (e) => {
  if (!recording) return;
  if (e.key === "Escape") {
    recording = false;
    hkbox.classList.remove("recording");
    hkbox.blur();
    window.chrome.webview.postMessage(JSON.stringify({ t: "cancelRecord" }));
    hkHint.textContent = HINT_IDLE;
    showHotkey();
  }
  e.preventDefault();
  e.stopPropagation();
});

window.hotkeyRecorded = function (combo) {
  recording = false;
  hkbox.classList.remove("recording");
  hkbox.blur();
  if (combo) {
    window.pendingHotkey = combo;
    hkHint.textContent = HINT_CAPTURED;
  } else {
    hkHint.textContent = HINT_IDLE;
  }
  showHotkey();
};

window.applySettings = (function (orig) {
  return function (s) {
    orig(s);
    hkHint.textContent = HINT_IDLE;
  };
})(window.applySettings);
