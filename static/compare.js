(function () {
  const progress = document.getElementById("compare-progress");
  const fill = document.getElementById("compare-progress-fill");
  const text = document.getElementById("compare-progress-text");
  const result = document.getElementById("compare-result");

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
