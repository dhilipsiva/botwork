# Botwork's development shells, so the checks run without rustup (on NixOS, for instance).
#
#   nix develop            # the pinned stable Rust, just, Python, Node, cargo-llvm-cov, cargo-mutants
#   nix develop .#nightly  # the pinned nightly and cargo-fuzz, for `just fuzz-smoke` and `just public-api`
#
# Every version here equals the one CI and the campaign scripts use: stable is Cargo.toml's
# `rust-version`, the stable of this release (D20, docs/decisions.md); the nightly is the one
# .github/workflows/ci.yml and scripts/fuzz_smoke.py name; and the tools are the ones
# scripts/coverage.py, scripts/mutation_core.py, and scripts/fuzz_smoke.py check for.
# tests/dev_shell.rs keeps them equal.
{
  description = "Botwork development shells: pinned stable and nightly Rust and the campaign tools";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Cargo.toml's rust-version (D20); they move together, at a release.
        rustVersion = "1.99.1";
        # The nightly of the public API job and the fuzz campaign.
        nightlyDate = "2026-08-14";

        stable = pkgs.rust-bin.stable.${rustVersion}.default.override {
          # llvm-tools: scripts/coverage.py reads llvm-cov and llvm-profdata from the sysroot.
          extensions = [
            "rust-src"
            "clippy"
            "rustfmt"
            "llvm-tools-preview"
          ];
          # The WebAssembly test guest (scripts/wasm_guest.py).
          targets = [ "wasm32-wasip2" ];
        };
        nightly = pkgs.rust-bin.nightly.${nightlyDate}.minimal;

        # scripts/coverage.py records cargo-llvm-cov 0.9.1; nixpkgs has 0.9.0.
        cargo-llvm-cov = pkgs.rustPlatform.buildRustPackage rec {
          pname = "cargo-llvm-cov";
          version = "0.9.1";
          src = pkgs.fetchCrate {
            inherit pname version;
            hash = "sha256-GajnapkpjlSaKrs7SYl3NvXzfLfQvDEZtDuregBQqBw=";
          };
          cargoHash = "sha256-uSMRu+OjdEyxYyGc65Z5vYOcHyOcgf1L3Kxc4QHRM2o=";
          # Its tests need a rustup toolchain with llvm-tools.
          doCheck = false;
        };

        # nixpkgs' builds of the other two campaign tools, asserted so that `nix flake update`
        # cannot move them.
        cargo-mutants =
          assert pkgs.cargo-mutants.version == "27.1.0";
          pkgs.cargo-mutants;
        cargo-fuzz =
          assert pkgs.cargo-fuzz.version == "0.13.2";
          pkgs.cargo-fuzz;

        common = [
          pkgs.just
          pkgs.python3
          pkgs.git
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          name = "botwork";
          packages = common ++ [
            stable
            # The JavaScript adapter's tests and the VS Code extension; CI uses Node 22.
            pkgs.nodejs_22
            cargo-llvm-cov
            cargo-mutants
          ];
        };

        devShells.nightly = pkgs.mkShell {
          name = "botwork-nightly";
          packages = common ++ [
            nightly
            cargo-fuzz
          ];
        };
      }
    );
}
