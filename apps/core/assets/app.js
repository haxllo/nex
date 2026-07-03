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
  const help = $("help");

  // Local mirror of pushed state.
  let rows = [];
  let selected = 0;
  let queryEcho = ""; // last query Rust pushed back (avoid input clobber)
  let lastQuerySent = "";
  let inCommandMode = false;
  let rowMap = new Map(); // index → HTMLElement for O(1) selection toggle

  function post(t, v) {
    try {
      window.ipc.postMessage(JSON.stringify(v === undefined ? { t } : { t, v }));
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

  function render() {
    clampSelected();
    const frag = document.createDocumentFragment();

    for (let i = 0; i < rows.length; i++) {
      const r = rows[i];
      if (r.role === "header") {
        const li = document.createElement("li");
        li.className = "section";
        li.textContent = r.title;
        frag.appendChild(li);
        continue;
      }
      if (r.role === "status") {
        const li = document.createElement("li");
        li.className = "section";
        li.style.textTransform = "none";
        li.style.color = "var(--text-faint)";
        li.textContent = r.title;
        frag.appendChild(li);
        continue;
      }

      const li = document.createElement("li");
      li.className = "row" + (r.role === "calculator" ? " calculator" : "");
      li.setAttribute("role", "option");
      li.dataset.index = String(i);
      if (i === selected) li.classList.add("selected");

      if (r.role !== "calculator") {
        if (r.icon && r.kind !== "action") {
          const img = document.createElement("img");
          img.className = "icon";
          img.src = r.icon;
          img.onerror = () => img.classList.add("placeholder");
          li.appendChild(img);
        } else if (r.kind !== "action") {
          const ph = document.createElement("div");
          ph.className = "icon placeholder";
          li.appendChild(ph);
        }
      }

      const text = document.createElement("div");
      text.className = "text";
      const title = document.createElement("div");
      title.className = "title";
      title.textContent = r.title;
      text.appendChild(title);
      if (r.subtitle) {
        const sub = document.createElement("div");
        sub.className = "subtitle";
        sub.textContent = r.subtitle;
        text.appendChild(sub);
      }
      li.appendChild(text);

      if (r.kind && r.role !== "calculator") {
        const kind = document.createElement("div");
        kind.className = "kind";
        kind.textContent = r.kind;
        li.appendChild(kind);
      }

      li.addEventListener("mousemove", () => setSelected(i, false));
      li.addEventListener("click", () => {
        setSelected(i, false);
        post("submit", i);
      });
      frag.appendChild(li);
    }

    // Atomic swap — no flash between clearing and rebuilding.
    list.replaceChildren(frag);

    // Rebuild row map for O(1) selection toggles.
    rowMap = new Map();
    for (const li of list.children) {
      if (li.classList.contains("row")) rowMap.set(Number(li.dataset.index), li);
    }

    // Status / empty state.
    const hasRows = rows.some((r) => r.role !== "status");
    if (!hasRows && statusEl.dataset.text) {
      statusEl.textContent = statusEl.dataset.text;
      statusEl.classList.remove("hidden");
    } else {
      statusEl.classList.add("hidden");
    }

    // Idle state: hide divider + list area and footer when no rows.
    bodyEl.classList.toggle("idle", !hasRows);
    footerEl.classList.toggle("idle", !hasRows);

    scrollToSelected();
    measure();
  }

  function setSelected(i, scroll) {
    if (i === selected) return;
    const prev = selected;
    selected = i;
    const prevEl = rowMap.get(prev);
    if (prevEl) prevEl.classList.remove("selected");
    const nextEl = rowMap.get(selected);
    if (nextEl) nextEl.classList.add("selected");
    if (scroll) scrollToSelected();
    post("select", selected);
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

  // ── command mode ───────────────────────────────────────────
  function updateSearchIcon() {
    searchIcon.style.opacity = "0";
    setTimeout(() => {
      if (inCommandMode) {
        searchIcon.innerHTML =
          '<text x="11" y="17" font-size="20" font-weight="400" fill="var(--text-faint)" text-anchor="middle" font-family="monospace">></text>';
      } else {
        searchIcon.innerHTML =
          '<circle cx="11" cy="11" r="7" fill="none" stroke="var(--text-faint)" stroke-width="2" stroke-linecap="round"></circle><line x1="21" y1="21" x2="16.65" y2="16.65" stroke="var(--text-faint)" stroke-width="2" stroke-linecap="round"></line>';
      }
      searchIcon.style.opacity = "1";
    }, 130);
  }

  // ── height measurement (resize native window to hug content) ──
  let lastH = 0;
  function measure() {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const h = Math.ceil(panel.getBoundingClientRect().height);
        post("painted");
        if (h !== lastH && h > 0) {
          lastH = h;
          post("resize", h);
        }
      });
    });
  }

  // ── keyboard ─────────────────────────────────────────────
  window.addEventListener(
    "keydown",
    (e) => {
      // ── command mode: `>` to enter, backspace-on-empty to exit ──
      if (e.key === ">" && !inCommandMode && document.activeElement === input) {
        e.preventDefault();
        inCommandMode = true;
        input.value = "";
        queryEcho = "";
        updateSearchIcon();
        post("query", ">");
        return;
      }
      if (e.key === "Backspace" && inCommandMode && input.value === "") {
        e.preventDefault();
        inCommandMode = false;
        updateSearchIcon();
        post("query", "");
        return;
      }

      if (e.key === "ArrowDown" || (e.ctrlKey && (e.key === "j" || e.key === "J"))) {
        e.preventDefault();
        moveSelection(1);
      } else if (e.key === "ArrowUp" || (e.ctrlKey && (e.key === "k" || e.key === "K"))) {
        e.preventDefault();
        moveSelection(-1);
      } else if (e.key === "Enter") {
        e.preventDefault();
        if (selected >= 0) post("submit", selected);
      } else if (e.key === "Escape") {
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

  // ── query input (adaptive debounce) ──────────────────────
  // First char of each typing burst fires immediately (0ms).
  // Subsequent rapid chars coalesce at 80ms so SearchWorker
  // drains stale requests from its mpsc channel.
  let debounce = null;
  let lastInputTime = 0;
  input.addEventListener("input", () => {
    let raw = input.value;
    // In command mode the `>` prefix is kept out of the display
    // input — keydown handles enter/exit, `input` just sends
    // the text content.
    if (raw.startsWith(">")) {
      inCommandMode = true;
      raw = raw.slice(1);
      input.value = raw;
    }
    const query = inCommandMode ? ">" + raw : raw;
    if (raw === queryEcho && query === lastQuerySent) return;
    lastQuerySent = query;
    const now = performance.now();
    const delay = (now - lastInputTime > 300) ? 0 : 80;
    lastInputTime = now;
    clearTimeout(debounce);
    debounce = setTimeout(() => post("query", query), delay);
  });

  help.addEventListener("click", () => post("openConfig"));

  // ── Rust → JS bridge ─────────────────────────────────────
  window.nex = {
    apply(state) {
      // Lightweight selection-only update (no rows = incremental).
      if (!Array.isArray(state.rows) && typeof state.selected === "number") {
        setSelected(state.selected, true);
        return;
      }

      if (state.theme) document.documentElement.dataset.theme = state.theme;

      // Only overwrite the input if Rust changed it out from under us
      // (e.g. clear on hide, quick-shortcut expansion).
      if (typeof state.query === "string") {
        let display = state.query;
        let wasCmd = inCommandMode;
        if (display.startsWith(">")) { inCommandMode = true; display = display.slice(1); }
        else { inCommandMode = false; }
        if (wasCmd !== inCommandMode) updateSearchIcon();
        if (display !== input.value) {
          queryEcho = display;
          input.value = display;
        }
      }

      rows = Array.isArray(state.rows) ? state.rows : [];
      selected = typeof state.selected === "number" ? state.selected : 0;

      if (state.placeholder) {
        input.placeholder = state.placeholder;
      } else {
        input.placeholder = "Search for apps, files and actions…";
      }

      statusEl.dataset.text = state.status || "";

      render();
    },

    focus() {
      input.focus();
      input.select();
    },
  };

  // Tell Rust the page is ready to receive state.
  // Do NOT call measure() here — it posts "painted" which races with
  // the first push_state.  painted must only fire after nex.apply()
  // renders the pushed state, otherwise the window appears blank.
  post("ready");
})();
