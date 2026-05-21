import init, { analyzeReplay } from "./pkg/wows_lag_check.js";

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
    const buf = await file.arrayBuffer();
    const t0 = performance.now();
    const result = analyzeReplay(new Uint8Array(buf));
    const ms = (performance.now() - t0).toFixed(0);
    setStatus("ok", `Parsed in ${ms} ms. ${result.samples_total} ping samples, ${result.server_ticks_total} server ticks, ${result.spikes.length} spikes.`);
    lastResult = result;
    renderResult(result);
  } catch (err) {
    console.error(err);
    setStatus("error", `Failed to parse: ${err.message ?? err}`);
  }
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
    spikeRows.innerHTML = r.spikes.map((s) => {
      const stall = s.client_present_during_gap
        ? `<span class="inline-flex items-center gap-1.5"><span class="size-2 rounded-full bg-orange-400"></span>server-only</span>`
        : `<span class="inline-flex items-center gap-1.5"><span class="size-2 rounded-full bg-violet-400"></span>client+server</span>`;
      return `<tr class="hover:bg-slate-800/50">
        <td class="py-2 pr-3 font-mono text-slate-300">${fmtClock(s.gap_start_clock)}</td>
        <td class="py-2 pr-3 font-mono text-slate-300">${fmtBattleClock(s.gap_start_clock, r.battle_start_clock_approx_s)}</td>
        <td class="py-2 pr-3 text-right font-mono ${s.gap_seconds > 2 ? "text-rose-300 font-semibold" : "text-slate-200"}">${(s.gap_seconds * 1000).toFixed(0)} ms</td>
        <td class="py-2 pr-3 text-right font-mono text-slate-200">${s.peak_ping_ms} ms</td>
        <td class="py-2 pr-3">${stall}</td>
        <td class="py-2 pr-3 text-right font-mono text-slate-400">${s.client_rate_hz.toFixed(1)} Hz</td>
      </tr>`;
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
    ["Arena ID",        m.arena_id ? `${m.arena_id} (0x${m.arena_id_hex})` : "(not found)"],
  ];
  metaList.innerHTML = metaRows.map(([k, v]) =>
    `<dt class="text-slate-500">${k}</dt><dd class="text-slate-200 font-mono break-all">${escapeHtml(v)}</dd>`
  ).join("");

  const ps = r.ping_stats;
  pingStats.innerHTML = [
    ["min",  `${ps.min_ms} ms`],
    ["mean", `${ps.mean_ms.toFixed(1)} ms`],
    ["p95",  `${ps.p95_ms} ms`],
    ["max",  `${ps.max_ms} ms`],
  ].map(([k, v]) =>
    `<dt class="text-slate-500">${k}</dt><dd class="text-slate-200 font-medium">${v}</dd>`
  ).join("");
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
    }
    body += `\`\`\`\n`;
  }

  body += `\n_Battle times assume a 30s pre-battle countdown. Generated by wows-lag-check._`;
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
