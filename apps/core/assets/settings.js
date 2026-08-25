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
  document.getElementById("status").textContent = "";
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
  document.getElementById("status").textContent = r.saved
    ? "Saved"
    : "Error: " + r.error;
};

window.chrome.webview.postMessage(JSON.stringify({ t: "ready" }));

window.pendingHotkey = null;
const MODS = new Set(["Control", "Alt", "Shift", "Meta"]);
let recording = false;
const hkbox = document.getElementById("hotkeyBox");


function showHotkey() {
  hkbox.textContent = window.pendingHotkey || window.currentSettings?.hotkey;
}

hkbox.addEventListener("click", () => {
  recording = true;
  hkbox.classList.add("recording");
  hkbox.textContent = "Press keys...  (Esc to cancel)";
  window.chrome.webview.postMessage(JSON.stringify({ t: "recordHotkey" }));
});

window.hotkeyRecorded = function (combo) {
  hkbox.classList.remove("recording");
  if (combo) window.pendingHotkey = combo;
  showHotkey();
}