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
        lib = pkgs.lib;
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            (final: prev: {
              espeak-ng = prev.espeak-ng.override {
                klattSupport = false;
                pcaudiolibSupport = false;
                speechPlayerSupport = false;
              };
            })
          ];
        };

        pkgDesc = (lib.importTOML ./Cargo.toml).package;
        pkgPath = lib.makeBinPath [
          pkgs.espeak-ng
          pkgs.mbrola
        ];
        ttsServicePkg = pkgs.rustPlatform.buildRustPackage {
          pname = pkgDesc.name;
          version = pkgDesc.version;
          meta.mainProgram = pkgDesc.name;

          src = lib.sources.cleanSource ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "serenity-0.12.5" = "sha256-7MuZU8e7cCPnv9WgS5HWU93tT1HOBvlLOI1H2ypmu+o=";
              "songbird-0.6.1" = "sha256-hOfSZ3OT9jB40rVvRcgl0OdLD3k0fOrpUxhWo4aSc3c=";
            };
          };

          nativeBuildInputs = with pkgs; [
            makeWrapper
            cmake
          ];
          patchPhase = ''
            substituteInPlace src/modes/espeak.rs \
              --replace-fail /usr/share/mbrola ${pkgs.mbrola-voices} \
              --replace-fail /usr/local/share/espeak-ng-data ${pkgs.espeak-ng}/share/espeak-ng-data
          '';
          postInstall =
            let
              mbrolaDataDir = "${pkgs.mbrola-voices}/data";
              mbrolaXDGDir = pkgs.runCommand "mbrola-xdg-dir" { } ''
                mkdir -p $out/mbrola
                for voiceDir in ${mbrolaDataDir}/*; do
                  voiceName=$(basename $voiceDir)
                  ln -s ${mbrolaDataDir}/$voiceName/$voiceName $out/mbrola/$voiceName
                done
              '';
            in
            ''
              wrapProgram $out/bin/tts-service \
                --prefix PATH : ${pkgPath} \
                --prefix XDG_DATA_DIRS : ${mbrolaXDGDir}
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
