# wows-lag-check

Static web app that finds ping spikes and server stalls in World of Warships
replay files. Drop a `.wowsreplay`, get a severity badge, a spike table, and a
Discord-pasteable summary including the arena ID. Everything runs locally in
the browser via WASM.

## What it detects

The parser walks the decrypted packet stream and looks at three things:

- `ServerTick` (0x0e) clocks. Gaps of 500 ms or more between consecutive ticks
  are flagged as spikes. Normal cadence is ~7 Hz.
- `PlayerNetStats` (0x1d) samples. ~10 Hz reports of `ping`, `fps`, and a
  `is_lagging` flag from the client.
- `Map` (0x28) packet. Source of the 64-bit `arena_id` for cross-referencing
  with server-side replays.

Each spike is classified as either:

- **server-only**: client-only packets (Camera/GunMarker/PlayerNetStats, ~30 Hz
  combined) kept landing in the replay throughout the gap. The server stopped
  sending updates while the client kept running.
- **client+server**: the gap is silent of client packets too. Either the client
  itself froze, or a long server stall blocked the network thread enough to
  stop the client from ticking.

The server/region (EU, NA, ASIA, RU, SG, CIS) is recovered by scanning the
decrypted packet stream for pickle SHORT_BINSTRING tokens matching known realm
codes. The `realm` key name is memoized so it doesn't recur, but the value
appears once per player record.

The parser is a stripped-down vendored subset of the `wows-replays` crate
(https://github.com/landaire/wows-toolkit). Decoding ServerTick + PlayerNetStats
+ Map doesn't need entity definitions, so this is version-independent and
ships no game assets.

## Build

With Nix:

```
nix develop
./build.sh
(cd dist && python3 -m http.server 8080)
```

Without Nix you need: Rust stable with the `wasm32-unknown-unknown` target,
`wasm-pack`, `wasm-bindgen-cli`, and the standalone `tailwindcss` v4 CLI on
`PATH`.

## CLI sanity check

```
cargo run --release --bin smoke -- /path/to/replay.wowsreplay
```

Prints the spike list and severity to stdout. Useful when iterating on the
analyzer without touching the browser.

## Deploy

The `.github/workflows/deploy.yml` workflow runs on every push to `main`,
builds the static site via Nix, and uploads `dist/` to GitHub Pages.

To enable on a fork: **Settings -> Pages -> Source -> GitHub Actions**, then
push. First deploy publishes to `https://<user>.github.io/wows-lag-check/`.

## Layout

```
src/
  lib.rs        wasm-bindgen entry: analyzeReplay(bytes)
  replay.rs     blowfish decrypt, zlib decompress, packet walker, realm scan
  analysis.rs   spike detection, severity classification, JSON shape
  bin/smoke.rs  native CLI mirror of the WASM
web/
  index.html    page shell
  app.js        file drop, WASM init, SVG chart, spike table, copy button
  style.css     Tailwind v4 input
build.sh        wasm-pack + tailwindcss + cp into dist/
flake.nix       devShell + nix run .#build
.github/workflows/deploy.yml
```
