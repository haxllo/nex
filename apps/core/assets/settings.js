const FIELDS = [
  ["gridView", "checked"],
  ["quickLaunchEnabled", "checked"],
  ["maxResults", "valueNumber"],
  ["quickLaunchMaxItems", "valueNumber"],
  ["indexMaxItemsTotal", "valueNumber"],
  ["hotkey", "value"],
];

window.applySettings = function (s) {
  document.documentElement.dataset.theme = s.theme || "dark";
  window.currentSettings = s;
  for (const [id, kind] of FIELDS) {
    const el = document.getElementById(id);
    if (!el || s[id] === undefined) continue;
    el[kind === "checked" ? "checked" : "value"] = s[id];
  }
  document.getElementById("status").textContent = "";
};

function save() {
  const cfg = {};
  for (const [id, kind] of FIELDS) {
    const el = document.getElementById(id);
    if (kind === "checked") cfg[id] = el.checked;
    else if (kind === "valueNumber") cfg[id] = Number(el.value);
    else cfg[id] = el.value.trim();
  }
  window.chrome.webview.postMessage(JSON.stringify({ t: "save", cfg }));
}

window.saveResult = function (r) {
  document.getElementById("status").textContent = r.saved
    ? "Saved ✓"
    : "Error: " + r.error;
};

window.chrome.webview.postMessage(JSON.stringify({ t: "ready" }));

/**function send() {
    const v =document.getElementById("val").value;
    window.chrome.webview.postMessage(JSON.stringify({ t: "setSetting", v}));
    document.getElementById("status").textContent = "sent: " + v;
}
window.chrome.webview.addEventListener("message", (e) => {
    document.getElementById("status").textContent = "host says: " + e.data.v;
});
window.applySettings = function(s) {
    window.currentSettings = s;
    document.getElementById("val").value = s.hotkey;
    document.getElementById("status").textContent = "applied settings: " + JSON.stringify(s);
};

window.saveResult = function (r) {
    document.getElementById("status").textContent = r.saved ? "Saved ✓": "Error: " + r.error;
};
function save() {
    const hotkey = document.getElementById("val").value;
    const cfg = Object.assign({}, window.currentSettings || {}, { hotkey });
    window.chrome.webview.postMessage(JSON.stringify({ t: "ready" }));
}

function save() {
  const cfg = Object.assign({}, window.currentSettings || {}, {
    hotkey: document.getElementById("val").value,
  });
  window.chrome.webview.postMessage(JSON.stringify({ t: "save", cfg }));
}**/