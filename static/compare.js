(function () {
  const pick = (suffix) =>
    document.getElementById("compare-" + suffix) ||
    document.getElementById("sets-" + suffix) ||
    document.getElementById("verify-" + suffix) ||
    document.getElementById("organize-" + suffix) ||
    document.getElementById("similar-" + suffix);
  const progress = pick("progress");
  const fill = pick("progress-fill");
  const text = pick("progress-text");
  const result = pick("result");

  let phaseLabel = "Working";

  // Compact "…/parent/leaf" form of a path for progress labels.
  function shortPath(value) {
    if (!value) return "";
    const clean = String(value).trim().replace(/\/+$/, "");
    const parts = clean.split("/").filter(Boolean);
    if (parts.length === 0) return "";
    return "…/" + parts.slice(-2).join("/");
  }

  function fieldValue(form, name) {
    const el = form.querySelector('[name="' + name + '"]');
    return el ? el.value : "";
  }

  function handleEvent(raw) {
    let event = "message";
    const data = [];
    for (const line of raw.split("\n")) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      else if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
    }
    const body = data.join("\n");

    if (event === "progress") {
      const parts = body.split(" ");
      const done = Number(parts[0]);
      const total = Number(parts[1]);
      const pct = total > 0 ? Math.round((done / total) * 100) : 0;
      fill.style.width = pct + "%";
      text.textContent = phaseLabel + " " + done + " / " + total;
    } else if (event === "done") {
      progress.hidden = true;
      result.innerHTML = body;
      // Result pages that embed JSON payloads render client-side.
      if (document.getElementById("verify-mismatches-data")) renderVerify();
      if (document.getElementById("compare-a-data")) initCompareTables();
      if (document.getElementById("similar-rows-data") || document.getElementById("similar-matches-data")) {
        initSimilarTables();
      }
      // The scan just fed the cache — update the path-field hints.
      if (window.refreshCacheHints) window.refreshCacheHints();
    } else if (event === "error") {
      text.textContent = "Error: " + body;
      text.classList.add("error");
      fill.style.width = "100%";
      if (window.refreshCacheHints) window.refreshCacheHints();
    }
  }

  document.addEventListener("change", function (e) {
    if (e.target && e.target.id === "copy-op-select") {
      const label = document.getElementById("transfer-label");
      if (label) label.textContent = e.target.value === "move" ? "Move" : "Copy";
    }
  });

  document.addEventListener("submit", function (e) {
    const form = e.target;
    let url = null;
    if (form.matches("form[data-table]")) {
      const table =
        window.__tables && window.__tables["compare-table-" + form.dataset.table];
      const files = table
        ? table.getRows("active").map((row) => row.getData().path)
        : [];
      if (files.length === 0) {
        e.preventDefault();
        text.textContent = "Error: no rows match this table's filter.";
        text.classList.add("error");
        progress.hidden = false;
        return;
      }
      const hidden = form.querySelector('input[name="files"]');
      if (hidden) hidden.value = files.join("\n");
      url = "/compare/transfer";
      const op = form.querySelector('select[name="op"]');
      phaseLabel =
        (op && op.value === "move" ? "Moving " : "Copying ") +
        files.length + " filtered file(s) to " +
        shortPath(form.querySelector('input[name="destination"]').value);
    } else if (form.id === "compare-form") {
      url = "/compare/run";
      phaseLabel = "Scanning " + shortPath(fieldValue(form, "a")) +
        " vs " + shortPath(fieldValue(form, "b"));
    } else if (form.id === "compare-exif-form") {
      url = "/compare/exif";
      phaseLabel = "Reading EXIF";
    } else if (form.id === "compare-copy-form") {
      url = "/compare/copy";
      phaseLabel = "Copying into " + shortPath(fieldValue(form, "destination"));
    } else if (form.id === "sets-form") {
      url = "/sets/run";
      const candidates = fieldValue(form, "candidates")
        .split("\n")
        .filter((line) => line.trim() !== "").length;
      phaseLabel = "Scanning " + shortPath(fieldValue(form, "a")) +
        " vs " + candidates + " candidate tree(s)";
    } else if (form.id === "compare-deep-form") {
      url = "/compare/deep";
      const pairs = fieldValue(form, "pairs")
        .split("\n")
        .filter((line) => line.trim() !== "").length;
      phaseLabel = "Deep scan (" + pairs + " pair(s))";
    } else if (form.id === "purgatory-form") {
      url = "/compare/purgatory";
      phaseLabel = "Moving from " + shortPath(fieldValue(form, "b_root"));
    } else if (form.id === "purgatory-form-dups") {
      url = "/compare/purgatory";
      phaseLabel = "Moving exact copies from " + shortPath(fieldValue(form, "b_root"));
    } else if (form.id === "organize-form") {
      url = "/organize/scan";
      phaseLabel = "Scanning " + shortPath(fieldValue(form, "source"));
    } else if (form.id === "organize-move-form") {
      url = "/organize/run";
      phaseLabel = "Organizing into " + shortPath(fieldValue(form, "destination"));
    } else if (form.id === "similar-form") {
      url = "/similar/run";
      const against = fieldValue(form, "compare");
      phaseLabel = "Finding similar in " + shortPath(fieldValue(form, "root")) +
        (against.trim() ? " vs " + shortPath(against) : "");
    } else if (form.id === "similar-decide-form") {
      const table = window.__tables && window.__tables["similar-table-matches"];
      const rows = table ? table.getSelectedRows() : [];
      const visible = table ? table.getRows("active").length : 0;
      if (rows.length === 0) {
        e.preventDefault();
        text.textContent =
          visible === 0
            ? "Error: no rows match the current filter."
            : "Error: tick at least one row (Use column) first.";
        text.classList.add("error");
        progress.hidden = false;
        return;
      }
      const lines = rows.map((row) => {
        const data = row.getData();
        return data.a + "\t" + data.b + "\t" + (data.action || "take_bigger");
      });
      const hidden = form.querySelector('input[name="decisions"]');
      if (hidden) hidden.value = lines.join("\n");
      url = "/similar/decide";
      phaseLabel = "Applying " + rows.length + " decision(s)";
    } else if (form.id === "similar-swap-form") {
      url = "/similar/swap";
      phaseLabel = "Swapping better copies into " + shortPath(fieldValue(form, "a_root"));
    } else if (form.id === "verify-form") {
      url = "/verify/run";
      phaseLabel = "Verifying " + shortPath(fieldValue(form, "root"));
    } else if (form.id === "verify-rehome-form") {
      url = "/verify/rehome";
      phaseLabel = "Re-homing " + shortPath(fieldValue(form, "root"));
    }
    if (!url) return;
    e.preventDefault();

    progress.hidden = false;
    fill.style.width = "0%";
    text.textContent = "Working…";
    text.classList.remove("error");
    result.innerHTML = "";

    const body = new URLSearchParams(new FormData(form)).toString();
    fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded", "Accept": "text/event-stream" },
      body: body,
    }).then(async (res) => {
      if (!res.ok) {
        text.textContent = "Error: " + (await res.text());
        text.classList.add("error");
        return;
      }
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      let terminal = false;
      const onEvent = function (raw) {
        if (/^event:\s*(done|error)\s*$/m.test(raw)) terminal = true;
        handleEvent(raw);
      };
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let idx;
        while ((idx = buffer.indexOf("\n\n")) !== -1) {
          const raw = buffer.slice(0, idx);
          buffer = buffer.slice(idx + 2);
          onEvent(raw);
        }
      }
      // A stream that ends without done/error means the worker died (panic)
      // or the connection dropped — never leave the UI on "Working…".
      if (!terminal) {
        text.textContent =
          "Error: the scan stopped before finishing — check the server console for a panic.";
        text.classList.add("error");
        fill.style.width = "100%";
      }
    });
  });
})();

// --- Reveal in folder -------------------------------------------------------
document.addEventListener("click", function (e) {
  const el = e.target.closest("[data-reveal]");
  if (!el) return;
  e.preventDefault();
  fetch("/reveal?path=" + encodeURIComponent(el.dataset.reveal)).catch(function () {});
});

// --- Destination column live recompute (exif view) -------------------------
function padNum(n, w) {
  return String(n).padStart(w, "0");
}

function computeDestination(sourcePath, dateStr, format, destRoot) {
  const digits = String(dateStr).split(/\D+/).filter(Boolean).map(Number);
  if (digits.length < 3) return "";
  const [y, m, d] = digits;

  const parts = String(sourcePath).split("/").filter(Boolean);
  const filename = parts.pop() || "";
  const folder = parts.pop() || "";
  const ymd = folder.match(/^\d{4}-\d{2}-\d{2}\s*(.*)$/);
  const mmdd = folder.match(/^\d{2}-\d{2}\s*(.*)$/);
  const desc = ymd ? (ymd[1] || "").trim() : mmdd ? mmdd[1].trim() : folder.trim();

  // Bind the date tokens FIRST (template only), then splice the description
  // so description text like "MM" can never be rewritten by a token.
  let out0 = format
    .replace(/YYYY/g, padNum(y, 4))
    .replace(/MM/g, padNum(m, 2))
    .replace(/DD/g, padNum(d, 2));

  let out;
  if (!desc) {
    out = out0.replace(/<folder description>/g, "").replace(/<desc>/g, "").replace(/DESC/g, "");
    out = out.split("/").map((s) => s.replace(/^[-_\s]+|[-_\s]+$/g, "").trim()).join("/");
  } else {
    out = out0.replace(/<folder description>/g, desc).replace(/<desc>/g, desc).replace(/DESC/g, desc);
  }


  // If the destination root already ends with the year, don't double it.
  const rootSegs = String(destRoot).split("/").filter(Boolean);
  const outSegs = out.split("/").filter(Boolean);
  if (outSegs.length > 0 && rootSegs[rootSegs.length - 1] === outSegs[0]) {
    outSegs.shift();
  }

  return rootSegs.join("/") + "/" + outSegs.join("/") + "/" + filename;
}

function recomputeDestinations() {
  const dest = document.getElementById("exif-destination");
  const fmt = document.getElementById("exif-format");
  if (!dest || !fmt) return;
  document.querySelectorAll("tr[data-path][data-date]").forEach((tr) => {
    const cell = tr.querySelector(".dest-cell");
    if (!cell) return;
    cell.textContent = computeDestination(
      tr.dataset.path,
      tr.dataset.date,
      fmt.value,
      dest.value
    );
  });
}

document.addEventListener("input", function (e) {
  if (e.target.id === "exif-destination" || e.target.id === "exif-format") {
    recomputeDestinations();
  }
});

// --- Verify results: client-side render + days-off filter -------------------
const VERIFY_ROW_CAP = 300;

function verifyData(id) {
  const tag = document.getElementById(id);
  if (!tag) return [];
  try {
    return JSON.parse(tag.textContent);
  } catch (err) {
    return [];
  }
}

function buildRow(cells, cls) {
  const tr = document.createElement("tr");
  if (cls) tr.className = cls;
  for (const c of cells) {
    const td = document.createElement("td");
    if (c.cls) td.className = c.cls;
    td.textContent = c.text === undefined ? c : c.text;
    if (c.reveal) {
      td.classList.add("clickable");
      td.dataset.reveal = c.reveal;
    }
    tr.appendChild(td);
  }
  return tr;
}

function renderVerify() {
  const mismatches = verifyData("verify-mismatches-data");
  const files = verifyData("verify-files-data");
  const daysInput = document.getElementById("verify-days");
  const statusInput = document.getElementById("verify-status-filter");
  if (!daysInput) return;

  const threshold = Number(daysInput.value) || 0;

  // Mismatches filtered by days-off threshold (default 10).
  const movable = mismatches.filter((m) => (m.days || 0) >= threshold);
  const rowsBody = document.getElementById("verify-mismatch-rows");
  rowsBody.textContent = "";
  movable.slice(0, VERIFY_ROW_CAP).forEach((m) => {
    rowsBody.appendChild(buildRow([
      { text: m.path, cls: "path", reveal: m.path },
      { text: m.exif, cls: "muted" },
      { text: m.folder, cls: "muted" },
      { text: m.days, cls: "muted" },
    ], "row-b"));
  });
  if (movable.length > VERIFY_ROW_CAP) {
    rowsBody.appendChild(buildRow([
      { text: "… " + (movable.length - VERIFY_ROW_CAP) + " more not shown", cls: "muted" },
    ]));
  }
  if (mismatches.length === 0) {
    rowsBody.appendChild(buildRow([{ text: "None — every file is in its date-correct folder.", cls: "muted" }]));
  }

  // The re-home form operates on the filtered subset.
  const hidden = document.getElementById("rehome-mismatches");
  if (hidden) hidden.value = movable.map((m) => m.path + "\t" + m.exif).join("\n");
  const count = document.getElementById("rehome-count");
  if (count) count.textContent = movable.length;
  const note = document.getElementById("verify-days-note");
  if (note) {
    note.textContent = movable.length + " of " + mismatches.length + " mismatches shown (≥ " + threshold + " days off)";
  }
  const badge = document.getElementById("verify-mismatch-badge");
  if (badge) badge.textContent = mismatches.length;

  // All files, filtered by status.
  const statusFilter = statusInput ? statusInput.value : "all";
  const shown = statusFilter === "all" ? files : files.filter((f) => f.status === statusFilter);
  const filesBody = document.getElementById("verify-files-rows");
  filesBody.textContent = "";
  shown.slice(0, VERIFY_ROW_CAP).forEach((f) => {
    filesBody.appendChild(buildRow([
      { text: f.path, cls: "path", reveal: f.path },
      { text: f.status, cls: "muted" },
      { text: f.exif || "", cls: "muted" },
      { text: f.folder || "", cls: "muted" },
      { text: f.days === null || f.days === undefined ? "" : f.days, cls: "muted" },
    ], f.status === "mismatch" ? "row-b" : "row-a"));
  });
  if (shown.length > VERIFY_ROW_CAP) {
    filesBody.appendChild(buildRow([
      { text: "… " + (shown.length - VERIFY_ROW_CAP) + " more not shown (use the status filter)", cls: "muted" },
    ]));
  }
  if (shown.length === 0) {
    filesBody.appendChild(buildRow([{ text: "No files match this filter.", cls: "muted" }]));
  }
  const filesNote = document.getElementById("verify-files-note");
  if (filesNote) {
    filesNote.textContent = "Showing " + Math.min(shown.length, VERIFY_ROW_CAP) + " of " + shown.length + " filtered files (" + files.length + " total).";
  }
}

document.addEventListener("input", function (e) {
  if (e.target.id === "verify-days") renderVerify();
});
document.addEventListener("change", function (e) {
  if (e.target.id === "verify-status-filter" || e.target.id === "verify-days") renderVerify();
});

// --- Cached-tree hints ------------------------------------------------------
// Path fields with [data-cache-status] get a live hint saying what that tree
// already has cached, layer by layer (green = cached, red = not yet).
(function () {
  const spans = new WeakMap();
  const n = (v) => Number(v || 0).toLocaleString();

  function layer(name, value, total) {
    const cls = value > 0 ? "cache-on" : "cache-off";
    return name + ' <span class="' + cls + '">' + n(value) + "</span>";
  }

  function update(el) {
    let span = spans.get(el);
    if (!span) {
      span = document.createElement("span");
      span.className = "hint cache-hint";
      el.insertAdjacentElement("afterend", span);
      spans.set(el, span);
    }
    const path = el.value.trim();
    if (!path) {
      span.textContent = "";
      return;
    }
    fetch("/cache-status?path=" + encodeURIComponent(path))
      .then((r) => r.json())
      .then((j) => {
        if (j.cached_files > 0) {
          span.innerHTML =
            "cached: " + n(j.cached_files) + " file(s) · " +
            layer("shallow", j.shallow) + " · " +
            layer("deep", j.deep) + " · " +
            layer("phash", j.phash) + " · " +
            layer("exif", j.exif);
        } else {
          span.innerHTML = 'not cached <span class="cache-off">0</span>';
        }
      })
      .catch(function () {});
  }

  let timer = null;
  document.addEventListener("input", function (e) {
    const el = e.target && e.target.closest && e.target.closest("[data-cache-status]");
    if (!el) return;
    clearTimeout(timer);
    timer = setTimeout(() => update(el), 300);
  });

  function refreshAll() {
    document.querySelectorAll("[data-cache-status]").forEach((el) => {
      if (el.value.trim()) update(el);
    });
  }
  // Scans feed the cache, so refresh whenever one finishes (or fails partway).
  window.refreshCacheHints = refreshAll;
  refreshAll();
  // fields.js restores remembered values after this script runs.
  setTimeout(refreshAll, 600);
})();


// --- Result tables (Tabulator) ---------------------------------------------
// The compare/similar result tables embed their full data as JSON; Tabulator
// handles column resizing (visible, aligned handles), select-all checkboxes,
// pagination, and width persistence.
let initCompareTables = function () {};
let initSimilarTables = function () {};
(function () {
  const tables = (window.__tables = window.__tables || {});

  const payload = (id) => {
    const el = document.getElementById(id);
    if (!el) return null;
    try {
      return JSON.parse(el.textContent || "[]");
    } catch (e) {
      return null;
    }
  };

  function makeHost(id) {
    const tbody = document.getElementById(id);
    if (!tbody) return null;
    const legacy = tbody.closest("table");
    const div = document.createElement("div");
    div.id = id;
    div.className = "tabulator-host";
    legacy.replaceWith(div);
    return div;
  }

  function pathFormatter(cell) {
    const span = document.createElement("span");
    span.className = "path clickable";
    const value = cell.getValue() || "";
    span.dataset.reveal = value;
    span.title = value;
    span.textContent = value;
    return span;
  }

  function badgeFormatter(cell) {
    const span = document.createElement("span");
    const data = cell.getData();
    span.className = "badge " + (data.removable ? "badge-dirty" : "badge-ok");
    span.textContent = cell.getValue() || "";
    return span;
  }

  function previewFormatter(cell) {
    const data = cell.getData();
    const paths = "a" in data ? [data.a, data.b] : [data.keeper, data.candidate];
    // Tabulator re-runs formatters when it restores a persisted column
    // layout; drop any previous preview block so the cell never accumulates
    // duplicates (this caused 4 images per cell).
    const cellEl = cell.getElement();
    if (cellEl) {
      cellEl.querySelectorAll(".previews").forEach((node) => node.remove());
    }
    const div = document.createElement("div");
    div.className = "previews";
    for (const path of paths) {
      const img = document.createElement("img");
      img.className = "thumb";
      img.loading = "lazy";
      img.alt = "";
      img.title = path;
      img.src = "/thumb?path=" + encodeURIComponent(path);
      img.addEventListener("error", () => img.remove());
      div.appendChild(img);
    }
    return div;
  }

  function bytesFmt(n) {
    if (n === undefined || n === null) return "?";
    const units = ["B", "KB", "MB", "GB", "TB"];
    let v = Number(n);
    let u = 0;
    while (v >= 1024 && u < units.length - 1) {
      v /= 1024;
      u += 1;
    }
    return (u === 0 ? v : v.toFixed(1)) + " " + units[u];
  }

  function deltaText(copyBytes, keepBytes) {
    if (!keepBytes || copyBytes === undefined || copyBytes === null) return "?";
    const diff = copyBytes - keepBytes;
    const pct = Math.round((diff / keepBytes) * 100);
    const sign = diff > 0 ? "+" : "−";
    return sign + Math.abs(pct) + "% (" + sign + bytesFmt(Math.abs(diff)) + ")";
  }

  function deltaFormatter(cell) {
    const data = cell.getData();
    const copy = "b_bytes" in data ? data.b_bytes : data.candidate_bytes;
    const keep = "a_bytes" in data ? data.a_bytes : data.keeper_bytes;
    return deltaText(copy, keep);
  }

  function actionFormatter(cell) {
    const select = document.createElement("select");
    select.className = "row-action";
    for (const [value, label] of [
      ["take_bigger", "3 · take bigger (suggested)"],
      ["keep_both", "1 · keep both (move B into A)"],
      ["purgatory_both", "2 · keep neither (both → purgatory)"],
    ]) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      select.appendChild(option);
    }
    select.value = cell.getValue() || "take_bigger";
    select.addEventListener("change", () => {
      cell.getRow().update({ action: select.value });
    });
    return select;
  }

  function destinationFormatter(cell) {
    const data = cell.getData();
    const formatInput = document.getElementById("similar-decide-format");
    const rootInput = document.querySelector(
      '#similar-decide-form input[name="a_root"]'
    );
    const computed = computeDestination(
      data.b || "",
      data.date || "unknown",
      formatInput ? formatInput.value : "YYYY/MM-DD <folder description>",
      rootInput ? rootInput.value : ""
    );
    const span = document.createElement("span");
    span.className = computed ? "path" : "muted";
    span.textContent = computed || "no EXIF date";
    return span;
  }

  const PATH_FIELDS = ["path", "keeper", "candidate", "a", "b"];

  function makeTable(spec) {
    const host = makeHost(spec.id);
    if (!host) return null;
    const table = new Tabulator(host, {
      data: spec.rows,
      columns: spec.columns,
      layout: "fitDataFill",
      height: "65vh",
      resizableColumns: true,
      selectableRows: spec.selectable ? true : false,
      pagination: true,
      paginationSize: 200,
      paginationSizeSelector: [100, 200, 500, 1000],
      persistence: { columns: ["width"] },
      persistenceID: "dedupe2.tab." + spec.id,
      placeholder: "none",
    });
    tables[spec.id] = table;

    // Text filter + similarity slider (both optional).
    const filter = spec.filter ? document.getElementById(spec.filter) : null;
    const slider = spec.slider ? document.getElementById(spec.slider) : null;
    const count = spec.count ? document.getElementById(spec.count) : null;
    const apply = () => {
      const q = (filter && filter.value ? filter.value : "").trim().toLowerCase();
      const limit = slider ? Number(slider.value) : Infinity;
      table.setFilter((data) => {
        if (q) {
          const hit = PATH_FIELDS.some((field) =>
            String(data[field] || "").toLowerCase().includes(q)
          );
          if (!hit) return false;
        }
        if (typeof data.distance === "number" && data.distance > limit) return false;
        return true;
      });
    };
    if (filter) filter.addEventListener("input", apply);
    if (slider) {
      const label = document.getElementById(spec.slider + "-value");
      const sync = () => {
        if (label) label.textContent = slider.value;
      };
      slider.addEventListener("input", () => {
        sync();
        apply();
      });
      sync();
    }
    table.on("dataFiltered", (filters, rows) => {
      if (count) count.textContent = "showing " + rows.length + " row(s)";
    });

    // Frame: corner resize + fullscreen with Escape and an in-frame exit.
    // It replaces the legacy .table-scroll so there are no nested scrollers
    // (Tabulator scrolls internally).
    const frame = document.createElement("div");
    frame.className = "table-frame";
    const legacyScroller = host.closest(".table-scroll") || host;
    legacyScroller.replaceWith(frame);
    frame.appendChild(host);

    const corner = document.createElement("div");
    corner.className = "corner-resizer";
    corner.title = "Drag to resize the table area";
    frame.appendChild(corner);
    corner.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      const startX = e.clientX;
      const startY = e.clientY;
      const startWidth = frame.getBoundingClientRect().width;
      const startHeight = host.getBoundingClientRect().height;
      const onMove = (move) => {
        frame.style.width = Math.max(320, startWidth + (move.clientX - startX)) + "px";
        host.style.height = Math.max(160, startHeight + (move.clientY - startY)) + "px";
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        table.redraw();
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    });

    const tools = frame.previousElementSibling;
    const hostTools = tools && tools.classList.contains("table-tools") ? tools : null;
    if (hostTools && !hostTools.querySelector(".fullscreen-toggle")) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "btn btn-small btn-secondary fullscreen-toggle";
      button.textContent = "⛶ Full screen";
      const exit = document.createElement("button");
      exit.type = "button";
      exit.className = "btn btn-small btn-secondary fullscreen-exit";
      exit.textContent = "⤡ Exit full screen";
      exit.hidden = true;
      frame.appendChild(exit);
      const setFullscreen = (on) => {
        frame.classList.toggle("fullscreen", on);
        button.textContent = on ? "⤡ Exit full screen" : "⛶ Full screen";
        exit.hidden = !on;
        setTimeout(() => table.redraw(), 30);
      };
      button.addEventListener("click", () =>
        setFullscreen(!frame.classList.contains("fullscreen"))
      );
      exit.addEventListener("click", () => setFullscreen(false));
      document.addEventListener("keydown", (e) => {
        if (e.key === "Escape" && frame.classList.contains("fullscreen")) {
          setFullscreen(false);
        }
      });
      hostTools.appendChild(button);
    }
    return table;
  }

  initCompareTables = function () {
    const a = payload("compare-a-data");
    const b = payload("compare-b-data");
    const dup = payload("compare-dup-data");
    const unread = payload("compare-unreadable-data");
    if (!a && !b && !dup && !unread) return;
    const pathColumn = (title) => [
      { title, field: "path", minWidth: 260, formatter: pathFormatter },
    ];
    if (a) {
      makeTable({
        id: "compare-table-a",
        columns: pathColumn("Path"),
        rows: a.map((path) => ({ path })),
        filter: "compare-filter-a",
        count: "compare-count-a",
      });
    }
    if (dup) {
      makeTable({
        id: "compare-table-dups",
        columns: [
          { title: "Tree", field: "tree", width: 70, hozAlign: "center" },
          { title: "Path", field: "path", minWidth: 260, formatter: pathFormatter },
        ],
        rows: dup,
        filter: "compare-filter-dups",
        count: "compare-count-dups",
      });
    }
    if (b) {
      makeTable({
        id: "compare-table-b",
        columns: pathColumn("Path"),
        rows: b.map((path) => ({ path })),
        filter: "compare-filter-b",
        count: "compare-count-b",
      });
    }
    if (unread) {
      makeTable({
        id: "compare-table-unreadable",
        columns: pathColumn("Path"),
        rows: unread.map((path) => ({ path })),
        filter: "compare-filter-unreadable",
        count: "compare-count-unreadable",
      });
    }
  };

  initSimilarTables = function () {
    const dups = payload("similar-dups-data");
    const groups = payload("similar-rows-data");
    const matches = payload("similar-matches-data");
    if (!dups && !groups && !matches) return;
    if (dups) {
      makeTable({
        id: "similar-table-dups",
        columns: [
          { title: "Keep (biggest file)", field: "keeper", minWidth: 220, formatter: pathFormatter },
          { title: "Exact copy", field: "path", minWidth: 220, formatter: pathFormatter },
        ],
        rows: dups,
        filter: "similar-filter-dups",
        count: "similar-count-dups",
      });
    }
    if (groups) {
      makeTable({
        id: "similar-table-rows",
        columns: [
          { title: "Preview", field: "preview", width: 440, headerSort: false, formatter: previewFormatter },
          { title: "Keep (biggest file)", field: "keeper", minWidth: 220, formatter: pathFormatter },
          { title: "Copy", field: "candidate", minWidth: 220, formatter: pathFormatter },
          { title: "Verdict", field: "keeper", width: 90, headerSort: false, formatter: () => {
            const span = document.createElement("span");
            span.className = "badge badge-dirty";
            span.textContent = "remove";
            return span;
          } },
          { title: "Distance", field: "distance", width: 90, hozAlign: "right" },
          { title: "Keep px", field: "keeper_pixels", width: 90, hozAlign: "right" },
          { title: "Copy px", field: "candidate_pixels", width: 90, hozAlign: "right" },
          { title: "Keep size", field: "keeper_bytes", width: 100, hozAlign: "right", formatter: (c) => bytesFmt(c.getValue()) },
          { title: "Copy size", field: "candidate_bytes", width: 100, hozAlign: "right", formatter: (c) => bytesFmt(c.getValue()) },
          { title: "Size Δ", field: "candidate_bytes", width: 140, hozAlign: "right", formatter: deltaFormatter },
        ],
        rows: groups,
        filter: "similar-filter-rows",
        count: "similar-count-rows",
        slider: "similar-similarity-rows",
      });
    }
    if (matches) {
      makeTable({
        id: "similar-table-matches",
        selectable: true,
        columns: [
          { title: "", field: "use", width: 44, hozAlign: "center", headerSort: false, formatter: "rowSelection", titleFormatter: "rowSelection" },
          { title: "Preview", field: "preview", width: 440, headerSort: false, formatter: previewFormatter },
          { title: "A (kept when bigger)", field: "a", minWidth: 220, formatter: pathFormatter },
          { title: "B", field: "b", minWidth: 220, formatter: pathFormatter },
          { title: "Verdict", field: "verdict", minWidth: 180, formatter: badgeFormatter },
          { title: "Action", field: "action", width: 220, headerSort: false, formatter: actionFormatter },
          { title: "Destination (EXIF of B)", field: "destination", minWidth: 220, headerSort: false, formatter: destinationFormatter },
          { title: "Distance", field: "distance", width: 90, hozAlign: "right" },
          { title: "A px", field: "a_pixels", width: 90, hozAlign: "right" },
          { title: "B px", field: "b_pixels", width: 90, hozAlign: "right" },
          { title: "A size", field: "a_bytes", width: 100, hozAlign: "right", formatter: (c) => bytesFmt(c.getValue()) },
          { title: "B size", field: "b_bytes", width: 100, hozAlign: "right", formatter: (c) => bytesFmt(c.getValue()) },
          { title: "Size Δ", field: "b_bytes", width: 140, hozAlign: "right", formatter: deltaFormatter },
        ],
        rows: matches.map((row) => ({ ...row, action: "take_bigger" })),
        filter: "similar-filter-matches",
        count: "similar-count-matches",
        slider: "similar-similarity-matches",
      });
    }
  };

  // Live destination recompute when the decide format changes.
  document.addEventListener("input", function (e) {
    if (e.target && e.target.id === "similar-decide-format") {
      const table = tables["similar-table-matches"];
      if (table) table.getRows().forEach((row) => row.reformat());
    }
  });

  initCompareTables();
  initSimilarTables();
})();

window.__initSimilarTables = initSimilarTables;
