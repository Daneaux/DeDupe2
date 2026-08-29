const tabs = document.querySelectorAll(".tab");
const panels = document.querySelectorAll(".panel");

tabs.forEach((tab) => {
  tab.addEventListener("click", () => {
    tabs.forEach((t) => {
      t.classList.remove("active");
      t.setAttribute("aria-selected", "false");
    });
    tab.classList.add("active");
    tab.setAttribute("aria-selected", "true");

    const target = tab.dataset.target;
    panels.forEach((p) => {
      p.hidden = p.id !== target;
    });
  });
});

document.querySelectorAll(".segmented").forEach((group) => {
  group.addEventListener("click", (e) => {
    const btn = e.target.closest(".seg");
    if (!btn) return;
    group.querySelectorAll(".seg").forEach((s) => s.classList.remove("active"));
    btn.classList.add("active");
  });
});

document.querySelectorAll(".btn-scan").forEach((btn) => {
  btn.addEventListener("click", () => {
    const card = btn.closest(".volume-card");
    btn.disabled = true;
    btn.textContent = "Scanning…";
    setTimeout(() => {
      btn.disabled = false;
      btn.textContent = "Re-scan";
      card.classList.remove("dirty");
      const badges = card.querySelectorAll(".badge");
      badges.forEach((b) => b.remove());
      const title = card.querySelector(".volume-title");
      const ok = document.createElement("span");
      ok.className = "badge badge-ok";
      ok.textContent = "Scanned";
      title.appendChild(ok);
    }, 1200);
  });
});

document.querySelectorAll(".btn-delete").forEach((btn) => {
  btn.addEventListener("click", () => {
    const card = btn.closest(".volume-card");
    if (confirm("Remove this volume?")) card.remove();
  });
});

const filetypeInput = document.getElementById("filetype-input");
const filetypeList = document.getElementById("filetypes-list");

function addFiletype(value) {
  const li = document.createElement("li");
  li.className = "chip";
  li.textContent = value;
  const remove = document.createElement("button");
  remove.className = "chip-remove";
  remove.title = "Remove " + value;
  remove.textContent = "\u00d7";
  remove.addEventListener("click", () => li.remove());
  li.appendChild(remove);
  filetypeList.appendChild(li);
}

document.getElementById("btn-add-filetype").addEventListener("click", () => {
  const value = filetypeInput.value.trim();
  if (!value) return;
  addFiletype(value);
  filetypeInput.value = "";
});

filetypeInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    const value = filetypeInput.value.trim();
    if (!value) return;
    addFiletype(value);
    filetypeInput.value = "";
  }
});

const addVolumeModal = document.getElementById("add-volume-modal");
const addVolumePath = document.getElementById("add-volume-path");

function openModal() {
  addVolumeModal.hidden = false;
  addVolumePath.focus();
}

function closeModal() {
  addVolumeModal.hidden = true;
  addVolumePath.value = "";
}

document.getElementById("btn-add-volume").addEventListener("click", openModal);

addVolumeModal.querySelectorAll("[data-close]").forEach((el) => {
  el.addEventListener("click", closeModal);
});

document.getElementById("btn-confirm-add-volume").addEventListener("click", () => {
  const path = addVolumePath.value.trim();
  if (!path) return;
  const card = document.createElement("article");
  card.className = "volume-card";
  card.innerHTML =
    '<div class="volume-main">' +
    '<input type="checkbox" class="volume-select" />' +
    '<div class="drive-badge">' + path.charAt(0) + '</div>' +
    '<div class="volume-info">' +
    '<div class="volume-title"><h3>' + path + '</h3>' +
    '<span class="badge badge-muted">Not scanned</span></div>' +
    '<div class="volume-path">' + path + '</div>' +
    '<div class="capacity"><div class="capacity-bar"><div class="capacity-fill" style="width: 0%"></div></div>' +
    '<span class="capacity-text">0 GB of 0 GB</span></div>' +
    '</div></div>' +
    '<div class="volume-controls">' +
    '<div class="segmented"><button class="seg active" data-depth="shallow">Shallow</button>' +
    '<button class="seg" data-depth="deep">Deep</button></div>' +
    '<div class="volume-actions">' +
    '<button class="btn btn-small btn-primary">Scan</button>' +
    '<button class="btn btn-small btn-danger">Delete</button>' +
    '</div></div>';
  document.querySelector(".volume-list").appendChild(card);
  closeModal();
});

document.querySelectorAll(".btn-scan-subtree").forEach((btn) => {
  btn.addEventListener("click", () => {
    const select = btn.closest(".subtree-scan").querySelector(".subtree-select");
    const sub = select.value;
    if (!sub) {
      alert("Select a sub-directory first.");
      return;
    }
    alert("Scan subtree " + sub + " (not implemented)");
  });
});

document.getElementById("btn-scan-all").addEventListener("click", () => {
  alert("Scan all (not implemented)");
});
