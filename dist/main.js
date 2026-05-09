// TriRecover — Recuva-style recovery wizard
(function () {
  const { invoke } = window.__TAURI__.core;
  const { open } = window.__TAURI__.dialog;
  const { listen } = window.__TAURI__.event;
  const { open: openExternal } = window.__TAURI__.shell;

  const CIRC = 2 * Math.PI * 52; // ring circumference

  const state = {
    source: null,
    destPath: null,
    files: [],       // raw from backend
    filtered: [],    // after category/search/quality filter
    selected: new Set(),
    drives: [],
    activeCat: "all",
    scanType: "deep",
    totalRecoverable: 0,
  };

  // ---------- Helpers ----------
  const $ = (id) => document.getElementById(id);
  function fmtBytes(n) {
    const u = ["B","KB","MB","GB","TB"];
    let v = n, i = 0;
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return v.toFixed(v >= 100 || i === 0 ? 0 : 1) + " " + u[i];
  }

  function categoryOf(ext) {
    const map = {
      jpg:"Image",png:"Image",gif:"Image",bmp:"Image",tiff:"Image",psd:"Image",ai:"Image",
      mp4:"Video",mov:"Video",mkv:"Video",avi:"Video",
      pdf:"Document",
      docx:"Office",xlsx:"Office",pptx:"Office",
      zip:"Archive",rar:"Archive","7z":"Archive",
      txt:"Text",csv:"Text",sql:"Text",
    };
    return map[ext] || "Other";
  }

  function extClass(ext) {
    const cat = categoryOf(ext);
    if (cat === "Image") return "ext-img";
    if (cat === "Video") return "ext-vid";
    if (cat === "Document" || cat === "Office") return "ext-doc";
    if (cat === "Archive") return "ext-arc";
    return "ext-txt";
  }

  function qualityInfo(recov) {
    if (recov >= 70) return { cls: "good", label: "Excellent" };
    if (recov >= 40) return { cls: "fair", label: "Fair" };
    return { cls: "poor", label: "Poor" };
  }

  function driveIcon(kind) {
    const k = (kind || "").toLowerCase();
    if (k.includes("usb") || k.includes("external")) return { emoji: "🔌", cls: "usb" };
    if (k.includes("sd")) return { emoji: "💾", cls: "sd" };
    if (k.includes("ssd") || k.includes("nvme")) return { emoji: "⚡", cls: "ssd" };
    if (k.includes("hdd")) return { emoji: "💿", cls: "hdd" };
    return { emoji: "💽", cls: "other" };
  }

  function driveBadge(kind) {
    const k = (kind || "").toLowerCase();
    if (k.includes("usb")) return { label: "USB", cls: "usb" };
    if (k.includes("sd")) return { label: "SD", cls: "ssd" };
    if (k.includes("ssd")) return { label: "SSD", cls: "ssd" };
    if (k.includes("nvme")) return { label: "NVMe", cls: "ssd" };
    if (k.includes("hdd")) return { label: "HDD", cls: "hdd" };
    return { label: kind, cls: "hdd" };
  }

  // ---------- Navigation ----------
  function goStep(n) {
    [0,1,2].forEach((i) => {
      const el = $("step-" + i);
      el.hidden = i !== n;
    });
    document.querySelectorAll(".step-dot").forEach((dot) => {
      const s = parseInt(dot.dataset.step);
      dot.classList.toggle("active", s === n);
      dot.classList.toggle("done", s < n);
    });
  }

  // ---------- Version ----------
  invoke("app_version").then((v) => $("version").textContent = "v" + v).catch(() => {});

  // ---------- Source tabs ----------
  $("tab-drive").addEventListener("click", () => {
    $("tab-drive").classList.add("active");
    $("tab-image").classList.remove("active");
    $("panel-drive").hidden = false;
    $("panel-image").hidden = true;
  });
  $("tab-image").addEventListener("click", () => {
    $("tab-image").classList.add("active");
    $("tab-drive").classList.remove("active");
    $("panel-image").hidden = false;
    $("panel-drive").hidden = true;
  });

  // ---------- Drive listing ----------
  async function loadDrives() {
    const grid = $("drives-grid");
    grid.innerHTML = '<div class="drive-card placeholder"><div class="spinner"></div><span>Scanning for drives…</span></div>';
    $("scan-type-section").hidden = true;
    state.source = null;
    try {
      const drives = await invoke("list_drives");
      state.drives = drives;
      grid.innerHTML = "";
      if (drives.length === 0) {
        grid.innerHTML = '<div class="drive-card placeholder"><span>No drives found. Run as Administrator to scan drives, or switch to the <strong>Disk Image</strong> tab.</span></div>';
        return;
      }
      for (const d of drives) {
        const icon = driveIcon(d.kind);
        const badge = driveBadge(d.kind);
        const card = document.createElement("div");
        card.className = "drive-card";
        card.dataset.path = d.path;
        card.innerHTML = `
          <div class="drive-icon ${icon.cls}">${icon.emoji}</div>
          <div class="drive-details">
            <div class="drive-name">${d.model || d.path}</div>
            <div class="drive-meta">${d.path}${d.serial ? " · " + d.serial : ""}</div>
            <div class="drive-size">${fmtBytes(d.size_bytes)}</div>
            <span class="drive-badge ${badge.cls}">${badge.label}</span>
          </div>
        `;
        card.addEventListener("click", () => selectDrive(card, d));
        grid.appendChild(card);
      }
    } catch (e) {
      grid.innerHTML = `<div class="drive-card placeholder"><span>Error: ${e}</span></div>`;
    }
  }

  function selectDrive(card, drive) {
    document.querySelectorAll(".drive-card").forEach((c) => c.classList.remove("selected"));
    card.classList.add("selected");
    state.source = drive.path;
    showScanSection();
  }

  $("refresh-drives").addEventListener("click", loadDrives);
  loadDrives();

  // ---------- Image picker ----------
  $("image-drop").addEventListener("click", async () => {
    const path = await open({
      multiple: false, directory: false,
      filters: [
        { name: "Disk images", extensions: ["img","dd","bin","iso","raw"] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (typeof path === "string") {
      state.source = path;
      const el = $("image-drop");
      el.classList.add("has-file");
      const pathEl = $("image-path");
      pathEl.textContent = path;
      pathEl.hidden = false;
      showScanSection();
    }
  });

  function showScanSection() {
    $("scan-type-section").hidden = false;
    $("scan-type-section").scrollIntoView({ behavior: "smooth" });
  }

  // ---------- Scan type ----------
  document.querySelectorAll(".scan-type-card").forEach((card) => {
    card.addEventListener("click", () => {
      document.querySelectorAll(".scan-type-card").forEach((c) => c.classList.remove("selected"));
      card.classList.add("selected");
      card.querySelector("input").checked = true;
      state.scanType = card.querySelector("input").value;
    });
  });

  // ---------- Start scan ----------
  let progressUn = null, doneUn = null;

  $("btn-start-scan").addEventListener("click", async () => {
    if (!state.source) return;
    goStep(1);

    // Reset UI
    $("ring-fg").style.strokeDashoffset = CIRC;
    $("ring-pct").textContent = "0%";
    $("stat-files").textContent = "0";
    $("stat-scanned").textContent = "0 B";
    $("stat-recoverable").textContent = "0 B";
    $("progress-fill").style.width = "0%";
    $("scan-live").textContent = "";
    $("scan-title").textContent = "Scanning drive…";
    $("scan-subtitle").textContent = "Looking for deleted files by signature";

    state.files = [];
    state.selected.clear();
    state.totalRecoverable = 0;

    // Listen for progress
    if (progressUn) progressUn();
    if (doneUn) doneUn();

    let lastRecoverable = 0;
    progressUn = await listen("scan/progress", (e) => {
      const p = e.payload;
      const pct = p.bytes_total > 0 ? (p.bytes_scanned / p.bytes_total) * 100 : 0;
      $("ring-fg").style.strokeDashoffset = CIRC - (CIRC * pct / 100);
      $("ring-pct").textContent = pct.toFixed(0) + "%";
      $("stat-files").textContent = p.files_found.toLocaleString();
      $("stat-scanned").textContent = fmtBytes(p.bytes_scanned);
      $("progress-fill").style.width = pct.toFixed(1) + "%";
    });

    doneUn = await listen("scan/done", (e) => {
      const d = e.payload;
      state.totalRecoverable = d.bytes_recoverable;
      $("stat-recoverable").textContent = fmtBytes(d.bytes_recoverable);
      $("scan-title").textContent = "Scan complete!";
      $("scan-subtitle").textContent = `Found ${d.files_found} files in ${(d.elapsed_ms/1000).toFixed(1)}s`;
    });

    const minSize = state.scanType === "quick" ? 65536 : 4096;
    try {
      const files = await invoke("scan_image", {
        imagePath: state.source,
        kinds: [],
        minSize,
      });
      state.files = (files || []).map((f) => ({
        ...f,
        category: categoryOf(f.extension),
        quality: qualityInfo(f.recoverability),
        fileName: `recovered_${String(f.id).padStart(4,"0")}.${f.extension}`,
      }));

      if (state.files.length === 0) {
        $("scan-title").textContent = "No deleted files found";
        $("scan-subtitle").textContent = "Try a different drive or scan type";
        setTimeout(() => goStep(0), 3000);
        return;
      }

      // Auto-advance to results
      setTimeout(() => {
        goStep(2);
        buildResults();
      }, 800);
    } catch (e) {
      $("scan-title").textContent = "Scan failed";
      $("scan-subtitle").textContent = String(e);
      $("ring-fg").style.stroke = "var(--danger)";
    }
  });

  // ---------- Results ----------
  function buildResults() {
    // Counts
    const counts = { all: state.files.length };
    for (const f of state.files) {
      counts[f.category] = (counts[f.category] || 0) + 1;
    }
    $("count-all").textContent = counts.all;
    for (const cat of ["Image","Video","Document","Office","Archive","Text"]) {
      const el = $("count-" + cat);
      if (el) el.textContent = counts[cat] || 0;
    }
    $("summary-total").textContent = state.files.length.toLocaleString();
    $("summary-size").textContent = fmtBytes(state.totalRecoverable);

    applyFilters();
  }

  function applyFilters() {
    const cat = state.activeCat;
    const search = $("search-box").value.toLowerCase();
    const quality = $("recov-filter").value;

    state.filtered = state.files.filter((f) => {
      if (cat !== "all" && f.category !== cat) return false;
      if (search && !f.fileName.toLowerCase().includes(search) && !f.extension.toLowerCase().includes(search)) return false;
      if (quality === "good" && f.recoverability < 70) return false;
      if (quality === "fair" && f.recoverability < 40) return false;
      if (quality === "poor" && f.recoverability >= 40) return false;
      return true;
    });

    renderFileTable();
  }

  function renderFileTable() {
    const tbody = $("file-tbody");
    tbody.innerHTML = "";
    $("no-results").hidden = state.filtered.length > 0;

    for (const f of state.filtered) {
      const tr = document.createElement("tr");
      const checked = state.selected.has(f.id);
      if (checked) tr.classList.add("checked");
      tr.innerHTML = `
        <td><input type="checkbox" data-id="${f.id}" ${checked ? "checked" : ""} /></td>
        <td><span class="file-ext ${extClass(f.extension)}">${f.extension}</span></td>
        <td>
          <div class="file-name">${f.fileName}</div>
          <div class="file-offset">Offset 0x${f.offset_bytes.toString(16).toUpperCase()} · ${f.signature}</div>
        </td>
        <td>${fmtBytes(f.length_bytes)}</td>
        <td>
          <span class="quality-dot ${f.quality.cls}"></span>
          <span class="quality-text ${f.quality.cls}">${f.quality.label}</span>
        </td>
      `;
      const cb = tr.querySelector("input");
      cb.addEventListener("change", () => {
        if (cb.checked) state.selected.add(f.id);
        else state.selected.delete(f.id);
        tr.classList.toggle("checked", cb.checked);
        updateRecoverBar();
      });
      tbody.appendChild(tr);
    }
    updateRecoverBar();
  }

  // Category clicks
  $("cat-list").addEventListener("click", (e) => {
    const item = e.target.closest(".cat-item");
    if (!item) return;
    document.querySelectorAll(".cat-item").forEach((c) => c.classList.remove("active"));
    item.classList.add("active");
    state.activeCat = item.dataset.cat;
    applyFilters();
  });

  // Search + filter
  $("search-box").addEventListener("input", applyFilters);
  $("recov-filter").addEventListener("change", applyFilters);

  // Select all
  function syncSelectAll(checked) {
    if (checked) {
      state.filtered.forEach((f) => state.selected.add(f.id));
    } else {
      state.filtered.forEach((f) => state.selected.delete(f.id));
    }
    renderFileTable();
  }
  $("select-all-cb").addEventListener("change", (e) => syncSelectAll(e.target.checked));
  $("th-check-cb").addEventListener("change", (e) => {
    $("select-all-cb").checked = e.target.checked;
    syncSelectAll(e.target.checked);
  });

  function updateRecoverBar() {
    const n = state.selected.size;
    $("selected-count").textContent = n + " selected";
    $("recover-summary").textContent = n === 0
      ? "Select files to recover"
      : `${n} file${n > 1 ? "s" : ""} selected`;
    $("btn-recover").disabled = n === 0 || !state.destPath;
  }

  // ---------- Destination ----------
  $("btn-pick-dest").addEventListener("click", async () => {
    const path = await open({ directory: true });
    if (typeof path === "string") {
      state.destPath = path;
      $("dest-label").textContent = path.length > 35
        ? "…" + path.slice(-32)
        : path;
      updateRecoverBar();
    }
  });

  // ---------- Recover ----------
  $("btn-recover").addEventListener("click", async () => {
    if (!state.destPath || state.selected.size === 0) return;
    const overlay = $("recover-overlay");
    overlay.hidden = false;
    $("recover-progress-section").hidden = false;
    $("recover-done-section").hidden = true;
    $("recover-progress-text").textContent = `Recovering ${state.selected.size} files…`;

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
      $("recover-progress-section").hidden = true;
      $("recover-done-section").hidden = false;
      $("recover-done-title").textContent = r.failed > 0
        ? "Recovery completed with errors"
        : "Recovery complete!";
      $("recover-done-text").textContent =
        `${r.written} files recovered · ${fmtBytes(r.bytes_written)} written` +
        (r.failed > 0 ? ` · ${r.failed} failed` : "");
      state._recoverDest = r.destination;
    } catch (e) {
      $("recover-progress-section").hidden = true;
      $("recover-done-section").hidden = false;
      $("recover-done-title").textContent = "Recovery failed";
      $("recover-done-text").textContent = String(e);
    }
  });

  $("btn-open-dest").addEventListener("click", () => {
    if (state._recoverDest) {
      openExternal(state._recoverDest).catch(() => {});
    }
  });

  $("btn-close-overlay").addEventListener("click", () => {
    $("recover-overlay").hidden = true;
  });
})();
