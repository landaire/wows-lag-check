{
  description = "Static web app that finds ping spikes in WoWs replays";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ];
        };

        # Need Tailwind v4 syntax. tailwindcss_4 if available, else fall through.
        tailwind = pkgs.tailwindcss_4 or pkgs.tailwindcss;

        commonTools = [
          rustToolchain
          pkgs.wasm-pack
          pkgs.wasm-bindgen-cli
          pkgs.binaryen
          tailwind
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          packages = commonTools ++ [
            pkgs.git
            pkgs.python3
          ];

          shellHook = ''
            echo "wows-lag-check dev shell"
            echo "  nix run .#build  - build dist/"
            echo "  nix run .#serve  - serve dist/ at http://localhost:8080"
          '';
        };

        # nix run .#build runs ./build.sh with the right PATH. Output lands in
        # ./dist rather than the nix store so cargo can pull from crates.io.
        apps.build = {
          type = "app";
          program = toString (pkgs.writeShellScript "wows-lag-check-build" ''
            export PATH=${pkgs.lib.makeBinPath commonTools}:$PATH
            exec bash ./build.sh
          '');
        };

        # nix run .#serve [port]  - serve ./dist locally after a build.
        apps.serve = {
          type = "app";
          program = toString (pkgs.writeShellScript "wows-lag-check-serve" ''
            PORT=''${1:-8080}
            if [ ! -f dist/index.html ]; then
              echo "dist/ not built yet. Run ./build.sh or nix run .#build first." >&2
              exit 1
            fi
            echo "Serving dist/ at http://localhost:$PORT"
            exec ${pkgs.python3}/bin/python3 -m http.server "$PORT" -d dist
          '');
        };
        apps.default = self.apps.${system}.serve;

        formatter = pkgs.nixpkgs-fmt;
      });
}
