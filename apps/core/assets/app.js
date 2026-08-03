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
  const powerBtn = $("power-btn");
  const powerMenu = $("power-menu");
  const powerConfirm = $("power-confirm");
  const powerConfirmTitle = $("power-confirm-title");
  const powerConfirmYes = $("power-confirm-yes");
  const powerWrapTop = $("power-wrap-top");
  const powerBtnTop = $("power-btn-top");
  const contextMenu = $("context-menu");

  // Local mirror of pushed state.
  let rows = [];
  let selected = 0;
  let queryEcho = ""; // last query Rust pushed back (avoid input clobber)
  let lastQuerySent = "";
  let inCommandMode = false;
  let rowMap = new Map(); // index → HTMLElement for O(1) selection toggle
  let quickLaunchItems = []; // Quick Launch items for idle state
  let pendingShow = false; // show occurred, waiting for first real results

  // Persistent icon cache — survives DOM rebuilds across state pushes.
  // Key: icon path (string), Value: data URI (string).
  const iconCache = new Map();

  // Themed fallback shown while real icon loads (cold cache).
  // 24×24 app icons, base64-encoded PNGs.
  const PLACEHOLDER_ICON_LIGHT = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAYAAADgdz34AAAACXBIWXMAAA7DAAAOwwHHb6hkAAAAGXRFWHRTb2Z0d2FyZQB3d3cuaW5rc2NhcGUub3Jnm+48GgAAAV5JREFUSIntlbFKXEEUhr+jxhhdJGATU6bcJghi8A2CmJAttg8+gewbJM3uK2yxWlpZpcozWC9pbEJE0GaNQXdJlC/NCCOLendvtgj4N/fMOTPzzX8uzMCEFflAfQJUSuw3iIj+UFadUzvqwHK6Ur+qy7ccqJ+AbaABfC/h4DnQAr5FxDuAmVR4C+xGREfdAg6AV8BP4A/wIiL2ixDUBaCtTkfE9VTKV4DzFJ8BfeAXcAFcZrUiOgeeArO5A4CqWk/xSvouZSerU0xr+eDmH3SB6ginLKL5iOhPPTyvnB4B/zdgH3gD1IAOcPovAQPgI7AFrAJt4CWwDjSB7q3ZanfES+13uiDzdcdqW91Un5UFqG6orTtqF2ozb1FvjDa9B77cUfsBHOYOGmM4OFJn1JMst6e+HsKp0+rntGgUrao7Ke6ri2N04n6ptQQo9F6MA6ioPfXDRAAJMjuxzR/SX5si3xbNsX0KAAAAAElFTkSuQmCC";
  const PLACEHOLDER_ICON_DARK = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAYAAADgdz34AAAACXBIWXMAAAsSAAALEgHS3X78AAAAGXRFWHRTb2Z0d2FyZQB3d3cuaW5rc2NhcGUub3Jnm+48GgAAATlJREFUSInt1btKQ0EQBuBP4xWDCDbaWtqI4O0RRFS08AV8AvENFFEfQQu1tEpl5TPYKjY2ogjaeMMLXojFLuQQTsJJQhDBH5azZ/7d/ZmZnVmajJay/3bkGzjvHW9pRBf24oJiA+MLxxgs92ANK1jFZQMe9GEb55iDtkhM4yB6sYwTDOERnxhAIaNID3aRw3drNObxFOcPQhyf8YLXBJcFT+hER9IDGMZSnI/Gb3+CX5INE2nGM40lN210Q6sm41/gbwsUMIlFoUDvqi2u9Zq+CcW5gw2MC5U7hU2cite0XoEPoUEm990ILWK2/PB6BIqYERpbGveCLUo5uK8WvwqYx1EF7goXScNqHR5cC73sNmE7xEiaYg7rcVMtImPYV0p8b/YAZMdiFMj6XtSMvJDDhWYJEB+XX8EPVDecpJK7ij8AAAAASUVORK5CYII=";
  function placeholderIcon() {
    return document.documentElement.dataset.theme === "light" ? PLACEHOLDER_ICON_DARK : PLACEHOLDER_ICON_LIGHT;
  }

  const WEB_ICON_LIGHT = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAYAAADgdz34AAAACXBIWXMAAA7DAAAOwwHHb6hkAAAAGXRFWHRTb2Z0d2FyZQB3d3cuaW5rc2NhcGUub3Jnm+48GgAAActJREFUSImtlTtLXFEUhde5CFbOCEGw1FGxyOsPxGmipaOdEqb0F+QPZEidpE0gCfkHKdIp2AWxEu3EQlNGhhBnJpjCx2dx9x0vJ+e+jAsO3LP32mvd8+BsqQKABtCoUlMk2ASa9v0auLbRSeUX7iI8D+wCPaAOrPIvVizXA3aAubLiL4A/JvIGcMBBwGDfcm9tPgDWi8RXgcuUyAywFBBPsGicBBdAK0v8CfA3RT62+Iccg/fGOU7FzoFHIYMtr/izxY9yDA6N88WLbye6I0aYlLRnI8E3YFTS14JtHZX0UdKpF590zv3MPZN7AfFN+B1YfgNo52xPgrZxfZwBUSSpIWk84N2XVCvxjzXj+qhLmookTWQU9o1UxmCQkZuIJFFC5K4gktTNSNYk9UqI9CWNZeS6kaQTSWcZBqG9DRmEzqon6UeJ+v+DkyTggaQNL7ct6UDSywKNd5KeSnruxT85534NZ8Cmd4/v5alIGzwmfqgSVHnsTlKxc+BhcK3ETaTqcz2bml8Ay7kbCqwTNw+Im4kjbi4+9o2fbjhrBec1NJkjboNJy1wJGLS4bZnfgdlS4p7RArdNvwNc2XhlsSbwrLJwjuE0MF2l5gYmrTzD6bhmfAAAAABJRU5ErkJggg==";
  const WEB_ICON_DARK = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAYAAADgdz34AAAACXBIWXMAAA7DAAAOwwHHb6hkAAAAGXRFWHRTb2Z0d2FyZQB3d3cuaW5rc2NhcGUub3Jnm+48GgAAAaZJREFUSIm11b9qFGEUBfDfDkIqN4IEUmo0WPjvBUwatTSbzhBS+gS+gMFabRVUfAMLO4V0IlaiXbBILJUgupsQi6hrMXfIOH6TmV2TA4fhO/d+587dYe9lNMwEDw3zQbiL38HVUnxuHONzeIs+JrGIYYW9iPXxBrNtzZexEyb30MGHRIH3Ebsf520sNZkv4mfJ5AyuJ8wLXouc4ryHhTrzS/hRSt4I/dEBBR5GzkZJ28WFVIFXlctPQ/94QIH1yHlW0dcK02PxnMa7YIEXmMDzupYDE3iMLxV9Gp8b7h4OOvjm3/ZnsJLQq1yJ3Kr+HVkWwROJwgN0W7xgN3KrmMSpDFM1FweR1KbAdk1sKpO3c1QYZtiqCXblY6AJAxyviW1l2JR/kFSB1G+bKpD6Vn18anH//9CJ50ncqsTW5EPudoPHA1zG1Yr+BF/LwktHMCrKuCgfVOMMu82Stovzde32jD6uz5bOe7hRZ15gSf7HGcqXSUe+XFILh78Xzs0m8wKz8jVYrMxeosCC/ZX5OjoZGXP2l/4qfgXvhDaPK+MY1+F0sDX+ALpftxiqTJeOAAAAAElFTkSuQmCC";
  function webIcon() {
    return document.documentElement.dataset.theme === "light" ? WEB_ICON_DARK : WEB_ICON_LIGHT;
  }

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

  // ── toast notification ────────────────────────────────────
  // ── pin/unpin icons ────────────────────────────────────────
  const pinIconSvg = `<svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="var(--text-faint)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 3L5 15L9 11L13 15L13 3Z"/></svg>`;
  const pinIconPinnedSvg = `<svg width="18" height="18" viewBox="0 0 18 18" fill="var(--accent)" stroke="var(--accent)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 3L5 15L9 11L13 15L13 3Z"/></svg>`;
  const addIconSvg = `<svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="var(--text-faint)" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 3L5 15L9 11L13 15L13 3Z"/></svg>`;

  function createPinIcon(item, index) {
    const pinIcon = document.createElement('div');
    pinIcon.className = 'pin-icon' + (item.pinned ? ' pinned' : '');
    pinIcon.innerHTML = item.pinned ? pinIconPinnedSvg : pinIconSvg;
    pinIcon.addEventListener('click', (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (item.pinned) {
        post('unpin', item.title);
      } else {
        post('pin', item.title);
      }
      input.focus();
    });
    return pinIcon;
  }

  function isItemPinned(filePath) {
    if (!filePath) return false;
    const normalized = filePath.replace(/\\/g, '/').toLowerCase();
    return quickLaunchItems.some(item => {
      const itemPath = (item.path || '').replace(/\\/g, '/').toLowerCase();
      return itemPath === normalized && item.pinned;
    });
  }

  function createAddIcon(item) {
    const addIcon = document.createElement('div');
    const filePath = item.filePath || item.icon;
    const pinned = isItemPinned(filePath);
    addIcon.className = 'add-icon' + (pinned ? ' pinned' : '');
    addIcon.innerHTML = pinned ? pinIconPinnedSvg : addIconSvg;
    addIcon.addEventListener('click', (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (filePath) {
        if (pinned) {
          post('unpin', item.title);
        } else {
          post('addToQuickLaunch', filePath);
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
      li.className = "row" + (r.role === "calculator" ? " calculator" : "") + (r.role === "quick_launch" ? " quick-launch" : "");
      li.setAttribute("role", "option");
      li.dataset.index = String(i);
      if (i === selected) li.classList.add("selected");

      if (r.role !== "calculator") {
        if (r.icon && r.kind !== "action") {
          const img = document.createElement("img");
          img.className = "icon";
          img.dataset.iconPath = r.icon; // store path for patchIcons()
          if (iconCache.has(r.icon)) {
            img.src = iconCache.get(r.icon);
          } else {
            img.src = placeholderIcon(); // theme-aware fallback
          }
          // Don't add placeholder class here — patchIcons() will set
          // src and the browser handles loading. Only onerror triggers
          // placeholder.
          img.onerror = () => img.classList.add("placeholder");
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
      if (r.subtitle) {
        const sub = document.createElement("div");
        sub.className = "subtitle";
        sub.textContent = r.subtitle;
        text.appendChild(sub);
      }
      li.appendChild(text);

      // Quick Launch row: add pin/bookmark icon
      if (r.role === "quick_launch") {
        const quickLaunchItem = quickLaunchItems.find(item => item.title === r.title);
        if (quickLaunchItem) {
          li.appendChild(createPinIcon(quickLaunchItem, i));
        }
      } else if (r.kind === "app" && r.role !== "calculator") {
        // App row: add "+" icon to add to Quick Launch
        li.appendChild(createAddIcon(r));
      } else if (r.kind && r.role !== "calculator") {
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
      li.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        setSelected(i, false);
        showContextMenu(e.clientX, e.clientY, r);
      });
      frag.appendChild(li);
    }

    // Hide body while swapping to prevent quick-launch rows leaking
    // into search results for one frame as the layout recomputes.
    bodyEl.style.visibility = "hidden";

    // Atomic swap — no flash between clearing and rebuilding.
    list.replaceChildren(frag);
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
    const wasIdle = bodyEl.classList.contains("idle");
    bodyEl.classList.toggle("idle", !hasRows);
    bodyEl.style.visibility = "";
    footerEl.classList.toggle("idle", !hasRows);

    // Idle: the power button replaces the config button in the search row.
    const idle = !hasRows;
    help.classList.toggle("hidden", idle);
    powerWrapTop.classList.toggle("hidden", !idle);

    // When leaving idle with search results, defer measure() by one
    // animation frame so the panel settles at its new height before
    // we send the first resize IPC — avoids the QL → search flash.
    if (wasIdle && !idle) {
      requestAnimationFrame(() => measure());
    } else {
      measure();
    }
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

  // Grid-aware vertical navigation: jump to same column in prev/next row.
  function moveSelectionGridDown(dy) {
    const sel = selectableIndices();
    if (sel.length === 0) return;
    let pos = sel.indexOf(selected);
    if (pos === -1) pos = 0;

    // Group selectable indices into physical rows by offsetTop.
    const rows = [];
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
      if (row.length > 0) rows.push(row);
    }

    // Find current position in the row grid.
    let cr = -1, cc = -1;
    for (let r = 0; r < rows.length; r++) {
      const c = rows[r].indexOf(selected);
      if (c !== -1) { cr = r; cc = c; break; }
    }
    if (cr === -1) return;

    const tr = cr + dy;
    if (tr < 0 || tr >= rows.length) return;
    // Clamp column to target row width.
    const tc = Math.min(cc, rows[tr].length - 1);
    setSelected(rows[tr][tc], true);
  }

  // ── icon patching ─────────────────────────────────────────
  // Called after icon data arrives. Updates <img> elements from cache.
  // Does NOT skip placeholder elements — on cold cache, render() creates
  // icons without src, and patchIcons() must update them all.
  function patchIcons() {
    for (const li of list.children) {
      const img = li.querySelector("img.icon");
      if (!img) continue;
      const path = img.dataset.iconPath;
      if (path && iconCache.has(path)) {
        const dataUri = iconCache.get(path);
        if (img.src !== dataUri) img.src = dataUri;
      }
    }
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

  // ── height measurement + painted notification ──
  // Sends resize IPC on first content paint so Rust expands the window
  // to match the panel's content height. The panel is already rendered
  // at full height (clipped by overflow:hidden) — no DWM acrylic flash.
  // The first measurement (idle, ~109px) records the height but does NOT
  // send resize — only the transition to real content triggers expansion.
  // Resize IPC is sent immediately — the Rust-side debounce (100ms)
  // coalesces rapid typing requests into a single frame update.
  let lastH = 0;
  let needsPainted = false;
  function measure() {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const h = Math.ceil(panel.getBoundingClientRect().height);
        if (h > 0 && h !== lastH) {
          const prev = lastH;
          lastH = h;
          // First measurement: skip resize only if panel is truly idle
          // (no rows, search bar only). If content is already showing
          // (quick launch items), send resize immediately.
          if (prev > 0 || !bodyEl.classList.contains("idle")) {
            if (skipResize) {
              skipResize = false;
              return;
            }
            // Rust-side debounce (100ms) coalesces rapid typing resize
            // requests — no need for a JS-side debounce here.
            post("resize", h);
          }
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
        if (list.classList.contains("grid-view")) {
          moveSelectionGridDown(1);
        } else {
          moveSelection(1);
        }
      } else if (e.key === "ArrowUp" || (e.ctrlKey && (e.key === "k" || e.key === "K"))) {
        e.preventDefault();
        if (list.classList.contains("grid-view")) {
          moveSelectionGridDown(-1);
        } else {
          moveSelection(-1);
        }
      } else if (e.key === "Enter") {
        e.preventDefault();
        if (selected >= 0) post("submit", selected);
      } else if (e.key === "Escape") {
        if (footerPower.hasConfirm()) {
          footerPower.closeConfirm();
          input.focus();
          return;
        }
        if (footerPower.isOpen()) {
          footerPower.closeMenu();
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

  // ── query input (adaptive debounce) ──────────────────────
  // First char of each typing burst fires immediately (0ms).
  // Subsequent rapid chars coalesce at 40ms so SearchWorker
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
    const delay = (now - lastInputTime > 300) ? 0 : 40;
    lastInputTime = now;
    clearTimeout(debounce);
    debounce = setTimeout(() => post("query", query), delay);
  });

  help.addEventListener("click", () => post("openConfig"));

  // ── power button dropup ──────────────────────────────────
  // Factory wires one power button + menu + confirm panel.
  function makePowerUi(btn, menu, confirm, title, yes) {
    let open = false;
    let confirmAction = null; // "shutdown" | "restart" | null
    const api = {
      closeMenu() {
        open = false;
        menu.classList.add("hidden");
        btn.classList.remove("open");
        input.focus();
      },
      closeConfirm() {
        if (!confirmAction) return;
        confirmAction = null;
        confirm.classList.add("hidden");
      },
      isOpen() { return open; },
      hasConfirm() { return confirmAction !== null; },
      isConfirmTarget(el) { return confirm.contains(el); },
      isMenuTarget(el) { return btn.contains(el) || menu.contains(el); },
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
      menu.classList.toggle("hidden", !open);
      btn.classList.toggle("open", open);
    });

    menu.addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (!b) return;
      const action = b.dataset.power;
      if (!action) return;
      // Destructive actions need an in-overlay confirm panel first.
      if (action === "shutdown" || action === "restart") {
        e.stopPropagation(); // keep this click from closing the panel we're about to open
        confirmAction = action;
        api.closeMenu();
        const isShutdown = action === "shutdown";
        title.textContent = isShutdown ? "Shut down now?" : "Restart now?";
        yes.textContent = isShutdown ? "Shut down" : "Restart";
        confirm.classList.remove("hidden");
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

  const footerPower = makePowerUi(powerBtn, powerMenu, powerConfirm, powerConfirmTitle, powerConfirmYes);
  powerBtnTop.addEventListener("click", (e) => {
    e.stopPropagation();
    post("powerPopup");
  });

  // Close the dropup / confirm when clicking anywhere outside
  document.addEventListener("click", (e) => {
    if (footerPower.hasConfirm() && !footerPower.isConfirmTarget(e.target)) footerPower.closeConfirm();
    if (footerPower.isOpen() && !footerPower.isMenuTarget(e.target)) footerPower.closeMenu();
    if (!contextMenu.classList.contains("hidden") && !contextMenu.contains(e.target)) {
      hideContextMenu();
    }
  });

  // ── context menu ──────────────────────────────────────────
  let ctxRow = null; // the row the context menu was opened on

  function showContextMenu(x, y, row) {
    ctxRow = row;
    // Determine which actions are relevant
    const isApp = row.kind === "app" || row.role === "quick_launch" || (row.kind === "action" && !row.title.startsWith("Search Web"));
    const isFile = row.kind === "file" || row.kind === "folder" || (row.subtitle && row.subtitle.length > 0 && row.kind !== "action");

    const el = contextMenu;
    const btns = el.querySelectorAll("button");

    // Show/hide buttons based on item kind
    btns.forEach(b => {
      const action = b.dataset.action;
      if (action === "open") b.classList.toggle("hidden", false);
      else if (action === "runas") b.classList.toggle("hidden", !isApp && !isFile);
      else if (action === "openfolder") b.classList.toggle("hidden", !row.subtitle);
      else if (action === "copypath") b.classList.toggle("hidden", !row.subtitle);
      else if (action === "pin") {
        const path = row.filePath || row.icon || "";
        const pinned = isItemPinned(path) || isItemPinned(row.icon);
        b.textContent = pinned ? "Unpin from Quick Launch" : "Pin to Quick Launch";
        b.classList.toggle("hidden", row.kind !== "app");
      }
      else if (action === "uninstall") b.classList.toggle("hidden", row.kind !== "app");
    });

    // Temporarily remove hidden to measure actual layout, then position
    el.classList.remove("hidden");
    const menuW = el.offsetWidth || 180;
    const menuH = el.offsetHeight || 0;
    el.classList.add("hidden");

    const pad = 8;
    let left = x + pad;
    if (left + menuW > window.innerWidth - pad) {
      left = x - menuW - pad;
    }
    let top = y + pad;
    if (top + menuH > window.innerHeight - pad) {
      top = y - menuH - pad;
    }
    el.style.left = Math.max(pad, Math.min(left, window.innerWidth - menuW - pad)) + "px";
    el.style.top = Math.max(pad, Math.min(top, window.innerHeight - menuH - pad)) + "px";
    el.classList.remove("hidden");
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
    const path = ctxRow.filePath || ctxRow.subtitle || "";
    const pinned = isItemPinned(path) || isItemPinned(ctxRow.icon);

    if (action === "open") {
      hideContextMenu();
      post("submit", selected);
    } else if (action === "pin") {
      hideContextMenu();
      if (pinned) {
        post("unpin", title);
      } else {
        post("pin", title);
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

  // ── Rust → JS bridge ─────────────────────────────────────
  window.nex = {
    apply(state) {
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

      // Close the power dropup / confirm whenever Rust pushes a fresh state
      // (show, hide, query change, etc.)
      footerPower.closeMenu();
      footerPower.closeConfirm();
      hideContextMenu();

      // Lightweight selection-only update (no rows = incremental).
      if (!Array.isArray(state.rows) && typeof state.selected === "number") {
        setSelected(state.selected, true);
        return;
      }

      if (state.theme) document.documentElement.dataset.theme = state.theme;

      // Toggle grid/list layout based on Rust config
      if (typeof state.gridView === "boolean") {
        list.classList.toggle("grid-view", state.gridView);
      }

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

      // Store Quick Launch items if provided
      if (Array.isArray(state.quickLaunch)) {
        quickLaunchItems = state.quickLaunch;
      }

      if (state.placeholder) {
        input.placeholder = state.placeholder;
      } else {
        input.placeholder = "Search for apps, files and actions…";
      }

      statusEl.dataset.text = state.status || "";

      // Signal that the next render should fire post("painted")
      // so the Rust side can show + focus the window. Only set on
      // show (when Rust sends showPending=true in the state JSON).
      // Also reset scroll position — otherwise scrollTop survives
      // across hide/show and new queries start at old scroll depth.
      const isShow = state.showPending;
      if (isShow) {
        pendingShow = true;
        needsPainted = true;
        lastH = 0; // fresh show cycle: trigger resize on first content paint
        scrollToInstant(0);
      }
      render();

      // On fresh show, the Show push has empty rows (hide cleared them).
      // Real results arrive on a later Apply push with showPending=false.
      // The pendingShow flag bridges this gap — consumed here when the
      // first non-empty rows arrive after a show cycle.
      if (pendingShow && rows.length > 0) {
        pendingShow = false;
        scrollToInstant(0);
        requestAnimationFrame(() => { scrollToInstant(0); });
        // Scroll to top — selected item starts at index 0, already in view.
      }
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
})();
