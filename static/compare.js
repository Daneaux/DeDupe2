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
      if (window.__initSimilarTables) window.__initSimilarTables();
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
      const controller = window.__compareControllers
        ? window.__compareControllers[form.dataset.table]
        : null;
      const files = controller ? controller.matchingPaths() : [];
      if (files.length === 0) {
        e.preventDefault();
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

// --- Compare results: full-set filtering ------------------------------------
// The compare page embeds the FULL result sets as JSON so this filter searches
// everything (including rows that don't fit on screen).
(function () {
  const RENDER_CAP = 500;
  const payload = (id) => {
    const el = document.getElementById(id);
    if (!el) return null;
    try {
      return JSON.parse(el.textContent || "[]");
    } catch (e) {
      return null;
    }
  };
  const aData = payload("compare-a-data");
  const bData = payload("compare-b-data");
  const dupData = payload("compare-dup-data");
  const unreadData = payload("compare-unreadable-data");
  if (!aData && !bData && !dupData && !unreadData) return;
  const filterEl = document.getElementById("compare-filter");
  const countEl = document.getElementById("compare-filter-count");

  function pathRow(cls, path) {
    const tr = document.createElement("tr");
    tr.className = cls;
    const td = document.createElement("td");
    td.className = "path clickable";
    td.dataset.reveal = path;
    td.textContent = path;
    tr.appendChild(td);
    return tr;
  }

  function dupRow(entry) {
    const tr = document.createElement("tr");
    tr.className = entry.tree === "A" ? "row-a" : "row-b";
    const badge = document.createElement("td");
    const span = document.createElement("span");
    span.className = "badge " + (entry.tree === "A" ? "badge-ok" : "badge-dirty");
    span.textContent = entry.tree;
    badge.appendChild(span);
    tr.appendChild(badge);
    const td = document.createElement("td");
    td.className = "path clickable";
    td.dataset.reveal = entry.path;
    td.textContent = entry.path;
    tr.appendChild(td);
    return tr;
  }

  function fill(tbodyId, rows, build) {
    const tbody = document.getElementById(tbodyId);
    if (!tbody || !rows) return 0;
    const q = (filterEl && filterEl.value ? filterEl.value : "").trim().toLowerCase();
    const matching = rows.filter((r) => {
      const p = typeof r === "string" ? r : r.path;
      return !q || String(p).toLowerCase().includes(q);
    });
    tbody.innerHTML = "";
    for (const r of matching.slice(0, RENDER_CAP)) {
      tbody.appendChild(build(r));
    }
    if (matching.length > RENDER_CAP) {
      const tr = document.createElement("tr");
      tr.className = "more-row";
      const td = document.createElement("td");
      td.className = "muted";
      td.textContent = "… " + (matching.length - RENDER_CAP) + " more match — refine the filter";
      tr.appendChild(td);
      tbody.appendChild(tr);
    }
    if (matching.length === 0) {
      const tr = document.createElement("tr");
      const td = document.createElement("td");
      td.className = "muted";
      td.textContent = q ? "No rows match the filter." : "None.";
      tr.appendChild(td);
      tbody.appendChild(tr);
    }
    return matching.length;
  }

  function render() {
    const aCount = fill("compare-table-a", aData, (p) => pathRow("row-a", p));
    const dupCount = fill("compare-table-dups", dupData, dupRow);
    const bCount = fill("compare-table-b", bData, (p) => pathRow("row-b", p));
    const unCount = fill("compare-table-unreadable", unreadData, (p) => pathRow("row-b", p));
    if (countEl) {
      const q = (filterEl && filterEl.value ? filterEl.value : "").trim();
      countEl.textContent =
        (q ? 'matching "' + q + '": ' : "totals: ") +
        aCount + " in A · " + dupCount + " duplicate row(s) · " +
        bCount + " only in B · " + unCount + " unreadable";
    }
  }

  if (filterEl) filterEl.addEventListener("input", render);
  render();
})();

// --- Compare results: per-table filters with "more…" -----------------------
// The compare page embeds its FULL result sets as JSON; each table gets its
// own filter box and pages in more rows on demand.
let initCompareTables = function () {};
(function () {
  const PAGE = 200;

  const payload = (id) => {
    const el = document.getElementById(id);
    if (!el) return null;
    try {
      return JSON.parse(el.textContent || "[]");
    } catch (e) {
      return null;
    }
  };

  function pathRow(cls, path) {
    const tr = document.createElement("tr");
    tr.className = cls;
    const td = document.createElement("td");
    td.className = "path clickable";
    td.dataset.reveal = path;
    td.textContent = path;
    tr.appendChild(td);
    return tr;
  }

  function dupRow(entry) {
    const tr = document.createElement("tr");
    tr.className = entry.tree === "A" ? "row-a" : "row-b";
    const badgeTd = document.createElement("td");
    const span = document.createElement("span");
    span.className = "badge " + (entry.tree === "A" ? "badge-ok" : "badge-dirty");
    span.textContent = entry.tree;
    badgeTd.appendChild(span);
    tr.appendChild(badgeTd);
    const td = document.createElement("td");
    td.className = "path clickable";
    td.dataset.reveal = entry.path;
    td.textContent = entry.path;
    tr.appendChild(td);
    return tr;
  }

  const controllers = {};

  function cell(text, className) {
    const td = document.createElement("td");
    if (className) td.className = className;
    td.textContent = text;
    return td;
  }

  function revealCell(text) {
    const td = document.createElement("td");
    td.className = "path clickable";
    td.dataset.reveal = text;
    td.textContent = text;
    return td;
  }

  function badgeCell(text, removable) {
    const td = document.createElement("td");
    const span = document.createElement("span");
    span.className = "badge " + (removable ? "badge-dirty" : "badge-ok");
    span.textContent = text;
    td.appendChild(span);
    return td;
  }

  function controller(cfg) {
    const tbody = document.getElementById(cfg.tbody);
    const filter = document.getElementById(cfg.filter);
    const count = document.getElementById(cfg.count);
    const more = document.getElementById(cfg.more);
    if (!tbody || !cfg.rows) return;
    let shown = 0;
    let matching = [];

    function matchingRows() {
      const q = (filter && filter.value ? filter.value : "").trim().toLowerCase();
      return cfg.rows.filter((r) => {
        const p = typeof r === "string" ? r : r.path;
        return !q || String(p).toLowerCase().includes(q);
      });
    }

    function updateCount() {
      if (!count) return;
      const q = (filter && filter.value ? filter.value : "").trim();
      count.textContent =
        matching.length === 0
          ? q
            ? "no matches"
            : "none"
          : "showing " + Math.min(shown, matching.length) + " of " + matching.length +
            (q ? " match" : "") + " (" + cfg.rows.length + " total)";
    }

    function appendPage() {
      const next = matching.slice(shown, shown + PAGE);
      for (const r of next) tbody.appendChild(cfg.build(r));
      shown += next.length;
      if (more) more.hidden = shown >= matching.length;
      updateCount();
    }

    function reset() {
      matching = matchingRows();
      shown = 0;
      tbody.innerHTML = "";
      if (matching.length === 0) {
        const tr = document.createElement("tr");
        const td = document.createElement("td");
        td.className = "muted";
        td.textContent = (filter && filter.value ? "No rows match the filter." : "None.");
        tr.appendChild(td);
        tbody.appendChild(tr);
        if (more) more.hidden = true;
        updateCount();
        return;
      }
      appendPage();
    }

    if (filter) filter.addEventListener("input", reset);
    if (more) more.addEventListener("click", appendPage);
    reset();

    controllers[cfg.name] = {
      matchingPaths: () => matching.map((r) => (typeof r === "string" ? r : r.path)),
    };
    window.__compareControllers = controllers;
  }

  initCompareTables = function () {
    const a = payload("compare-a-data");
    const b = payload("compare-b-data");
    const dup = payload("compare-dup-data");
    const unread = payload("compare-unreadable-data");
    if (!a && !b && !dup && !unread) return;
    controller({
      name: "a",
      tbody: "compare-table-a",
      filter: "compare-filter-a",
      count: "compare-count-a",
      more: "compare-more-a",
      rows: a || [],
      build: (p) => pathRow("row-a", p),
    });
    controller({
      name: "dups",
      tbody: "compare-table-dups",
      filter: "compare-filter-dups",
      count: "compare-count-dups",
      more: "compare-more-dups",
      rows: dup || [],
      build: dupRow,
    });
    controller({
      name: "b",
      tbody: "compare-table-b",
      filter: "compare-filter-b",
      count: "compare-count-b",
      more: "compare-more-b",
      rows: b || [],
      build: (p) => pathRow("row-b", p),
    });
    controller({
      name: "unreadable",
      tbody: "compare-table-unreadable",
      filter: "compare-filter-unreadable",
      count: "compare-count-unreadable",
      more: "compare-more-unreadable",
      rows: unread || [],
      build: (p) => pathRow("row-b", p),
    });
  };

  // --- Similar tab tables -------------------------------------------------
  function similarGroupRow(r) {
    const tr = document.createElement("tr");
    tr.appendChild(revealCell(r.keeper));
    tr.appendChild(revealCell(r.candidate));
    tr.appendChild(badgeCell("remove", true));
    tr.appendChild(cell(r.distance));
    tr.appendChild(cell(r.keeper_pixels));
    tr.appendChild(cell(r.candidate_pixels));
    tr.appendChild(cell(r.keeper_bytes));
    tr.appendChild(cell(r.candidate_bytes));
    return tr;
  }

  function similarMatchRow(r) {
    const tr = document.createElement("tr");
    tr.appendChild(revealCell(r.a));
    tr.appendChild(revealCell(r.b));
    tr.appendChild(badgeCell(r.verdict, r.removable));
    tr.appendChild(cell(r.distance));
    tr.appendChild(cell(r.a_pixels));
    tr.appendChild(cell(r.b_pixels));
    tr.appendChild(cell(r.a_bytes));
    tr.appendChild(cell(r.b_bytes));
    return tr;
  }

  let initSimilarTables = function () {};
  function similarDupRow(r) {
    const tr = document.createElement("tr");
    tr.appendChild(revealCell(r.keeper));
    tr.appendChild(revealCell(r.path));
    return tr;
  }

  initSimilarTables = function () {
    const groups = payload("similar-rows-data");
    const matches = payload("similar-matches-data");
    const dups = payload("similar-dups-data");
    if (!groups && !matches && !dups) return;
    if (dups) {
      controller({
        name: "similar-dups",
        tbody: "similar-table-dups",
        filter: "similar-filter-dups",
        count: "similar-count-dups",
        more: "similar-more-dups",
        rows: dups,
        build: similarDupRow,
      });
    }
    if (groups) {
      controller({
        name: "similar-rows",
        tbody: "similar-table-rows",
        filter: "similar-filter-rows",
        count: "similar-count-rows",
        more: "similar-more-rows",
        rows: groups,
        build: similarGroupRow,
      });
    }
    if (matches) {
      controller({
        name: "similar-matches",
        tbody: "similar-table-matches",
        filter: "similar-filter-matches",
        count: "similar-count-matches",
        more: "similar-more-matches",
        rows: matches,
        build: similarMatchRow,
      });
    }
  };

  window.__initSimilarTables = initSimilarTables;

  // Direct load (results already in the document).
  initCompareTables();
})();
