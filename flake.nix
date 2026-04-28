{
  description = "gearbox Rust vehicle/robot simulator development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    # nixGL wraps a command with the host's GPU drivers so OpenGL / Vulkan
    # apps (e.g. Bevy via wgpu) work inside a nix devShell on non-NixOS hosts.
    nixgl.url = "github:nix-community/nixGL";
  };

  outputs =
    { self, nixpkgs, rust-overlay, flake-utils, nixgl, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];

        pkgs = import nixpkgs {
          inherit system overlays;
          config = {
            allowUnfree = true;
            nvidia.acceptLicense = true;
          };
        };

        # .envrc exports GEARBOX_NVIDIA_VERSION from /proc/driver/nvidia/version
        # before direnv loads the flake. We read it via getEnv (works because
        # --impure is wired into .envrc). Reading from a file under .direnv/
        # doesn't work: flakes in a git repo only expose git-tracked files to
        # the evaluator, and .direnv/ is globally gitignored.
        nvidiaVersion = let v = builtins.getEnv "GEARBOX_NVIDIA_VERSION";
        in if v != "" then v
           else throw "gearbox: GEARBOX_NVIDIA_VERSION is unset — is direnv loaded and is the NVIDIA driver running?";

        # Build nixGL pinned to the detected version. `nvidiaHash = null`
        # makes it fetch the matching .run impurely (--impure is wired into
        # .envrc), so this stays automatic as the host driver changes.
        # Note: nixGL still refs xorg.libX11/libxcb/libxshmfence internally,
        # which prints deprecation warnings during eval — upstream bug.
        nixglPkgs = import "${nixgl}/default.nix" {
          inherit pkgs nvidiaVersion;
          nvidiaHash = null;
        };

        # Stable, unversioned `nixGL` / `nixVulkan` aliases — the underlying
        # binaries have the detected driver version baked into their names.
        nixGLAlias = pkgs.runCommand "nixGL" { } ''
          mkdir -p $out/bin
          ln -s ${nixglPkgs.nixGLNvidia}/bin/nixGLNvidia-${nvidiaVersion} $out/bin/nixGL
        '';
        nixVulkanAlias = pkgs.runCommand "nixVulkan" { } ''
          mkdir -p $out/bin
          ln -s ${nixglPkgs.nixVulkanNvidia}/bin/nixVulkanNvidia-${nvidiaVersion} $out/bin/nixVulkan
        '';

        # Runtime libs Bevy needs on Linux (audio, input, windowing, GPU).
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
            })
            pkgs.clang
            pkgs.mold
            pkgs.pkg-config

            # GPU wrappers.
            nixGLAlias
            nixVulkanAlias
            nixglPkgs.nixGLNvidia
            nixglPkgs.nixVulkanNvidia
            nixglPkgs.nixGLIntel      # Mesa fallback (AMD / Intel iGPU)
            nixglPkgs.nixVulkanIntel

            # Pixar's OpenUSD reference toolchain — `usdview`,
            # `usdrecord`, `usdcat`, etc. Lets us cross-check the
            # Rust viewer's output against Pixar's renderer when an
            # asset doesn't render the way the author intended.
            pkgs.openusd
          ] ++ bevyLibs;

          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath bevyLibs;
          # Silence wgpu's validation-layer spam. wgpu's generated SPIR-V
          # uses relaxed atomic ordering that Vulkan 1.3 validation
          # rejects — naga/wgpu upstream bug, harmless at runtime.
          WGPU_VALIDATION = "0";
          WGPU_DEBUG = "0";
        };
      }
    );
}
