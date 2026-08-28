const FIELDS = [
  ["gridView", "checked"],
  ["quickLaunchEnabled", "checked"],
  ["maxResults", "valueNumber"],
  ["quickLaunchMaxItems", "valueNumber"],
  ["indexMaxItemsTotal", "valueNumber"],
];

window.applySettings = function (s) {
  document.documentElement.dataset.theme = s.theme || "dark";
  window.currentSettings = s;
  window.pendingHotkey = null;
  for (const [id, kind] of FIELDS) {
    const el = document.getElementById(id);
    if (!el || s[id] === undefined) continue;
    el[kind === "checked" ? "checked" : "value"] = s[id];
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
    else cfg[id] = el.value.trim();
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