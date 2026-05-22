import init, { analyzeReplay, replayInfo } from "./pkg/wows_lag_check.js";
import { html, render, svg } from "./vendor/lit-html.js";

const DATA_REPO = "landaire/wows-replay-data";
const DATA_API = `https://api.github.com/repos/${DATA_REPO}/contents`;
const DATA_RAW = `https://raw.githubusercontent.com/${DATA_REPO}/main`;
const entityDefCache = new Map();
const gameDataCache = new Map();
const REPLAY_EXTS = [".wowsreplay", ".wotreplay"];

const dropzone = document.getElementById("dropzone");
const fileInput = document.getElementById("fileInput");
const statusSection = document.getElementById("status");
const statusBox = document.getElementById("statusBox");
const resultSection = document.getElementById("result");
const metaList = document.getElementById("metaList");
const pingStats = document.getElementById("pingStats");
const chartWrap = document.getElementById("chartWrap");
const spikeRows = document.getElementById("spikeRows");
const battleTimeHeader = document.getElementById("battleTimeHeader");
const spikesNote = document.getElementById("spikesNote");
const spikesCard = document.getElementById("spikesCard");
const spikeShips = document.getElementById("spikeShips");
const severityCard = document.getElementById("severityCard");
const severityDot = document.getElementById("severityDot");
const severityLabel = document.getElementById("severityLabel");
const severitySub = document.getElementById("severitySub");
const severityHeadline = document.getElementById("severityHeadline");
const copyBtn = document.getElementById("copyBtn");
const copyBtnLabel = document.getElementById("copyBtnLabel");
const copyPopover = document.getElementById("copyPopover");
const copyChartBtn = document.getElementById("copyChartBtn");
const copyChartBtnLabel = document.getElementById("copyChartBtnLabel");
const thresholdSlider = document.getElementById("thresholdSlider");
const thresholdLabel = document.getElementById("thresholdLabel");
const thresholdReset = document.getElementById("thresholdReset");

let wasmReady = init();
let lastResult = null;
let lastInputs = null;
let popoverTimer = null;

const DEFAULT_THRESHOLD_MS = 500;
let thresholdMs = DEFAULT_THRESHOLD_MS;
let thresholdDebounce = null;

const SEVERITY_STYLE = {
  clean:    { dot: "bg-emerald-400",  card: "border-emerald-700 bg-emerald-950/40",  label: "Clean",    sub: "No stalls detected" },
  minor:    { dot: "bg-yellow-400",   card: "border-yellow-700  bg-yellow-950/40",   label: "Minor",    sub: "Brief hiccups" },
  moderate: { dot: "bg-orange-400",   card: "border-orange-700  bg-orange-950/40",   label: "Moderate", sub: "Multiple or multi-second stalls" },
  severe:   { dot: "bg-rose-500",     card: "border-rose-700    bg-rose-950/50",     label: "Severe",   sub: "Match-affecting stall(s)" },
};

const KIND_ICON = {
  kill:       { icon: "ph-crosshair", color: "text-rose-400" },
  consumable: { icon: "ph-shield", color: "text-sky-400" },
  spotted:    { icon: "ph-eye",    color: "text-amber-400" },
};

// Hover-link elements that share a data-eid (player name <-> table row).
spikesCard.addEventListener("mouseover", (ev) => {
  const el = ev.target.closest("[data-eid]");
  if (!el) return;
  for (const n of spikesCard.querySelectorAll(`[data-eid="${el.dataset.eid}"]`)) {
    n.classList.add("eid-hl");
  }
});
spikesCard.addEventListener("mouseout", (ev) => {
  if (!ev.target.closest("[data-eid]")) return;
  for (const n of spikesCard.querySelectorAll(".eid-hl")) {
    n.classList.remove("eid-hl");
  }
});

function setStatus(kind, msg) {
  statusSection.classList.remove("hidden");
  const ok = kind !== "error";
  statusBox.className = "rounded-lg px-4 py-3 text-sm border flex items-center gap-2.5 " + (
    ok ? "bg-emerald-950 text-emerald-200 border-emerald-900"
       : "bg-rose-950 text-rose-200 border-rose-900"
  );
  render(html`
    <i class="ph ${ok ? "ph-check-circle text-emerald-400" : "ph-warning-circle text-rose-400"} text-xl shrink-0"></i>
    <span>${msg}</span>
  `, statusBox);
}

function showLoading(title, detail, percent) {
  statusSection.classList.remove("hidden");
  statusBox.className = "rounded-lg px-4 py-3 text-sm border bg-sky-950 text-sky-200 border-sky-900";
  render(html`
    <div class="flex items-center gap-2.5">
      <i class="ph ph-circle-notch text-xl text-sky-400 inline-block animate-spin shrink-0"></i>
      <span class="font-medium">${title}</span>
    </div>
    ${detail
      ? html`<p class="mt-1.5 ml-7 text-xs text-sky-400/70 font-mono break-all">${detail}</p>`
      : ""}
    ${percent != null
      ? html`
        <div class="mt-2 ml-7 h-1.5 rounded-full bg-sky-900 overflow-hidden">
          <div class="h-full rounded-full bg-sky-400 transition-all duration-150 ease-out"
            style="width:${Math.max(0, Math.min(100, percent))}%"></div>
        </div>`
      : ""}
  `, statusBox);
}

dropzone.addEventListener("click", () => fileInput.click());
fileInput.addEventListener("change", () => {
  if (fileInput.files.length > 0) handleFile(fileInput.files[0]);
});
["dragenter", "dragover"].forEach((ev) =>
  dropzone.addEventListener(ev, (e) => {
    e.preventDefault();
    dropzone.classList.add("border-sky-500", "bg-slate-900");
  })
);
["dragleave", "drop"].forEach((ev) =>
  dropzone.addEventListener(ev, (e) => {
    e.preventDefault();
    dropzone.classList.remove("border-sky-500", "bg-slate-900");
  })
);
dropzone.addEventListener("drop", (e) => {
  e.preventDefault();
  if (e.dataTransfer.files.length > 0) handleFile(e.dataTransfer.files[0]);
});

copyBtn.addEventListener("click", async () => {
  if (!lastResult) return;
  const md = buildDiscordSummary(lastResult);
  try {
    await navigator.clipboard.writeText(md);
    flashCopyButton("Copied!");
  } catch (err) {
    console.error(err);
    flashCopyButton("Copy failed");
  }
  showPopover();
});

function flashCopyButton(text) {
  copyBtnLabel.textContent = text;
  setTimeout(() => (copyBtnLabel.textContent = "Copy for Discord"), 1500);
}

function flashChartButton(text) {
  copyChartBtnLabel.textContent = text;
  setTimeout(() => (copyChartBtnLabel.textContent = "Copy chart"), 1500);
}

copyChartBtn.addEventListener("click", async () => {
  const svg = chartWrap.querySelector("svg");
  if (!svg) return;
  try {
    const blob = await renderChartToPngBlob(svg);
    if (navigator.clipboard && window.ClipboardItem) {
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      flashChartButton("Copied!");
    } else {
      throw new Error("clipboard image write not supported");
    }
  } catch (err) {
    console.warn("Falling back to download:", err);
    try {
      const blob = await renderChartToPngBlob(svg);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "wows-lag-check-chart.png";
      document.body.appendChild(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
      flashChartButton("Downloaded");
    } catch (err2) {
      console.error(err2);
      flashChartButton("Failed");
    }
  }
});

thresholdSlider.addEventListener("input", () => {
  thresholdMs = parseInt(thresholdSlider.value, 10);
  thresholdLabel.textContent = `${thresholdMs} ms`;
  scheduleReanalyze();
});

thresholdReset.addEventListener("click", () => {
  thresholdSlider.value = String(DEFAULT_THRESHOLD_MS);
  thresholdMs = DEFAULT_THRESHOLD_MS;
  thresholdLabel.textContent = `${DEFAULT_THRESHOLD_MS} ms`;
  scheduleReanalyze();
});

function scheduleReanalyze() {
  if (!lastInputs) return;
  if (thresholdDebounce) clearTimeout(thresholdDebounce);
  thresholdDebounce = setTimeout(() => {
    thresholdDebounce = null;
    const { bytes, defsBundle, gameParams, translations } = lastInputs;
    try {
      const result = analyzeReplay(bytes, defsBundle, gameParams, translations, thresholdMs);
      lastResult = result;
      renderResult(result);
    } catch (err) {
      console.error(err);
      setStatus("error", `Re-analyze failed: ${err.message ?? err}`);
    }
  }, 150);
}

async function renderChartToPngBlob(svg) {
  const vb = svg.viewBox.baseVal;
  const scale = 2;
  const w = vb.width  * scale;
  const h = vb.height * scale;

  const svgStr = new XMLSerializer().serializeToString(svg);
  const svgBlob = new Blob([svgStr], { type: "image/svg+xml;charset=utf-8" });
  const url = URL.createObjectURL(svgBlob);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();

    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#0f172a";
    ctx.fillRect(0, 0, w, h);
    ctx.drawImage(img, 0, 0, w, h);
    return await new Promise((resolve, reject) =>
      canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("toBlob returned null"))), "image/png")
    );
  } finally {
    URL.revokeObjectURL(url);
  }
}

function showPopover() {
  copyPopover.classList.remove("hidden");
  if (popoverTimer) clearTimeout(popoverTimer);
  popoverTimer = setTimeout(() => copyPopover.classList.add("hidden"), 5000);
}

document.addEventListener("click", (e) => {
  if (!copyPopover.classList.contains("hidden")
      && !copyPopover.contains(e.target)
      && e.target !== copyBtn
      && !copyBtn.contains(e.target)) {
    copyPopover.classList.add("hidden");
  }
});

async function handleFile(file) {
  const name = file.name.toLowerCase();
  if (!REPLAY_EXTS.some((ext) => name.endsWith(ext))) {
    setStatus("error", `"${file.name}" is not a replay file.`);
    return;
  }
  showLoading("Reading replay...", file.name, null);
  resultSection.classList.add("hidden");

  try {
    await wasmReady;
    const bytes = new Uint8Array(await file.arrayBuffer());

    const info = replayInfo(bytes);
    const isWot = info.game === "wot";
    let defsBundle = new Uint8Array(0);
    let gameParams = new Uint8Array(0);
    let translations = new Uint8Array(0);
    if (info.dir_name && !isWot) {
      try {
        defsBundle = await fetchEntityDefs(info.dir_name, (done, total) => {
          showLoading(
            "Loading game data...",
            total ? `${info.dir_name}: file ${done} of ${total}`
                  : `${info.dir_name}: listing...`,
            total ? (done / total) * 100 : null,
          );
        });
      } catch (err) {
        console.warn("Entity defs unavailable:", err);
      }
      try {
        const gd = await fetchGameData(info.dir_name, (path, loaded, total) => {
          const mb = (loaded / 1024 / 1024).toFixed(1);
          showLoading("Loading game data...", `${path}  (${mb} MB)`, total ? (loaded / total) * 100 : null);
        });
        gameParams = gd.gameParams;
        translations = gd.translations;
      } catch (err) {
        console.warn("Game params unavailable:", err);
      }
    }

    showLoading("Analyzing...", null, null);
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

    lastInputs = { bytes, defsBundle, gameParams, translations };
    const t0 = performance.now();
    const result = analyzeReplay(bytes, defsBundle, gameParams, translations, thresholdMs);
    const ms = (performance.now() - t0).toFixed(0);
    let dataNote = "";
    if (isWot) dataNote = " (WoT: basic mode)";
    else if (!result.entity_defs_loaded) dataNote = " (no entity defs)";
    else if (!result.game_params_loaded) dataNote = " (no ship names)";
    const corruptClocks = result.corrupt_packet_clocks ?? [];
    let corruptNote = "";
    if (corruptClocks.length > 0) {
      corruptNote = ` Skipped ${corruptClocks.length} corrupt packet${corruptClocks.length === 1 ? "" : "s"}.`;
    }
    setStatus("ok", `Parsed in ${ms} ms. ${result.samples_total} samples, ${result.server_ticks_total} ticks, ${result.spikes.length} spikes.${dataNote}${corruptNote}`);
    lastResult = result;
    renderResult(result);
  } catch (err) {
    console.error(err);
    setStatus("error", `Failed: ${err.message ?? err}`);
  }
}

async function fetchEntityDefs(dirName, onProgress) {
  if (entityDefCache.has(dirName)) {
    return entityDefCache.get(dirName);
  }

  const scriptsDir = `${dirName}/vfs/scripts`;
  const dirs = [scriptsDir, `${scriptsDir}/entity_defs`, `${scriptsDir}/entity_defs/interfaces`];

  const entries = [];
  for (const dir of dirs) {
    onProgress?.(0, 0);
    const resp = await fetch(`${DATA_API}/${dir}?ref=main`);
    if (!resp.ok) throw new Error(`list ${dir}: HTTP ${resp.status}`);
    for (const e of await resp.json()) {
      if (e.type !== "dir") entries.push({ dir, name: e.name, path: e.path });
    }
  }

  const prefix = `${dirName}/vfs/`;
  let done = 0;
  const total = entries.length;
  onProgress?.(0, total);
  const files = await Promise.all(
    entries.map(async (e) => {
      const content = await fetchRepoFile(e.path);
      done += 1;
      onProgress?.(done, total);
      return { key: e.path.slice(prefix.length), content };
    })
  );

  const bundle = packBundle(files);
  entityDefCache.set(dirName, bundle);
  return bundle;
}

async function fetchGameData(dirName, onProgress) {
  if (gameDataCache.has(dirName)) {
    return gameDataCache.get(dirName);
  }
  const gpPath = `${dirName}/game_params.rkyv.zst`;
  const moPath = `${dirName}/translations/en/LC_MESSAGES/global.mo.zst`;
  const gameParams = await fetchRepoFile(gpPath, (loaded, total) => onProgress?.(gpPath, loaded, total));
  const translations = await fetchRepoFile(moPath, (loaded, total) => onProgress?.(moPath, loaded, total));
  const data = { gameParams, translations };
  gameDataCache.set(dirName, data);
  return data;
}

async function fetchBytes(url, onProgress) {
  const resp = await fetch(url);
  if (!resp.ok) throw new Error(`fetch ${url}: HTTP ${resp.status}`);
  if (!onProgress || !resp.body) {
    return new Uint8Array(await resp.arrayBuffer());
  }
  const total = Number(resp.headers.get("content-length")) || 0;
  const reader = resp.body.getReader();
  const chunks = [];
  let loaded = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    loaded += value.length;
    onProgress(loaded, total);
  }
  const out = new Uint8Array(loaded);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

// Resolves repo symlinks (`../`-relative path blobs) to the real file.
async function fetchRepoFile(repoPath, onProgress) {
  const content = await fetchBytes(`${DATA_RAW}/${repoPath}`);
  if (new TextDecoder().decode(content.subarray(0, 3)) !== "../") {
    return content;
  }
  const dir = repoPath.slice(0, repoPath.lastIndexOf("/"));
  const target = new TextDecoder().decode(content).trim();
  return fetchBytes(`${DATA_RAW}/${resolveRepoPath(dir, target)}`, onProgress);
}

function resolveRepoPath(baseDir, rel) {
  const parts = baseDir.split("/").filter(Boolean);
  for (const seg of rel.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg !== "." && seg !== "") parts.push(seg);
  }
  return parts.join("/");
}

function packBundle(files) {
  const enc = new TextEncoder();
  const parts = files.map((f) => ({ key: enc.encode(f.key), content: f.content }));
  let size = 4;
  for (const p of parts) size += 8 + p.key.length + p.content.length;

  const out = new Uint8Array(size);
  const view = new DataView(out.buffer);
  let off = 0;
  view.setUint32(off, parts.length, true); off += 4;
  for (const p of parts) {
    view.setUint32(off, p.key.length, true); off += 4;
    out.set(p.key, off); off += p.key.length;
    view.setUint32(off, p.content.length, true); off += 4;
    out.set(p.content, off); off += p.content.length;
  }
  return out;
}

function fmtClock(seconds) {
  const m = Math.floor(seconds / 60);
  const s = seconds - m * 60;
  return `${String(m).padStart(2, "0")}:${s.toFixed(3).padStart(6, "0")}`;
}

function fmtClockShort(seconds) {
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds - m * 60);
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function fmtBattleClock(replayClock, battleStart) {
  const elapsed = replayClock - battleStart;
  if (elapsed < 0) return `pre-battle`;
  return fmtClock(elapsed);
}

function fmtSinceLast(seconds) {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "";
  const totalMs = Math.round(seconds * 1000);
  const m = Math.floor(totalMs / 60000);
  const s = Math.floor((totalMs % 60000) / 1000);
  const ms = totalMs % 1000;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}:${String(ms).padStart(3, "0")}`;
}

function renderResult(r) {
  resultSection.classList.remove("hidden");

  const sev = r.severity;
  const style = SEVERITY_STYLE[sev.severity] ?? SEVERITY_STYLE.minor;
  severityCard.className = `rounded-xl border p-5 flex items-center gap-4 ${style.card}`;
  severityDot.className = `size-4 rounded-full shrink-0 ${style.dot}`;
  severityLabel.textContent = style.label;
  severitySub.textContent = style.sub;
  severityHeadline.textContent = sev.headline;

  battleTimeHeader.textContent = r.battle_start_clock_exact ? "Battle time" : "Battle time (approx)";
  render(spikeRowsTemplate(r), spikeRows);
  spikesNote.textContent = r.spikes.length
    ? `${r.spikes.length} gap${r.spikes.length === 1 ? "" : "s"} >= ${r.spike_threshold_ms} ms`
    : "";

  const allShips = involvedShips(r.spikes.flatMap((s) => s.preceding_events ?? []));
  spikeShips.classList.toggle("hidden", allShips.length === 0);
  render(
    allShips.length
      ? html`<div class="text-slate-500 uppercase tracking-wider text-[10px] mb-1">Players</div>${shipsTable(allShips)}`
      : "",
    spikeShips,
  );

  renderChart(r);

  const m = r.meta;
  const metaRows = [
    ["Player",   m.player_name],
    ["Vehicle",  m.player_vehicle],
    ["Map",      m.map_display_name || m.map],
    ["Mode",     m.match_group ? `${m.game_type} (${m.match_group})` : m.game_type],
    ["Server",   m.region ?? "(unknown)"],
    ["Date",     m.date_time],
    ["Client",   m.client_version],
  ];
  if (m.players_per_team) metaRows.push(["Players/team", String(m.players_per_team)]);
  if (m.battle_duration_s) metaRows.push(["Battle duration", `${m.battle_duration_s}s`]);
  metaRows.push(["Replay duration", `${r.replay_duration_s.toFixed(1)}s`]);
  if (m.arena_id) {
    metaRows.push(["Arena ID", `${m.arena_id} (0x${m.arena_id_hex})`]);
  }
  render(defList(metaRows, "text-slate-200 font-mono break-all"), metaList);

  const ps = r.ping_stats;
  render(defList([
    ["min",  `${ps.min_ms} ms`],
    ["mean", `${ps.mean_ms.toFixed(1)} ms`],
    ["p95",  `${ps.p95_ms} ms`],
    ["max",  `${ps.max_ms} ms`],
  ], "text-slate-200 font-medium"), pingStats);
}

function spikeRowsTemplate(r) {
  if (r.spikes.length === 0) {
    return html`<tr><td colspan="7" class="py-3 text-slate-500">No gaps over ${r.spike_threshold_ms} ms detected.</td></tr>`;
  }
  return r.spikes.map((s) => spikeRow(s, r));
}

function spikeRow(s, r) {
  const stallColor = s.client_present_during_gap ? "bg-orange-400" : "bg-violet-400";
  const stallLabel = s.client_present_during_gap ? "server-only" : "client+server";
  const gapClass = s.gap_seconds > 2 ? "text-rose-300 font-semibold" : "text-slate-200";
  const sinceLast = fmtSinceLast(s.seconds_since_previous_spike);
  const mainRow = html`
    <tr class="hover:bg-slate-800/50 border-t border-slate-800">
      <td class="py-2 pr-3 font-mono text-slate-300">${fmtClock(s.gap_start_clock)}</td>
      <td class="py-2 pr-3 font-mono text-slate-300">${fmtBattleClock(s.gap_start_clock, r.battle_start_clock_s)}</td>
      <td class="py-2 pr-3 text-right font-mono text-slate-400">${sinceLast || html`<span class="text-slate-600">-</span>`}</td>
      <td class="py-2 pr-3 text-right font-mono ${gapClass}">${(s.gap_seconds * 1000).toFixed(0)} ms</td>
      <td class="py-2 pr-3 text-right font-mono text-slate-200">${s.peak_ping_ms} ms</td>
      <td class="py-2 pr-3">
        <span class="inline-flex items-center gap-1.5">
          <span class="size-2 rounded-full ${stallColor}"></span>${stallLabel}
        </span>
      </td>
      <td class="py-2 pr-3 text-right font-mono text-slate-400">${s.client_rate_hz.toFixed(1)} Hz</td>
    </tr>`;

  const events = s.preceding_events ?? [];
  const hasBurst = s.burst_ticks > 1;
  if (events.length === 0 && !hasBurst) return mainRow;

  return html`${mainRow}
    <tr class="bg-slate-900/60">
      <td colspan="7" class="pb-2 pl-4 pr-3">
        <div class="text-xs text-slate-400 space-y-0.5">
          ${hasBurst
            ? html`<div class="text-orange-300/90">Server stutter: ${s.burst_ticks} server ticks stamped the same clock before the freeze</div>`
            : ""}
          ${events.length
            ? html`<div class="text-slate-500 uppercase tracking-wider text-[10px]">in the 2s before</div>`
            : ""}
          ${events.map((e) => eventLine(s, e))}
        </div>
      </td>
    </tr>`;
}

// Event sentence as text/ship parts. UI renders ships as chips; Discord as plain names.
function composeEvent(e) {
  const ships = e.ships ?? [];
  const t = (v) => ({ t: "text", v });
  const sh = (i) => ({ t: "ship", v: ships[i] });
  if (e.kind === "spotted") return [t("Spotted "), sh(0)];
  if (e.kind === "consumable") return [sh(0), t(` used ${e.detail}`)];
  if (e.kind === "kill") {
    const eff = e.death_effect ? `; death effect: ${e.death_effect}` : "";
    return [sh(0), t(" destroyed by "), sh(1), t(` (${e.detail})${eff}`)];
  }
  return [t(e.kind)];
}

function eventLine(s, e) {
  const dt = (s.gap_start_clock - e.clock).toFixed(2);
  const kind = KIND_ICON[e.kind] ?? { icon: "ph-circle", color: "text-slate-400" };
  const parts = composeEvent(e).map((p) =>
    p.t === "ship"
      ? html`<span class="evt-name" data-eid=${p.v.entity_id}>${p.v.player}</span>`
      : p.v
  );
  return html`
    <div class="flex gap-2 items-baseline">
      <span class="text-slate-500 font-mono shrink-0">-${dt}s (${e.tick_offset} ticks)</span>
      <span class="inline-flex items-baseline gap-1.5 flex-wrap">
        <i class="ph ${kind.icon} ${kind.color} text-sm shrink-0"></i>
        <span>${parts}</span>
      </span>
    </div>`;
}

function involvedShips(events) {
  const seen = new Map();
  for (const e of events) {
    for (const ship of (e.ships ?? [])) {
      if (!seen.has(ship.entity_id)) seen.set(ship.entity_id, ship);
    }
  }
  return [...seen.values()];
}

function shipsTable(ships) {
  return html`
    <table class="w-full text-[11px] border-separate border-spacing-0">
      <thead>
        <tr class="text-slate-500 uppercase tracking-wider text-[10px] text-left">
          <th class="font-medium pr-3 pb-1">Player</th>
          <th class="font-medium pr-3 pb-1">Ship</th>
          <th class="font-medium pr-3 pb-1">Camo</th>
          <th class="font-medium pr-3 pb-1 text-right">Entity</th>
          <th class="font-medium pb-1 text-right">Ship ID</th>
        </tr>
      </thead>
      <tbody>
        ${ships.map((sh) => html`
          <tr data-eid=${sh.entity_id}>
            <td class="pr-3 py-0.5 text-slate-300">${sh.player}</td>
            <td class="pr-3 py-0.5 text-slate-300">${sh.ship_name ?? "-"}</td>
            <td class="pr-3 py-0.5 text-slate-400">${sh.camo ?? "-"}</td>
            <td class="pr-3 py-0.5 text-right font-mono text-slate-500">${sh.entity_id}</td>
            <td class="py-0.5 text-right font-mono text-slate-500">${sh.ship_param_id ?? "-"}</td>
          </tr>`)}
      </tbody>
    </table>`;
}

function defList(rows, ddClass) {
  return rows.map(([k, v]) => html`
    <dt class="text-slate-500">${k}</dt>
    <dd class="${ddClass}">${v}</dd>
  `);
}

function buildDiscordSummary(r) {
  const m = r.meta;
  const sev = r.severity;
  const rating = SEVERITY_STYLE[sev.severity]?.label ?? "Unknown";
  const arenaLine = m.arena_id
    ? `**Arena ID:** \`${m.arena_id}\` (\`0x${m.arena_id_hex}\`)`
    : `**Arena ID:** (not extracted)`;
  const serverLine = m.region ? `**Server:** ${m.region}` : `**Server:** (unknown)`;

  let body = `**Lag Analysis: ${rating}**
${sev.headline}

**Match:** ${m.map_display_name || m.map} (${m.game_type})
**Player:** ${m.player_name} on ${m.player_vehicle}
**Date:** ${m.date_time}
**Client:** ${m.client_version}
${serverLine}
${arenaLine}
`;

  if (r.spikes.length > 0) {
    body += `\n**Spikes** (gaps >= ${r.spike_threshold_ms} ms):\n\`\`\`\n`;
    body += `battle  gap     peak   type\n`;
    body += `------  ------  -----  --------------\n`;
    for (const s of r.spikes) {
      const bt = fmtClockShort(Math.max(0, s.gap_start_clock - r.battle_start_clock_s));
      const gap = `${(s.gap_seconds * 1000).toFixed(0)}ms`.padStart(6);
      const peak = `${s.peak_ping_ms}ms`.padStart(5);
      const type = s.client_present_during_gap ? "server-only" : "client+server";
      body += `${bt.padEnd(6)}  ${gap}  ${peak}  ${type}\n`;
      if (s.burst_ticks > 1) {
        body += `        burst: ${s.burst_ticks} server ticks stamped one clock\n`;
      }
      for (const e of (s.preceding_events ?? [])) {
        const line = composeEvent(e).map((p) => (p.t === "ship" ? p.v.player : p.v)).join("");
        body += `        -${(s.gap_start_clock - e.clock).toFixed(2)}s (${e.tick_offset} ticks)  ${line}\n`;
      }
    }
    body += `\`\`\`\n`;

    const ships = involvedShips(r.spikes.flatMap((s) => s.preceding_events ?? []));
    if (ships.length > 0) {
      body += `\n**Ships**\n${shipTableText(ships)}`;
    }
  }

  return body;
}

function shipTableText(ships) {
  const headers = ["Player", "Entity", "Ship ID", "Ship", "Camo"];
  const rows = ships.map((sh) => [
    sh.player,
    String(sh.entity_id),
    sh.ship_param_id != null ? String(sh.ship_param_id) : "-",
    sh.ship_name ?? "-",
    sh.camo ?? "-",
  ]);
  const widths = headers.map((h, i) => Math.max(h.length, ...rows.map((r) => r[i].length)));
  const fmt = (cells) => cells.map((c, i) => c.padEnd(widths[i])).join("  ").trimEnd();
  let out = "```\n" + fmt(headers) + "\n" + fmt(widths.map((n) => "-".repeat(n))) + "\n";
  for (const r of rows) out += fmt(r) + "\n";
  return out + "```\n";
}

function renderChart(r) {
  const width = 1200;
  const height = 300;
  const pad = { l: 56, r: 16, t: 12, b: 36 };
  const innerW = width - pad.l - pad.r;
  const innerH = height - pad.t - pad.b;

  const xMax = Math.max(r.replay_duration_s, 1);
  const yMax = Math.max(50, Math.min(r.ping_stats.max_ms + 10, Math.max(140, r.ping_stats.p95_ms * 3)));

  const x = (clock) => pad.l + (clock / xMax) * innerW;
  const y = (ping) => pad.t + (1 - Math.min(ping, yMax) / yMax) * innerH;
  const yFps = (fps) => pad.t + (1 - Math.min(fps, 144) / 144) * innerH;

  const step = Math.max(1, Math.floor(r.samples.length / 2000));
  const pingPts = [];
  const fpsPts = [];
  for (let i = 0; i < r.samples.length; i += step) {
    const s = r.samples[i];
    pingPts.push(`${x(s.clock).toFixed(1)},${y(s.ping_ms).toFixed(1)}`);
    fpsPts.push(`${x(s.clock).toFixed(1)},${yFps(s.fps).toFixed(1)}`);
  }

  const yTicks = [0, Math.round(yMax / 4), Math.round(yMax / 2), Math.round((3 * yMax) / 4), Math.round(yMax)];
  const yTickLines = yTicks.map((t) => svg`
    <line x1=${pad.l} x2=${pad.l + innerW} y1=${y(t)} y2=${y(t)} stroke="#1e293b" stroke-width="1"/>
  `);
  const yTickLabels = yTicks.map((t) => svg`
    <text x=${pad.l - 8} y=${y(t) + 4} text-anchor="end" fill="#64748b" font-size="11" font-family="ui-monospace, monospace">${t}</text>
  `);

  const xTicks = [];
  for (let s = 0; s <= xMax; s += 60) xTicks.push(s);
  const xTickLines = xTicks.map((t) => svg`
    <line x1=${x(t)} x2=${x(t)} y1=${pad.t} y2=${pad.t + innerH} stroke="#1e293b" stroke-width="1"/>
  `);
  const xTickLabels = xTicks.map((t) => svg`
    <text x=${x(t)} y=${pad.t + innerH + 16} text-anchor="middle" fill="#64748b" font-size="11" font-family="ui-monospace, monospace">${fmtClock(t).slice(0, 5)}</text>
  `);

  const spikeBands = r.spikes.map((s) => {
    const xs = x(s.gap_start_clock);
    const xe = x(s.gap_end_clock);
    const w = Math.max(2, xe - xs);
    const fill = s.client_present_during_gap ? "rgba(251,146,60,0.32)" : "rgba(167,139,250,0.42)";
    const stroke = s.client_present_during_gap ? "rgba(251,146,60,0.65)" : "rgba(167,139,250,0.85)";
    return svg`<rect x=${xs} y=${pad.t} width=${w} height=${innerH} fill=${fill} stroke=${stroke} stroke-width="1"/>`;
  });

  // Quiet-interval annotation between adjacent spike bands.
  const sinceLineY = pad.t + 8;
  const sinceY = pad.t + 22;
  const LABEL_HALF_PX = 40;
  const MIN_LABEL_PX = 110;
  const TICK_HALF = 5;
  const sinceAnnotations = r.spikes.slice(1).flatMap((s, i) => {
    const prev = r.spikes[i];
    const idle = s.gap_start_clock - prev.gap_end_clock;
    if (!(idle > 0)) return [];
    const x1 = x(prev.gap_end_clock);
    const x2 = x(s.gap_start_clock);
    if (x2 - x1 < MIN_LABEL_PX) return [];
    const mid = (x1 + x2) / 2;
    const labelLeft = mid - LABEL_HALF_PX;
    const labelRight = mid + LABEL_HALF_PX;
    return [svg`
      <line x1=${x1} x2=${labelLeft} y1=${sinceLineY} y2=${sinceLineY} stroke="#94a3b8" stroke-width="2" stroke-dasharray="5 3"/>
      <line x1=${labelRight} x2=${x2} y1=${sinceLineY} y2=${sinceLineY} stroke="#94a3b8" stroke-width="2" stroke-dasharray="5 3"/>
      <line x1=${x1} x2=${x1} y1=${sinceLineY - TICK_HALF} y2=${sinceLineY + TICK_HALF} stroke="#94a3b8" stroke-width="2"/>
      <line x1=${x2} x2=${x2} y1=${sinceLineY - TICK_HALF} y2=${sinceLineY + TICK_HALF} stroke="#94a3b8" stroke-width="2"/>
      <rect x=${labelLeft} y=${sinceLineY + 2} width=${LABEL_HALF_PX * 2} height="16" rx="3" fill="rgba(15,23,42,0.85)" stroke="#334155" stroke-width="1"/>
      <text x=${mid} y=${sinceY} text-anchor="middle" fill="#f1f5f9" font-size="12" font-family="ui-monospace, monospace" font-weight="500">${fmtSinceLast(idle)}</text>
    `];
  });

  const bx = x(r.battle_start_clock_s);
  const battleMarker = svg`
    <line x1=${bx} x2=${bx} y1=${pad.t} y2=${pad.t + innerH} stroke="#475569" stroke-width="1" stroke-dasharray="3 3"/>
    <text x=${bx + 4} y=${pad.t + 12} fill="#94a3b8" font-size="10">battle start</text>
  `;

  render(
    html`
      <svg
        viewBox="0 0 ${width} ${height}"
        preserveAspectRatio="none"
        style="width:100%; min-width:760px; height:${height}px;"
      >
        ${yTickLines}
        ${xTickLines}
        ${spikeBands}
        ${sinceAnnotations}
        ${battleMarker}
        <polyline points=${fpsPts.join(" ")} fill="none" stroke="#64748b" stroke-width="1" stroke-opacity="0.6"/>
        <polyline points=${pingPts.join(" ")} fill="none" stroke="#38bdf8" stroke-width="1.4"/>
        ${yTickLabels}
        ${xTickLabels}
      </svg>
    `,
    chartWrap,
  );
}
