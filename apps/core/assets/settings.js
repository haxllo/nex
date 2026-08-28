const FIELDS = [
  ["gridView", "checked"],
  ["showFiles", "checked"],
  ["showFolders", "checked"],
  ["launchAtStartup", "checked"],
  ["quickLaunchEnabled", "checked"],
  ["quickLaunchAutoFill", "checked"],
  ["maxResults", "valueNumber"],
  ["quickLaunchMaxItems", "valueNumber"],
  ["indexMaxItemsTotal", "valueNumber"],
  ["searchModeDefault", "valueSelect"],
  ["searchDslEnabled", "checked"],
  ["webSearchProvider", "valueSelect"],
];

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
  if (e.key === "Escape") closeAllDropdowns(null);
});
// ───────────────────────────────────────────────────────────────────

window.applySettings = function (s) {
  document.documentElement.dataset.theme = s.theme || "dark";
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
  const statusEl = document.getElementById("status");
  statusEl.classList.remove("error", "ok");
  statusEl.querySelector(".status-text").textContent = "Ready";
  showHotkey();
};

function save() {
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
  window.chrome.webview.postMessage(JSON.stringify({ t: "save", cfg }));
}

window.saveResult = function (r) {
  const el = document.getElementById("status");
  el.classList.remove("error", "ok");
  if (r.saved) {
    el.classList.add("ok");
    el.querySelector(".status-text").textContent = "Saved";
  } else {
    el.classList.add("error");
    el.querySelector(".status-text").textContent = "Error: " + r.error;
  }
};

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