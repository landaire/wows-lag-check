import init, { analyzeReplay, replayInfo } from "./pkg/wows_lag_check.js";

// Per-build game entity definitions, fetched on demand.
const DATA_REPO = "landaire/wows-replay-data";
const DATA_API = `https://api.github.com/repos/${DATA_REPO}/contents`;
const DATA_RAW = `https://raw.githubusercontent.com/${DATA_REPO}/main`;
const entityDefCache = new Map(); // dirName -> Uint8Array bundle

const dropzone = document.getElementById("dropzone");
const fileInput = document.getElementById("fileInput");
const statusSection = document.getElementById("status");
const statusBox = document.getElementById("statusBox");
const resultSection = document.getElementById("result");
const metaList = document.getElementById("metaList");
const pingStats = document.getElementById("pingStats");
const chartWrap = document.getElementById("chartWrap");
const spikeRows = document.getElementById("spikeRows");
const spikesNote = document.getElementById("spikesNote");
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

let wasmReady = init();
let lastResult = null;
let popoverTimer = null;

const SEVERITY_STYLE = {
  clean:    { dot: "bg-emerald-400",  card: "border-emerald-700 bg-emerald-950/40",  label: "Clean",    sub: "No stalls detected" },
  minor:    { dot: "bg-yellow-400",   card: "border-yellow-700  bg-yellow-950/40",   label: "Minor",    sub: "Brief hiccups" },
  moderate: { dot: "bg-orange-400",   card: "border-orange-700  bg-orange-950/40",   label: "Moderate", sub: "Multiple or multi-second stalls" },
  severe:   { dot: "bg-rose-500",     card: "border-rose-700    bg-rose-950/50",     label: "Severe",   sub: "Match-affecting stall(s)" },
};

function setStatus(kind, msg) {
  statusSection.classList.remove("hidden");
  statusBox.textContent = msg;
  statusBox.className = "rounded-lg px-4 py-3 text-sm " + (
    kind === "error"   ? "bg-rose-950 text-rose-200 border border-rose-900" :
    kind === "loading" ? "bg-sky-950 text-sky-200 border border-sky-900" :
                         "bg-emerald-950 text-emerald-200 border border-emerald-900"
  );
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
  if (!file.name.toLowerCase().endsWith(".wowsreplay")) {
    setStatus("error", `"${file.name}" doesn't look like a .wowsreplay file.`);
    return;
  }
  setStatus("loading", `Parsing ${file.name} (${(file.size / 1024 / 1024).toFixed(2)} MB)...`);
  resultSection.classList.add("hidden");

  try {
    await wasmReady;
    const bytes = new Uint8Array(await file.arrayBuffer());

    const info = replayInfo(bytes);
    let defsBundle = new Uint8Array(0);
    if (info.dir_name) {
      try {
        setStatus("loading", `Loading entity definitions for build ${info.build}...`);
        defsBundle = await fetchEntityDefs(info.dir_name);
      } catch (err) {
        console.warn("Entity defs unavailable, parsing without them:", err);
      }
    }

    const t0 = performance.now();
    const result = analyzeReplay(bytes, defsBundle);
    const ms = (performance.now() - t0).toFixed(0);
    const defsNote = result.entity_defs_loaded ? "" : " (entity defs unavailable)";
    setStatus("ok", `Parsed in ${ms} ms. ${result.samples_total} ping samples, ${result.server_ticks_total} server ticks, ${result.spikes.length} spikes.${defsNote}`);
    lastResult = result;
    renderResult(result);
  } catch (err) {
    console.error(err);
    setStatus("error", `Failed to parse: ${err.message ?? err}`);
  }
}

/// Fetch the entity-def tree for a build from the wows-replay-data repo and
/// pack it into the bundle format the WASM module expects. Git symlinks (the
/// repo dedupes def files via a content-addressed vfs_common/ store) are
/// detected by their `../` content and resolved to the real blob.
async function fetchEntityDefs(dirName) {
  if (entityDefCache.has(dirName)) {
    return entityDefCache.get(dirName);
  }

  const scriptsDir = `${dirName}/vfs/scripts`;
  const dirs = [scriptsDir, `${scriptsDir}/entity_defs`, `${scriptsDir}/entity_defs/interfaces`];

  const entries = [];
  for (const dir of dirs) {
    const resp = await fetch(`${DATA_API}/${dir}?ref=main`);
    if (!resp.ok) throw new Error(`list ${dir}: HTTP ${resp.status}`);
    for (const e of await resp.json()) {
      if (e.type !== "dir") entries.push({ dir, name: e.name, path: e.path });
    }
  }

  const prefix = `${dirName}/vfs/`;
  const files = await Promise.all(
    entries.map(async (e) => {
      let content = await fetchBytes(`${DATA_RAW}/${e.path}`);
      // A git symlink reads back as its target path (e.g. ../../../vfs_common/ab/...).
      const asText = new TextDecoder().decode(content.subarray(0, 3));
      if (asText.startsWith("../")) {
        const target = new TextDecoder().decode(content).trim();
        content = await fetchBytes(`${DATA_RAW}/${resolveRepoPath(e.dir, target)}`);
      }
      return { key: e.path.slice(prefix.length), content };
    })
  );

  const bundle = packBundle(files);
  entityDefCache.set(dirName, bundle);
  return bundle;
}

async function fetchBytes(url) {
  const resp = await fetch(url);
  if (!resp.ok) throw new Error(`fetch ${url}: HTTP ${resp.status}`);
  return new Uint8Array(await resp.arrayBuffer());
}

/// Resolve a relative path (a symlink target) against a repo directory,
/// returning a repo-root-relative path.
function resolveRepoPath(baseDir, rel) {
  const parts = baseDir.split("/").filter(Boolean);
  for (const seg of rel.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg !== "." && seg !== "") parts.push(seg);
  }
  return parts.join("/");
}

/// Pack [{key, content}] into [u32 count]([u32 keyLen][key][u32 contentLen][content])*
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

function renderResult(r) {
  resultSection.classList.remove("hidden");

  const sev = r.severity;
  const style = SEVERITY_STYLE[sev.severity] ?? SEVERITY_STYLE.minor;
  severityCard.className = `rounded-xl border p-5 flex items-center gap-4 ${style.card}`;
  severityDot.className = `size-4 rounded-full shrink-0 ${style.dot}`;
  severityLabel.textContent = style.label;
  severitySub.textContent = style.sub;
  severityHeadline.textContent = sev.headline;

  if (r.spikes.length === 0) {
    spikeRows.innerHTML = `<tr><td colspan="6" class="py-3 text-slate-500">No gaps over 500 ms detected.</td></tr>`;
    spikesNote.textContent = "";
  } else {
    const KIND_DOT = { kill: "bg-rose-400", consumable: "bg-sky-400", spotted: "bg-amber-400" };
    spikeRows.innerHTML = r.spikes.map((s) => {
      const stall = s.client_present_during_gap
        ? `<span class="inline-flex items-center gap-1.5"><span class="size-2 rounded-full bg-orange-400"></span>server-only</span>`
        : `<span class="inline-flex items-center gap-1.5"><span class="size-2 rounded-full bg-violet-400"></span>client+server</span>`;
      const row = `<tr class="hover:bg-slate-800/50 border-t border-slate-800">
        <td class="py-2 pr-3 font-mono text-slate-300">${fmtClock(s.gap_start_clock)}</td>
        <td class="py-2 pr-3 font-mono text-slate-300">${fmtBattleClock(s.gap_start_clock, r.battle_start_clock_approx_s)}</td>
        <td class="py-2 pr-3 text-right font-mono ${s.gap_seconds > 2 ? "text-rose-300 font-semibold" : "text-slate-200"}">${(s.gap_seconds * 1000).toFixed(0)} ms</td>
        <td class="py-2 pr-3 text-right font-mono text-slate-200">${s.peak_ping_ms} ms</td>
        <td class="py-2 pr-3">${stall}</td>
        <td class="py-2 pr-3 text-right font-mono text-slate-400">${s.client_rate_hz.toFixed(1)} Hz</td>
      </tr>`;
      const events = (s.preceding_events ?? []);
      const hasBurst = s.burst_ticks > 1;
      if (events.length === 0 && !hasBurst) return row;
      const items = events.map((e) => {
        const dt = (s.gap_start_clock - e.clock).toFixed(2);
        const dot = KIND_DOT[e.kind] ?? "bg-slate-400";
        // entity_id / ship_param_id are numbers, safe to inline as-is.
        let ids = "";
        if (e.entity_id != null) ids += ` entity ${e.entity_id}`;
        if (e.ship_param_id != null) ids += ` ship ${e.ship_param_id}`;
        const idSpan = ids ? `<span class="text-slate-600 font-mono">${ids}</span>` : "";
        return `<div class="flex gap-2 items-baseline">`
          + `<span class="text-slate-500 font-mono shrink-0">-${dt}s (${e.tick_offset} ticks)</span>`
          + `<span class="inline-flex items-baseline gap-1.5 flex-wrap">`
          + `<span class="size-1.5 rounded-full ${dot}"></span>${escapeHtml(e.text)}${idSpan}</span></div>`;
      }).join("");
      // s.burst_ticks is a number, safe to inline.
      const burstNote = hasBurst
        ? `<div class="text-orange-300/90">Server stutter: ${s.burst_ticks} server ticks stamped the same clock before the freeze</div>`
        : "";
      const eventsBlock = events.length
        ? `<div class="text-slate-500 uppercase tracking-wider text-[10px]">in the 2s before</div>${items}`
        : "";
      const eventRow = `<tr class="bg-slate-900/60"><td colspan="6" class="pb-2 pl-4 pr-3">`
        + `<div class="text-xs text-slate-400 space-y-0.5">${burstNote}${eventsBlock}</div>`
        + `</td></tr>`;
      return row + eventRow;
    }).join("");
    spikesNote.textContent = `${r.spikes.length} gap${r.spikes.length === 1 ? "" : "s"} >= 500 ms`;
  }

  renderChart(r);

  const m = r.meta;
  const metaRows = [
    ["Player",          m.player_name],
    ["Ship",            m.player_vehicle],
    ["Map",             `${m.map_display_name || m.map}`],
    ["Mode",            `${m.game_type} (${m.match_group})`],
    ["Server",          m.region ?? "(unknown)"],
    ["Date",            m.date_time],
    ["Client",          m.client_version],
    ["Players/team",    String(m.players_per_team)],
    ["Battle duration", `${m.battle_duration_s}s`],
    ["Replay duration", `${r.replay_duration_s.toFixed(1)}s`],
    ["Arena ID",        m.arena_id
      ? `${m.arena_id} (0x${m.arena_id_hex})`
      : r.entity_defs_loaded
        ? "(not found)"
        : `(needs entity defs for build ${m.client_build ?? "?"})`],
  ];
  // metaRows hold replay-controlled strings (player name, map, etc.); build
  // the nodes with textContent so markup in those values can never execute.
  renderDefList(metaList, metaRows, "text-slate-200 font-mono break-all");

  const ps = r.ping_stats;
  renderDefList(pingStats, [
    ["min",  `${ps.min_ms} ms`],
    ["mean", `${ps.mean_ms.toFixed(1)} ms`],
    ["p95",  `${ps.p95_ms} ms`],
    ["max",  `${ps.max_ms} ms`],
  ], "text-slate-200 font-medium");
}

/// Render `[key, value]` pairs into a <dl> as <dt>/<dd> nodes. Uses
/// textContent, so values are never interpreted as HTML.
function renderDefList(dl, rows, ddClass) {
  dl.replaceChildren();
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.className = "text-slate-500";
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.className = ddClass;
    dd.textContent = v;
    dl.append(dt, dd);
  }
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
    body += `\n**Spikes** (gaps >= 500 ms):\n\`\`\`\n`;
    body += `battle  gap     peak   type\n`;
    body += `------  ------  -----  --------------\n`;
    for (const s of r.spikes) {
      const bt = fmtClockShort(Math.max(0, s.gap_start_clock - r.battle_start_clock_approx_s));
      const gap = `${(s.gap_seconds * 1000).toFixed(0)}ms`.padStart(6);
      const peak = `${s.peak_ping_ms}ms`.padStart(5);
      const type = s.client_present_during_gap ? "server-only" : "client+server";
      body += `${bt.padEnd(6)}  ${gap}  ${peak}  ${type}\n`;
      if (s.burst_ticks > 1) {
        body += `        burst: ${s.burst_ticks} server ticks stamped one clock\n`;
      }
      for (const e of (s.preceding_events ?? [])) {
        const eid = e.entity_id != null ? ` [entity ${e.entity_id}]` : "";
        body += `        -${(s.gap_start_clock - e.clock).toFixed(2)}s (${e.tick_offset} ticks)  ${e.text}${eid}\n`;
      }
    }
    body += `\`\`\`\n`;
  }

  return body;
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
  const yTickLines = yTicks.map((t) =>
    `<line x1="${pad.l}" x2="${pad.l + innerW}" y1="${y(t)}" y2="${y(t)}" stroke="#1e293b" stroke-width="1"/>`
  ).join("");
  const yTickLabels = yTicks.map((t) =>
    `<text x="${pad.l - 8}" y="${y(t) + 4}" text-anchor="end" fill="#64748b" font-size="11" font-family="ui-monospace, monospace">${t}</text>`
  ).join("");

  const xTicks = [];
  for (let s = 0; s <= xMax; s += 60) xTicks.push(s);
  const xTickLines = xTicks.map((t) =>
    `<line x1="${x(t)}" x2="${x(t)}" y1="${pad.t}" y2="${pad.t + innerH}" stroke="#1e293b" stroke-width="1"/>`
  ).join("");
  const xTickLabels = xTicks.map((t) =>
    `<text x="${x(t)}" y="${pad.t + innerH + 16}" text-anchor="middle" fill="#64748b" font-size="11" font-family="ui-monospace, monospace">${fmtClock(t).slice(0, 5)}</text>`
  ).join("");

  const spikeBands = r.spikes.map((s) => {
    const xs = x(s.gap_start_clock);
    const xe = x(s.gap_end_clock);
    const w = Math.max(2, xe - xs);
    const fill = s.client_present_during_gap ? "rgba(251,146,60,0.32)" : "rgba(167,139,250,0.42)";
    const stroke = s.client_present_during_gap ? "rgba(251,146,60,0.65)" : "rgba(167,139,250,0.85)";
    return `<rect x="${xs}" y="${pad.t}" width="${w}" height="${innerH}" fill="${fill}" stroke="${stroke}" stroke-width="1"/>`;
  }).join("");

  const bx = x(r.battle_start_clock_approx_s);
  const battleMarker = `
    <line x1="${bx}" x2="${bx}" y1="${pad.t}" y2="${pad.t + innerH}" stroke="#475569" stroke-width="1" stroke-dasharray="3 3"/>
    <text x="${bx + 4}" y="${pad.t + 12}" fill="#94a3b8" font-size="10">battle 0:00</text>
  `;

  chartWrap.innerHTML = `
    <svg viewBox="0 0 ${width} ${height}" preserveAspectRatio="none" style="width:100%; min-width:760px; height:${height}px;">
      ${yTickLines}
      ${xTickLines}
      ${spikeBands}
      ${battleMarker}
      <polyline points="${fpsPts.join(" ")}" fill="none" stroke="#64748b" stroke-width="1" stroke-opacity="0.6"/>
      <polyline points="${pingPts.join(" ")}" fill="none" stroke="#38bdf8" stroke-width="1.4"/>
      ${yTickLabels}
      ${xTickLabels}
    </svg>
  `;
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", "\"": "&quot;", "'": "&#39;"
  }[c]));
}
