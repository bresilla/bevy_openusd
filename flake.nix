{
  description = "bevy_mara — reusable glass-themed Bevy + egui editor UI kit, development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    nixgl.url = "github:nix-community/nixGL";
  };

  outputs =
    { self, nixpkgs, rust-overlay, flake-utils, nixgl, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [
          (final: prev: {
            xorg = prev.xorg // {
              libX11 = final.libx11;
              libxcb = final.libxcb;
              libxshmfence = final.libxshmfence;
            };
          })
          (import rust-overlay)
        ];

        pkgs = import nixpkgs {
          inherit system overlays;
          config = {
            allowUnfree = true;
            nvidia.acceptLicense = true;
          };
        };

        nvidiaVersion = let v = builtins.getEnv "NVIDIA_VERSION";
        in if v != "" then v
           else throw "bevy_mara: NVIDIA_VERSION is unset — is direnv loaded and is the NVIDIA driver running?";

        nixglPkgs = import "${nixgl}/default.nix" {
          inherit pkgs nvidiaVersion;
          nvidiaHash = null;
        };

        nixGLAlias = pkgs.runCommand "nixGL" { } ''
          mkdir -p $out/bin
          ln -s ${nixglPkgs.nixGLNvidia}/bin/nixGLNvidia-${nvidiaVersion} $out/bin/nixGL
        '';
        nixVulkanAlias = pkgs.runCommand "nixVulkan" { } ''
          mkdir -p $out/bin
          ln -s ${nixglPkgs.nixVulkanNvidia}/bin/nixVulkanNvidia-${nvidiaVersion} $out/bin/nixVulkan
        '';

        bevyLibs = with pkgs; [
          alsa-lib
          udev
          vulkan-loader
          libxkbcommon
          wayland
          libx11
          libxcursor
          libxi
          libxrandr
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            (pkgs.rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rustfmt" "clippy" ];
              targets = [ "wasm32-unknown-unknown" ];
            })
            pkgs.clang
            pkgs.mold
            pkgs.pkg-config
            pkgs.trunk
            pkgs.openusd

            (pkgs.python3.withPackages (ps: with ps; [ fonttools brotli ]))

            nixGLAlias
            nixVulkanAlias
            nixglPkgs.nixGLNvidia
            nixglPkgs.nixVulkanNvidia
            nixglPkgs.nixGLIntel
            nixglPkgs.nixVulkanIntel
          ] ++ bevyLibs;

          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath bevyLibs;
          WGPU_VALIDATION = "0";
          WGPU_DEBUG = "0";
        };
      }
    );
}
