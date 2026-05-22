# wows-lag-check

Static web app that finds ping spikes and server stalls in World of Warships
replay files. Drop a `.wowsreplay`, get a severity badge, a spike table, and a
Discord-pasteable summary including the arena ID. Everything runs locally in
the browser via WASM.

## What it detects

The replay's packet stream is parsed and three things drive the analysis:

- `ServerTick` (0x0e) clocks. Gaps of 500 ms or more between consecutive ticks
  are flagged as spikes. Normal cadence is ~7 Hz; every server-driven packet
  rides on that tick.
- `PlayerNetStats` (0x1d) samples. ~10 Hz reports of `ping`, `fps`, and an
  `is_lagging` flag from the client.
- `onArenaStateReceived`, decoded for the 64-bit `arena_id`, which WG can use
  to look up the server-side replay.

Each spike is classified as either:

- **server-only**: client-only packets (Camera/GunMarker/PlayerNetStats, each
  on a 10 Hz client timer, ~30 Hz combined) kept landing throughout the gap.
  The server stopped sending updates while the client kept running.
- **client+server**: the gap is silent of client packets too. Either the client
  itself froze, or a long server stall blocked the network thread enough to
  stop the client from ticking.

The server/region (EU, NA, ASIA, RU, SG, CIS) is recovered by scanning the
decrypted packet stream for pickle SHORT_BINSTRING tokens matching known realm
codes.

## Entity definitions

The replay packet format needs the game's entity definitions to fully decode
entity packets (and therefore the arena ID). Those are version-specific, so
they're loaded at runtime: the app reads the replay's build number, then
fetches that build's entity-def tree from the
[`landaire/wows-replay-data`](https://github.com/landaire/wows-replay-data)
repository (resolving the repo's content-addressed `vfs_common/` symlinks).

If the build isn't in that repo, the app falls back to a spec-free parse:
spike detection still works, but the arena ID is unavailable.

The parser is the upstream [`wows-replays`][wr] crate (sans-io / wasm build);
`wowsunpack` handles entity-def parsing.

[wr]: https://github.com/landaire/wows-toolkit

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
cargo run --release --bin smoke -- <replay.wowsreplay> [<build-dir>]
```

Prints the spike list and severity to stdout. `<build-dir>` is an optional
wows-replay-data build directory (e.g. `15.4.0_12506899`); when given, entity
defs are loaded from its `vfs/scripts/` tree and the replay is parsed through
the full parser.

## Deploy

The `.github/workflows/deploy.yml` workflow runs on every push to `main`,
builds the static site via Nix, and uploads `dist/` to GitHub Pages.

To enable on a fork: **Settings -> Pages -> Source -> GitHub Actions**, then
push. First deploy publishes to `https://<user>.github.io/wows-lag-check/`.

## Layout

```
src/
  lib.rs        wasm-bindgen entry: replayInfo, analyzeReplay
  replay.rs     build/version parsing, realm scan, entity-def bundle unpacking
  analysis.rs   spike detection, severity classification, JSON shape
  bin/smoke.rs  native CLI mirror of the WASM
web/
  index.html    page shell
  app.js        file drop, entity-def fetch, WASM, SVG chart, spike table
  style.css     Tailwind v4 input
build.sh        wasm-pack + tailwindcss + cp into dist/
flake.nix       devShell + nix run .#build
.github/workflows/deploy.yml
```
