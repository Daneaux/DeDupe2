(function () {
  const progress = document.getElementById("compare-progress") || document.getElementById("sets-progress");
  const fill = document.getElementById("compare-progress-fill") || document.getElementById("sets-progress-fill");
  const text = document.getElementById("compare-progress-text") || document.getElementById("sets-progress-text");
  const result = document.getElementById("compare-result") || document.getElementById("sets-result");

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
