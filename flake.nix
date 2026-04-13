{
  inputs = {
    nixpkgs-unpatched.url = "github:NixOS/nixpkgs/nixos-unstable-small";
    flake-utils.url = "github:numtide/flake-utils";

    tts-utils.url = "github:Discord-TTS/shared-workflows";
  };

  outputs =
    {
      nixpkgs-unpatched,
      flake-utils,
      tts-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgsUnpatched = import nixpkgs-unpatched { inherit system; };
        nixpkgs = pkgsUnpatched.applyPatches {
          name = "nixpkgs-patched";
          src = nixpkgs-unpatched;
          patches = [
            # Fixes espeak-ng with mbrola
            (pkgsUnpatched.fetchpatch2 {
              url = "https://github.com/NixOS/nixpkgs/pull/511135.patch";
              hash = "sha256-Y/KYc9ffXQ7wdNJuA86oXY3Fp2bMlmfVw21WUji66i4=";
            })
          ];
        };

        pkgs = import nixpkgs { inherit system; };
        lib = pkgs.lib;

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
              "serenity-0.12.5" = "sha256-xlBuX0qdTclrKaZkAwK3kXZdurxFT3UVYC7Eh/f+emA=";
              "songbird-0.5.0" = "sha256-zcCyn5XbP+rxJ/MT50vQUEJtYQ0sch2lYVmzQagIdIA=";
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
