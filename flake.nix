{
  description = "swaybeam - Miracast source for wlroots-based compositors";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        gstRuntimePlugins = with pkgs.gst_all_1; [
          gstreamer.out
          gst-plugins-base
          gst-plugins-good
          gst-plugins-bad
          gst-plugins-ugly
          gst-libav
          gst-vaapi
        ];

        src = craneLib.cleanCargoSource ./.;

        cargoVendorDir = craneLib.vendorCargoDeps {
          inherit src;
          overrideVendorCargoPackage = p: drv:
            if p.name == "libspa" && p.version == "0.10.0" then
              drv.overrideAttrs (_old: {
                postPatch = ''
                  substituteInPlace src/constants.rs \
                    --replace-fail 'spa_sys::SPA_ID_INVALID' '0xffffffff'
                '';
              })
            else if p.name == "pipewire" && p.version == "0.10.0" then
              drv.overrideAttrs (_old: {
                postPatch = ''
                  substituteInPlace src/constants.rs \
                    --replace-fail 'pw_sys::PW_ID_ANY' '0xffffffff'
                '';
              })
            else
              drv;
        };

        commonArgs = {
          pname = "swaybeam";
          inherit src cargoVendorDir;
          doCheck = false;
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper llvmPackages.clang llvmPackages.libclang ];
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          buildInputs = with pkgs; [
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            pipewire
          ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        swaybeam = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          doCheck = false;
          postInstall = let
            gstPluginPath = pkgs.lib.makeSearchPath "lib/gstreamer-1.0" gstRuntimePlugins;
            gstBin = "${pkgs.gst_all_1.gstreamer}/bin";
          in ''
            wrapProgram $out/bin/swaybeam \
              --set GST_PLUGIN_SYSTEM_PATH_1_0 "${gstPluginPath}" \
              --prefix PATH : "${gstBin}"
            wrapProgram $out/bin/validate-rtsp \
              --set GST_PLUGIN_SYSTEM_PATH_1_0 "${gstPluginPath}" \
              --prefix PATH : "${gstBin}"
            wrapProgram $out/bin/validate-wfd \
              --set GST_PLUGIN_SYSTEM_PATH_1_0 "${gstPluginPath}" \
              --prefix PATH : "${gstBin}"
            wrapProgram $out/bin/diagnose-session \
              --set GST_PLUGIN_SYSTEM_PATH_1_0 "${gstPluginPath}" \
              --prefix PATH : "${gstBin}"
          '';
          meta = with pkgs.lib; {
            description = "Miracast source for wlroots-based compositors";
            homepage = "https://github.com/forkline/swaybeam";
            license = licenses.mit;
            platforms = platforms.linux;
          };
        });
      in {
        packages.default = swaybeam;
        packages.swaybeam = swaybeam;
        devShells.default = craneLib.devShell {
          packages = with pkgs; [
            rustToolchain
            pkg-config
            llvmPackages.libclang
            just
            rust-analyzer
          ];
          buildInputs = with pkgs; [
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            gst_all_1.gst-plugins-good
            gst_all_1.gst-plugins-bad
            gst_all_1.gst-plugins-ugly
            gst_all_1.gst-libav
            gst_all_1.gst-vaapi
            pipewire
            wireplumber
            networkmanager
            wpa_supplicant
            xdg-desktop-portal-wlr
          ];
        };
      });
}
