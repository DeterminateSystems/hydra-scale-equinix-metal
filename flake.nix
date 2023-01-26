{
  description = "scale";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs =
    { self
    , nixpkgs
    , ...
    } @ inputs:
    let
      nameValuePair = name: value: { inherit name value; };
      genAttrs = names: f: builtins.listToAttrs (map (n: nameValuePair n (f n)) names);
      allSystems = [ "x86_64-linux" "aarch64-linux" "i686-linux" "x86_64-darwin" ];

      forAllSystems = f: genAttrs allSystems (system: f {
        inherit system;
        pkgs = import nixpkgs { inherit system; };
      });
    in
    {
      devShell = forAllSystems ({ system, pkgs, ... }: self.packages.${system}.package.overrideAttrs ({ nativeBuildInputs ? [ ], ... }: {
        nativeBuildInputs = nativeBuildInputs ++ (with pkgs; [
          entr
          rustfmt
          clippy
          cargo
          jq
        ]);
      }));

      packages = forAllSystems
        ({ system, pkgs, ... }: {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "scale";
            version = "0.0.0";

            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;
          };
        });

      nixosModules.default = ({ config, lib, pkgs, ... }:
        let
          cfg = config.services.hsem;
          categoriesFileFormat = pkgs.formats.json { };
        in
        {
          options.services.hsem = {
            enable = lib.mkEnableOption "hydra-scale-equinix-metal";

            secretFile = lib.mkOption {
              type = lib.types.str;
              description = lib.mdDoc ''
                The path to an environment file that contains METAL_AUTH_TOKEN
                and METAL_PROJECT_ID.
              '';
            };

            tags = lib.mkOption {
              type = with lib.types; listOf str;
            };

            facilities = lib.mkOption {
              type = with lib.types; listOf str;
            };

            hydraRoot = lib.mkOption {
              type = with lib.types; nullOr str;
              default = null;
            };

            prometheusRoot = lib.mkOption {
              type = with lib.types; nullOr str;
              default = null;
            };

            categories = lib.mkOption {
              inherit (categoriesFileFormat) type;
            };
          };

          config = lib.mkIf cfg.enable {
            systemd.services.hsem = {
              wantedBy = [ "default.target" ];
              after = [ "network.target" ];

              script = ''
                export $(xargs < ${cfg.secretFile})

                ${self.packages.default}/bin/scale \
                  --tags ${lib.concatStringsSep "," cfg.tags} \
                  --facilities ${lib.concatStringsSep "," cfg.facilities} \
                  ${lib.optionalString (cfg.hydraRoot != null) "--hydra-root ${cfg.hydraRoot}"} \
                  ${lib.optionalString (cfg.prometheusRoot != null) "--prometheus-root ${cfg.prometheusRoot}"} \
                  --categories-file ${categoriesFileFormat.generate "categories.json" cfg.categories}
              '';
            };
          };
        });
    };
}
