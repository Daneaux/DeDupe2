/* Photo Tools – shared JS */

// ── Segmented button groups ────────────────────
document.querySelectorAll(".segmented").forEach(group => {
  group.addEventListener("click", e => {
    const btn = e.target.closest(".seg");
    if (!btn) return;
    group.querySelectorAll(".seg").forEach(b => b.classList.remove("active"));
    btn.classList.add("active");
  });
});

function getSegValue(container, dataAttr) {
  const active = container.querySelector(`.seg[${dataAttr}].active`);
  return active ? active.getAttribute(dataAttr) : null;
}

// ── Folder Dedupe tab ──────────────────────────
(function () {
  const btnFind = document.getElementById("btn-find-dupes");
  const btnMerge = document.getElementById("btn-merge");
  if (!btnFind) return;

  const stratGroup = document.querySelector('[aria-label="Duplicate strategy"]');
  const opGroup = document.querySelector('[aria-label="Operation"]');
  const resultsArea = document.getElementById("results-area");
  const summary = document.getElementById("results-summary");
  const groupsDiv = document.getElementById("dup-groups");
  const mergeResult = document.getElementById("merge-result");

  function config() {
    return {
      folder_a: document.getElementById("folder-a").value.trim(),
      folder_b: document.getElementById("folder-b").value.trim(),
      dst: document.getElementById("dst-folder").value.trim(),
      strategy: getSegValue(stratGroup, "data-strategy") || "exact",
      op: getSegValue(opGroup, "data-op") || "move",
    };
  }

  function renderGroups(data) {
    groupsDiv.innerHTML = "";
    if (!data.groups || data.groups.length === 0) {
      groupsDiv.innerHTML = '<p style="color:var(--text-muted)">No duplicates found. 🎉</p>';
      return;
    }
    data.groups.forEach((g, i) => {
      const div = document.createElement("div");
      div.className = "dup-group";
      const hdr = document.createElement("div");
      hdr.className = "group-header";
      hdr.innerHTML = `<span>Duplicate group ${i + 1}</span><span class="hash-label">hash: ${g.hash}</span>`;
      div.appendChild(hdr);
      g.files.forEach(f => {
        const row = document.createElement("div");
        row.className = "file-row";
        row.innerHTML = `
          <span class="file-path">${esc(f.path)}</span>
          <span class="file-size">${fmtSize(f.size)}</span>
        `;
        div.appendChild(row);
      });
      groupsDiv.appendChild(div);
    });
  }

  btnFind.addEventListener("click", async () => {
    const c = config();
    if (!c.folder_a || !c.folder_b) {
      alert("Please enter both folders.");
      return;
    }
    btnFind.disabled = true;
    btnFind.innerHTML = '<span class="spinner"></span>Scanning…';
    resultsArea.hidden = false;
    summary.textContent = "Scanning…";
    groupsDiv.innerHTML = "";
    mergeResult.hidden = true;
    btnMerge.disabled = true;

    try {
      const res = await fetch("/photo_tools/api/find_duplicates", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(c),
      });
      const data = await res.json();
      if (!res.ok) throw new Error(data.error || "Request failed");

      summary.textContent = `Found ${data.total_groups} duplicate group(s) – ${data.total_files} files`;
      renderGroups(data);
      btnMerge.disabled = data.total_groups === 0;
    } catch (err) {
      summary.textContent = "Error";
      groupsDiv.innerHTML = `<p style="color:var(--red)">${esc(err.message)}</p>`;
    } finally {
      btnFind.disabled = false;
      btnFind.textContent = "Find Duplicates";
    }
  });

  btnMerge.addEventListener("click", async () => {
    const c = config();
    if (!c.dst) {
      alert("Please enter a destination folder.");
      return;
    }
    if (!confirm(`Merge folders into ${c.dst}?\nThis will ${c.op === "move" ? "move" : "copy"} files and remove duplicates.`)) return;

    btnMerge.disabled = true;
    btnMerge.innerHTML = '<span class="spinner"></span>Merging…';
    mergeResult.hidden = false;
    mergeResult.className = "merge-result";
    mergeResult.textContent = "Merging…";

    try {
      const res = await fetch("/photo_tools/api/merge", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(c),
      });
      const data = await res.json();
      if (!res.ok) throw new Error(data.error || "Merge failed");

      mergeResult.className = "merge-result success";
      mergeResult.textContent = `✓ Merged successfully. ${data.moved_files} file(s) written, ${data.discarded} duplicate(s) discarded.`;
    } catch (err) {
      mergeResult.className = "merge-result error";
      mergeResult.textContent = `✗ ${err.message}`;
    } finally {
      btnMerge.disabled = false;
      btnMerge.textContent = "Merge Folders";
    }
  });
})();

// ── Find Original Images tab ───────────────────
(function () {
  const btnScan = document.getElementById("btn-scan-originals");
  if (!btnScan) return;

  const modeGroup = document.querySelector('[aria-label="Match mode"]');
  const resultsArea = document.getElementById("orig-results-area");
  const summary = document.getElementById("orig-summary");
  const groupsDiv = document.getElementById("orig-groups");

  btnScan.addEventListener("click", async () => {
    const folder = document.getElementById("scan-folder").value.trim();
    if (!folder) { alert("Please enter a folder to scan."); return; }

    const mode = getSegValue(modeGroup, "data-mode") || "exif";

    btnScan.disabled = true;
    btnScan.innerHTML = '<span class="spinner"></span>Scanning…';
    resultsArea.hidden = false;
    summary.textContent = "Scanning…";
    groupsDiv.innerHTML = "";

    try {
      const res = await fetch("/photo_tools/api/find_originals", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ folder, mode }),
      });
      const data = await res.json();
      if (!res.ok) throw new Error(data.error || "Scan failed");

      const origCount = data.groups.filter(g => g.original).length;
      summary.textContent = `Scanned ${data.total_files} file(s) – ${origCount} original(s) identified`;
      renderOrigGroups(data);
    } catch (err) {
      summary.textContent = "Error";
      groupsDiv.innerHTML = `<p style="color:var(--red)">${esc(err.message)}</p>`;
    } finally {
      btnScan.disabled = false;
      btnScan.textContent = "Scan for Originals";
    }
  });

  function renderOrigGroups(data) {
    groupsDiv.innerHTML = "";
    if (!data.groups || data.groups.length === 0) {
      groupsDiv.innerHTML = '<p style="color:var(--text-muted)">No images found.</p>';
      return;
    }
    data.groups.forEach((g, i) => {
      const div = document.createElement("div");
      div.className = "orig-group";
      const hdr = document.createElement("div");
      hdr.className = "group-header";
      const label = g.original ? g.original.name : g.files[0].name;
      hdr.innerHTML = `<span>Group ${i + 1} – ${esc(label)}</span><span class="file-date">${esc(g.date || "")}</span>`;
      div.appendChild(hdr);

      g.files.forEach(f => {
        const row = document.createElement("div");
        row.className = "file-row";
        let badge;
        if (g.original && f.path === g.original.path) {
          badge = '<span class="badge badge-orig">Original</span>';
        } else if (g.original) {
          badge = '<span class="badge badge-deriv">Derived</span>';
        } else {
          badge = '<span class="badge badge-alone">Alone</span>';
        }
        row.innerHTML = `
          ${badge}
          <span class="file-path">${esc(f.name)}</span>
          <span class="file-size">${fmtSize(f.size)}</span>
          <span class="file-date">${esc(f.date || "")}</span>
        `;
        div.appendChild(row);
      });
      groupsDiv.appendChild(div);
    });
  }
})();

// ── Helpers ────────────────────────────────────
function esc(s) {
  const d = document.createElement("div");
  d.textContent = s;
  return d.innerHTML;
}

function fmtSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1048576) return (bytes / 1024).toFixed(1) + " KB";
  if (bytes < 1073741824) return (bytes / 1048576).toFixed(1) + " MB";
  return (bytes / 1073741824).toFixed(2) + " GB";
}
