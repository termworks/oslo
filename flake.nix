{
  description = "oslo shell and Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs?rev=4c1018dae018162ec878d42fec712642d214fdfa";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    { nixpkgs, flake-utils, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = manifest.workspace.package.version;
      fullFeatures = builtins.filter (name: name != "default") (
        builtins.attrNames manifest.features
      );
    in
    flake-utils.lib.eachSystem systems (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        lib = pkgs.lib;
        rustTarget =
          {
            x86_64-linux = "x86_64-unknown-linux-musl";
            aarch64-linux = "aarch64-unknown-linux-musl";
          }
          .${system};
        # **The toolchain this repository builds with, pinned here and nowhere else.**
        #
        # There used to be a `rust-toolchain.toml` beside `flake.nix` as well, and it was a third
        # copy of this number that nothing checked: the flake did not read it, the workflows
        # hardcoded their own, and its only real effect was to override whatever CI had just
        # installed — which the `msrv` and `fuzz` jobs then had to fight with `RUSTUP_TOOLCHAIN`.
        # The devshell is how this repository is built, so the devshell is where the version lives.
        #
        # The CI jobs pin the same version by hand in `.github/workflows/*.yml`; bump them together.
        # The MSRV is separate and lower — `rust-version` in `Cargo.toml` says 1.90, and the `msrv`
        # job installs exactly that.
        rustChannel = "1.94.0";
        toolchain = pkgs.rust-bin.stable.${rustChannel}.default.override {
          targets = [ rustTarget ];
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
        };
        mkOslo =
          { minimal ? false }:
          let
            binaryName = if minimal then "oslo-minimal" else "oslo";
          in
          pkgs.pkgsStatic.rustPlatform.buildRustPackage {
            pname = binaryName;
            inherit version;
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes = {
                "luna-0.5.0" = "sha256-YJsEKm7UykhqNQjhLPBgpmCHQZQhDLFVAENSD43SyAw=";
                "rune-0.1.5" = "sha256-03yaB4bv+iXYGVStpGDCNdJQzYh/oiB8hRFrIKuMfvU=";
                "tagdata-0.1.4" = "sha256-L/Fa0gS4hTrbaWvuvBB4NJ85xud8ixv7sSalLKRINQU=";
                "vista-recall-0.1.2" = "sha256-Zi3pKV0pZNZh/XM8dE+uhHIJuieT977D0bAD91/qrhY=";
              };
            };
            buildNoDefaultFeatures = true;
            buildFeatures = lib.optionals (!minimal) fullFeatures;
            cargoBuildFlags = [
              "--bin"
              "oslo"
            ];
            stripAllList = [ "bin" ];

            doCheck = false;
            doInstallCheck = true;
            nativeInstallCheckInputs = [ pkgs.binutils ];
            postInstall = lib.optionalString minimal ''
              mv "$out/bin/oslo" "$out/bin/${binaryName}"
            '';
            installCheckPhase = ''
              runHook preInstallCheck

              binary="$out/bin/${binaryName}"
              test "$("$binary" --version)" = "oslo version ${version}"
              test "$("$binary" -c 'printf ok')" = "ok"

              if readelf -l "$binary" | grep -q 'program interpreter'; then
                echo "error: $binary requests a dynamic loader" >&2
                exit 1
              fi
              if readelf -d "$binary" 2>/dev/null | grep -q NEEDED; then
                echo "error: $binary has dynamic dependencies" >&2
                exit 1
              fi

              runHook postInstallCheck
            '';

            meta = {
              description = "POSIX shell in Rust with an embedded Lua runtime";
              homepage = "https://github.com/termworks/oslo";
              license = lib.licenses.mit;
              mainProgram = binaryName;
              platforms = lib.platforms.linux;
            };
          };
        oslo = mkOslo { };
        osloMinimal = mkOslo { minimal = true; };
        mkApp = package: binaryName: {
          type = "app";
          program = "${package}/bin/${binaryName}";
          meta.description = "Run ${binaryName}";
        };
      in
      {
        packages = {
          inherit oslo;
          "oslo-minimal" = osloMinimal;
          default = oslo;
        };

        apps = {
          default = mkApp oslo "oslo";
          oslo = mkApp oslo "oslo";
          "oslo-minimal" = mkApp osloMinimal "oslo-minimal";
        };

        checks = {
          inherit oslo;
          "oslo-minimal" = osloMinimal;
        };

        devShells.default = pkgs.mkShell {
          packages = [
            toolchain
            pkgs.binutils
            pkgs.git
            pkgs.git-cliff
            pkgs.gnumake
          ];
        };
      }
    );
}
