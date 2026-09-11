(function () {
  const pick = (suffix) =>
    document.getElementById("compare-" + suffix) ||
    document.getElementById("sets-" + suffix) ||
    document.getElementById("verify-" + suffix);
  const progress = pick("progress");
  const fill = pick("progress-fill");
  const text = pick("progress-text");
  const result = pick("result");

  let phaseLabel = "Working";

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
      // Verify results render client-side from embedded JSON payloads.
      if (document.getElementById("verify-mismatches-data")) renderVerify();
    } else if (event === "error") {
      text.textContent = "Error: " + body;
      text.classList.add("error");
      fill.style.width = "100%";
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
    if (form.id === "compare-form") {
      url = "/compare/run";
      phaseLabel = "Scanning";
    } else if (form.id === "compare-exif-form") {
      url = "/compare/exif";
      phaseLabel = "Reading EXIF";
    } else if (form.id === "compare-copy-form") {
      url = "/compare/copy";
      phaseLabel = "Copying";
    } else if (form.id === "sets-form") {
      url = "/sets/run";
      phaseLabel = "Scanning";
    } else if (form.id === "compare-deep-form") {
      url = "/compare/deep";
      phaseLabel = "Deep scan";
    } else if (form.id === "verify-form") {
      url = "/verify/run";
      phaseLabel = "Verifying";
    } else if (form.id === "verify-rehome-form") {
      url = "/verify/rehome";
      phaseLabel = "Re-homing";
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
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let idx;
        while ((idx = buffer.indexOf("\n\n")) !== -1) {
          const raw = buffer.slice(0, idx);
          buffer = buffer.slice(idx + 2);
          handleEvent(raw);
        }
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

  let out = format;
  if (!desc) {
    out = out.replace(/<folder description>/g, "").replace(/<desc>/g, "").replace(/DESC/g, "");
    out = out.split("/").map((s) => s.replace(/^[-_\s]+|[-_\s]+$/g, "").trim()).join("/");
  } else {
    out = out.replace(/<folder description>/g, desc).replace(/<desc>/g, desc).replace(/DESC/g, desc);
  }
  out = out.replace(/YYYY/g, padNum(y, 4)).replace(/MM/g, padNum(m, 2)).replace(/DD/g, padNum(d, 2));

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

