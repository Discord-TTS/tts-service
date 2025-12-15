{

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable-small";
    flake-utils.url = "github:numtide/flake-utils";

    tts-utils.url = "github:Discord-TTS/shared-workflows";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      tts-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        lib = nixpkgs.lib;
        pkgs = import nixpkgs { inherit system; };

        espeak-ng = pkgs.espeak-ng.override {
          pcaudiolibSupport = false;
        };

        pkgDesc = (lib.importTOML ./Cargo.toml).package;
        ttsServicePkg = pkgs.rustPlatform.buildRustPackage {
          pname = pkgDesc.name;
          version = pkgDesc.version;
          meta.mainProgram = pkgDesc.name;

          src = lib.sources.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.makeWrapper ];
          postInstall = ''
            wrapProgram $out/bin/tts-service \
              --set PATH ${lib.makeBinPath ([ espeak-ng ])} \
              --set MBROLA_VOICES_BASE_PATH ${pkgs.mbrola-voices}/data
          '';
        };
      in
      tts-utils.mkTTSModule {
        inherit pkgs;
        package = ttsServicePkg;
        extraDockerContents = [ pkgs.dockerTools.caCertificates ];
      }
    );
}
