# wows-lag-check

Detect potential server lag spikes/client stalls in World of Warships, and events that may lead to them.

## Build

```
nix develop
./build.sh
```

Without Nix: Rust stable with the `wasm32-unknown-unknown` target, `wasm-pack`, `wasm-bindgen-cli`, and the standalone `tailwindcss` v4 CLI on `PATH`.

## Smoke

```
cargo run --release --bin smoke -- <replay.wowsreplay> [<build-dir>]
```

## License

Dual-licensed under either:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
