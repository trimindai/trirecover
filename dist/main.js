// TriRecover frontend (vanilla). Talks to the Rust backend through the Tauri
// global injected by `withGlobalTauri: true` in tauri.conf.json. No bundler.

(function () {
  const { invoke } = window.__TAURI__.core;
  const { open } = window.__TAURI__.dialog;
  const { listen } = window.__TAURI__.event;
  const { open: openExternal } = window.__TAURI__.shell;

  const state = {
    source: null,     // drive path or image file path
    sourceType: "drive", // "drive" or "image"
    destPath: null,
    files: [],
    selected: new Set(),
    drives: [],
  };

  // ---------- helpers ----------
  function $(id) { return document.getElementById(id); }
  function fmtBytes(n) {
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let v = n;
    let u = 0;
    while (v >= 1024 && u < units.length - 1) { v /= 1024; u++; }
    return v.toFixed(v >= 100 ? 0 : 2) + " " + units[u];
  }
  function fmtHex(n) {
    return "0x" + n.toString(16).padStart(12, "0");
  }
  function setStatus(msg, kind = "") {
    const el = $("scan-status");
    el.className = "status " + kind;
    el.textContent = msg;
  }
  function updateScanButton() {
    $("start-scan").disabled = !state.source;
  }

  // ---------- version ----------
  invoke("app_version").then((v) => {
    $("version").textContent = "v" + v;
  }).catch(() => {});

  // ---------- source tabs ----------
  $("tab-drive").addEventListener("click", () => {
    state.sourceType = "drive";
    $("tab-drive").classList.add("active");
    $("tab-image").classList.remove("active");
    $("panel-drive").hidden = false;
    $("panel-image").hidden = true;
    // restore drive selection
    const sel = $("drive-select");
    state.source = sel.value || null;
    updateScanButton();
  });

  $("tab-image").addEventListener("click", () => {
    state.sourceType = "image";
    $("tab-image").classList.add("active");
    $("tab-drive").classList.remove("active");
    $("panel-image").hidden = false;
    $("panel-drive").hidden = true;
    state.source = state._imagePath || null;
    updateScanButton();
  });

  // ---------- drive listing ----------
  async function loadDrives() {
    const sel = $("drive-select");
    sel.innerHTML = '<option value="">Scanning drives…</option>';
    $("drive-info").textContent = "";
    try {
      const drives = await invoke("list_drives");
      state.drives = drives;
      sel.innerHTML = "";
      if (drives.length === 0) {
        sel.innerHTML = '<option value="">No drives found</option>';
        state.source = null;
        updateScanButton();
        return;
      }
      sel.appendChild(new Option("— Select a drive —", ""));
      for (const d of drives) {
        const label = `${d.model || d.path} — ${fmtBytes(d.size_bytes)} (${d.kind}, ${d.bus})`;
        sel.appendChild(new Option(label, d.path));
      }
      state.source = null;
      updateScanButton();
    } catch (e) {
      sel.innerHTML = '<option value="">Error loading drives</option>';
      $("drive-info").textContent = "Error: " + e;
    }
  }

  $("drive-select").addEventListener("change", (e) => {
    const path = e.target.value;
    state.source = path || null;
    const d = state.drives.find((x) => x.path === path);
    if (d) {
      $("drive-info").textContent =
        `${d.path} · ${d.model} · Serial: ${d.serial || "N/A"} · Sector: ${d.sector_size}B`;
    } else {
      $("drive-info").textContent = "";
    }
    updateScanButton();
  });

  $("refresh-drives").addEventListener("click", loadDrives);
  loadDrives();

  // ---------- file picker (image mode) ----------
  $("pick-image").addEventListener("click", async () => {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [
        { name: "Disk images", extensions: ["img", "dd", "bin", "iso", "raw"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof path === "string") {
      state._imagePath = path;
      state.source = path;
      $("image-path").textContent = path;
      $("image-path").classList.remove("muted");
      updateScanButton();
    }
  });

  $("pick-dest").addEventListener("click", async () => {
    const path = await open({ directory: true });
    if (typeof path === "string") {
      state.destPath = path;
      $("dest-path").textContent = path;
      $("dest-path").classList.remove("muted");
      updateRecoverButton();
    }
  });

  // ---------- scan ----------
  let scanProgressUnlisten = null;
  let scanDoneUnlisten = null;
  async function setupListeners() {
    if (scanProgressUnlisten) scanProgressUnlisten();
    if (scanDoneUnlisten) scanDoneUnlisten();
    scanProgressUnlisten = await listen("scan/progress", (e) => {
      const p = e.payload;
      const pct = p.bytes_total > 0
        ? ((p.bytes_scanned / p.bytes_total) * 100).toFixed(1)
        : "0";
      setStatus(
        `Scanning… ${p.files_found} files found · ${fmtBytes(p.bytes_scanned)} / ${fmtBytes(p.bytes_total)} (${pct}%)`
      );
    });
    scanDoneUnlisten = await listen("scan/done", (e) => {
      const d = e.payload;
      setStatus(
        `Done. ${d.files_found} files · ${fmtBytes(d.bytes_recoverable)} recoverable · ${(d.elapsed_ms / 1000).toFixed(1)}s`,
        "ok"
      );
    });
  }
  setupListeners();

  $("start-scan").addEventListener("click", async () => {
    if (!state.source) return;
    $("start-scan").disabled = true;
    $("results-card").hidden = true;
    state.files = [];
    state.selected.clear();
    setStatus("Starting scan…");

    const kindsRaw = $("kinds").value.trim();
    const kinds = kindsRaw ? kindsRaw.split(",").map((s) => s.trim()).filter(Boolean) : [];
    const minSize = parseInt($("min-size").value || "0", 10) || 0;

    try {
      const files = await invoke("scan_image", {
        imagePath: state.source,
        kinds,
        minSize,
      });
      state.files = files || [];
      renderResults();
      $("results-card").hidden = state.files.length === 0;
      if (state.files.length === 0) setStatus("No files found.", "warn");
    } catch (e) {
      setStatus("Error: " + e, "err");
    } finally {
      $("start-scan").disabled = false;
    }
  });

  // ---------- results table ----------
  function renderResults() {
    const tbody = $("results-body");
    tbody.innerHTML = "";
    for (const f of state.files) {
      const tr = document.createElement("tr");
      tr.innerHTML = `
        <td><input type="checkbox" data-id="${f.id}" /></td>
        <td>${f.extension.toUpperCase()}</td>
        <td class="offset">${fmtHex(f.offset_bytes)}</td>
        <td class="size">${fmtBytes(f.length_bytes)}</td>
        <td class="recov">${f.recoverability}%</td>
        <td>${f.signature}</td>
      `;
      tbody.appendChild(tr);
    }
    tbody.querySelectorAll('input[type="checkbox"]').forEach((cb) => {
      cb.addEventListener("change", (e) => {
        const id = parseInt(e.target.getAttribute("data-id"), 10);
        if (e.target.checked) state.selected.add(id);
        else state.selected.delete(id);
        updateRecoverButton();
      });
    });
    updateRecoverButton();
  }

  $("select-all").addEventListener("click", () => {
    state.selected = new Set(state.files.map((f) => f.id));
    document.querySelectorAll('#results-body input[type="checkbox"]').forEach((cb) => {
      cb.checked = true;
    });
    updateRecoverButton();
  });
  $("select-none").addEventListener("click", () => {
    state.selected.clear();
    document.querySelectorAll('#results-body input[type="checkbox"]').forEach((cb) => {
      cb.checked = false;
    });
    updateRecoverButton();
  });
  $("head-check").addEventListener("change", (e) => {
    if (e.target.checked) $("select-all").click();
    else $("select-none").click();
  });

  function updateRecoverButton() {
    $("recover").disabled = state.selected.size === 0 || !state.destPath;
  }

  // ---------- recover ----------
  $("recover").addEventListener("click", async () => {
    if (!state.destPath || state.selected.size === 0) return;
    $("recover").disabled = true;
    setStatus(`Recovering ${state.selected.size} files…`);
    const items = state.files
      .filter((f) => state.selected.has(f.id))
      .map((f) => ({
        id: f.id,
        offset_bytes: f.offset_bytes,
        length_bytes: f.length_bytes,
        extension: f.extension,
      }));
    try {
      const r = await invoke("recover_files", {
        imagePath: state.source,
        items,
        destination: state.destPath,
      });
      setStatus(
        `Recovered ${r.written} files · ${fmtBytes(r.bytes_written)} written to ${r.destination}` +
          (r.failed > 0 ? ` · ${r.failed} failed` : ""),
        r.failed > 0 ? "warn" : "ok"
      );
    } catch (e) {
      setStatus("Recovery error: " + e, "err");
    } finally {
      $("recover").disabled = false;
    }
  });

  // ---------- footer link ----------
  document.getElementById("homepage-link").addEventListener("click", (e) => {
    e.preventDefault();
    openExternal(e.currentTarget.href).catch(() => {});
  });
})();
