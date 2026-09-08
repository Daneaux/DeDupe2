(function () {
  const progress = document.getElementById("merge-progress");
  const fill = document.getElementById("merge-progress-fill");
  const text = document.getElementById("merge-progress-text");
  const result = document.getElementById("merge-result");

  function showProgress(label) {
    progress.hidden = false;
    fill.style.width = "0%";
    text.textContent = label;
    text.classList.remove("error");
  }

  function hideProgress() {
    progress.hidden = true;
  }

  // Parse a single SSE event block (raw text up to a blank line).
  function handleEvent(raw) {
    let event = "message";
    const data = [];
    for (const line of raw.split("\n")) {
      if (line.startsWith("event:")) {
        event = line.slice(6).trim();
      } else if (line.startsWith("data:")) {
        data.push(line.slice(5).trimStart());
      }
    }
    const body = data.join("\n");

    if (event === "progress") {
      const parts = body.split(" ");
      const done = Number(parts[0]);
      const total = Number(parts[1]);
      const pct = total > 0 ? Math.round((done / total) * 100) : 0;
      fill.style.width = pct + "%";
      text.textContent = "Scanning " + done + " / " + total;
    } else if (event === "done") {
      hideProgress();
      result.innerHTML = body;
    } else if (event === "error") {
      text.textContent = "Error: " + body;
      text.classList.add("error");
      fill.style.width = "100%";
    }
  }

  async function run(url, form) {
    showProgress("Working…");
    result.innerHTML = "";

    const body = new URLSearchParams(new FormData(form)).toString();
    const res = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded", "Accept": "text/event-stream" },
      body: body,
    });

    if (!res.ok) {
      const msg = await res.text();
      text.textContent = "Error: " + msg;
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
  }

  document.addEventListener("submit", function (e) {
    const form = e.target;
    if (form.id === "merge-form") {
      e.preventDefault();
      run("/merge/preview", form);
    } else if (form.id === "merge-run-form") {
      e.preventDefault();
      run("/merge/run", form);
    }
  });
})();
