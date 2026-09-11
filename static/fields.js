// Field memory for all tabs: values persist across page switches, and
// ArrowUp/ArrowDown recall previously submitted entries (oldest first).
(function () {
  const VALUES_PREFIX = "dd:val:";
  const HISTORY_PREFIX = "dd:hist:";
  const MAX_HISTORY = 50;

  function formKey(form) {
    return form.id || form.getAttribute("action") || "form";
  }

  function fieldKey(form, el) {
    return formKey(form) + ":" + (el.name || el.id || "?");
  }

  function isTrackable(el) {
    return (
      (el.tagName === "INPUT" && el.type === "text") ||
      el.tagName === "TEXTAREA" ||
      el.tagName === "SELECT"
    );
  }

  function fieldsOf(form) {
    return Array.from(form.elements).filter(isTrackable);
  }

  function restoreForm(form) {
    fieldsOf(form).forEach((el) => {
      const saved = localStorage.getItem(VALUES_PREFIX + fieldKey(form, el));
      if (saved === null) return;
      if (el.tagName === "SELECT") {
        if (Array.from(el.options).some((o) => o.value === saved)) el.value = saved;
      } else if (el.value !== saved) {
        el.value = saved;
      }
    });
  }

  function restoreAll() {
    document.querySelectorAll("form").forEach(restoreForm);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", restoreAll);
  } else {
    restoreAll();
  }

  // Persist values as they are typed (delegated: works on all pages).
  document.addEventListener("input", function (e) {
    const el = e.target;
    if (!el.form || !isTrackable(el)) return;
    try {
      localStorage.setItem(VALUES_PREFIX + fieldKey(el.form, el), el.value);
    } catch (err) {
      /* storage full — values are a convenience, ignore */
    }
  });

  // Record submitted values into per-field history (capture phase).
  document.addEventListener(
    "submit",
    function (e) {
      fieldsOf(e.target).forEach(function (el) {
        if (el.tagName === "SELECT") return;
        const v = el.value.trim();
        if (!v) return;
        const key = HISTORY_PREFIX + fieldKey(e.target, el);
        let hist = getHistory(key);
        hist = hist.filter((h) => h !== v);
        hist.push(v);
        while (hist.length > MAX_HISTORY) hist.shift();
        try {
          localStorage.setItem(key, JSON.stringify(hist));
        } catch (err) {
          /* ignore */
        }
      });
    },
    true
  );

  function getHistory(key) {
    try {
      return JSON.parse(localStorage.getItem(key) || "[]");
    } catch (err) {
      return [];
    }
  }

  // ArrowUp/ArrowDown: cycle through previous entries. Browsing starts when
  // the field is empty (or holds an exact history value); ArrowDown past the
  // newest entry restores the draft that was being typed.
  document.addEventListener("keydown", function (e) {
    const el = e.target;
    if (!isTrackable(el) || el.tagName === "SELECT") return;
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    if (!el.form) return;

    const hist = getHistory(HISTORY_PREFIX + fieldKey(el.form, el));
    if (hist.length === 0) return;

    if (el._histIdx === undefined) {
      const browsing = el.value === "" || hist.includes(el.value);
      if (!browsing) return;
      el._draft = el.value;
      el._histIdx = hist.length; // one past the newest
    }

    if (e.key === "ArrowUp") {
      if (el._histIdx <= 0) {
        e.preventDefault();
        return;
      }
      el._histIdx -= 1;
      el.value = hist[el._histIdx];
    } else {
      if (el._histIdx >= hist.length) {
        el.value = el._draft || "";
        delete el._histIdx;
        e.preventDefault();
        return;
      }
      el._histIdx += 1;
      el.value = el._histIdx === hist.length ? el._draft || "" : hist[el._histIdx];
    }
    e.preventDefault();
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
})();
