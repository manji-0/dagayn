{
  description = "dagayn development environment";

  inputs = {
    # A channel tarball and a git URL rather than `github:` so the inputs also
    # resolve where the GitHub API is unreachable (e.g. sandboxed agents).
    nixpkgs.url = "https://channels.nixos.org/nixos-unstable/nixexprs.tar.xz";
    rust-overlay = {
      url = "git+https://github.com/oxalica/rust-overlay?shallow=1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          # Same channel as rust-toolchain.toml, plus what `cargo llvm-cov`
          # and rust-analyzer need.
          rustToolchain = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
            extensions = [
              "clippy"
              "rustfmt"
              "rust-src"
              "llvm-tools-preview"
            ];
          };

          # .python-version pins 3.14. uv uses this interpreter instead of
          # downloading one, and PyO3 links its shared libpython for
          # `cargo test --workspace` / `cargo clippy --workspace`.
          python = pkgs.python314;
        in
        {
          default = pkgs.mkShell {
            packages = [
              # Python package and its PyO3 core
              python
              pkgs.uv
              rustToolchain
              pkgs.cargo-llvm-cov
              pkgs.pkg-config

              # dagayn-vscode: corepack supplies the pnpm pinned by
              # package.json's packageManager field.
              pkgs.nodejs_22
              pkgs.corepack_22

              # Git hooks and the jj workspace tests
              pkgs.prek
              pkgs.jujutsu
              pkgs.git
            ];

            UV_PYTHON = "${python}/bin/python3";
            UV_PYTHON_DOWNLOADS = "never";
            PYO3_PYTHON = "${python}/bin/python3";
            COREPACK_ENABLE_DOWNLOAD_PROMPT = "0";
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
