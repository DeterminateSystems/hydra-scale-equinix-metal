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
      allSystems = [ "x86_64-linux" "aarch64-linux" "i686-linux" "x86_64-darwin" "aarch64-darwin" ];

      forAllSystems = f: genAttrs allSystems (system: f {
        inherit system;
        pkgs = import nixpkgs { inherit system; };
      });
    in
    {
      devShells = forAllSystems ({ system, pkgs, ... }: {
        default = self.packages.${system}.default.overrideAttrs ({ nativeBuildInputs ? [ ], ... }: {
          nativeBuildInputs = nativeBuildInputs ++ (with pkgs; [
            entr
            rustfmt
            clippy
            cargo
            jq
          ]);
        });
      });

      packages = forAllSystems
        ({ system, pkgs, ... }: {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "scale";
            version = "0.0.0";

            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            buildInputs = pkgs.lib.optionals (pkgs.stdenv.isDarwin) (with pkgs; [
              darwin.apple_sdk.frameworks.SystemConfiguration
            ]);
          };
        });

      nixosModules.default = ({ config, lib, pkgs, ... }:
        let
          cfg = config.services.hydra-scale-equinix-metal;
          configFileFormat = pkgs.formats.json { };
        in
        {
          options.services.hydra-scale-equinix-metal = {
            enable = lib.mkEnableOption "hydra-scale-equinix-metal";

            secretFile = lib.mkOption {
              type = lib.types.str;
              description = lib.mdDoc ''
                The path to an environment file that contains METAL_AUTH_TOKEN
                and METAL_PROJECT_ID.
              '';
            };

            hydraRoot = lib.mkOption {
              type = with lib.types; nullOr str;
              default = null;
            };

            prometheusRoot = lib.mkOption {
              type = with lib.types; nullOr str;
              default = null;
            };

            config = lib.mkOption {
              type = lib.types.submodule {
                options.metro = lib.mkOption {
                  type = lib.types.str;
                  description = lib.mdDoc ''
                    Metro code or ID of where the instance should be provisioned in.
                  '';
                  example = "any";
                };
                options.tags = lib.mkOption {
                  type = with lib.types; listOf str;
                  description = lib.mdDoc ''
                    The tags of the instances created.
                  '';
                };
                options.categories = lib.mkOption {
                  inherit (configFileFormat) type;
                };
              };
            };

            interval = lib.mkOption {
              type = with lib.types; listOf str;
              default = ["hourly"];
              description = lib.mdDoc ''
                The intervals at which to run (see `man systemd.time` for the format).
              '';
            };
          };

          config = lib.mkIf cfg.enable {
            systemd.services.hydra-scale-equinix-metal = {
              wantedBy = [ "default.target" ];
              after = [ "network.target" ];

              startAt = cfg.interval;
              serviceConfig = {
                EnvironmentFile = cfg.secretFile;
                DynamicUser = true;
                ExecStart = ''
                  ${self.packages.${pkgs.stdenv.system}.default}/bin/scale \
                    ${lib.optionalString (cfg.hydraRoot != null) "--hydra-root ${cfg.hydraRoot}"} \
                    ${lib.optionalString (cfg.prometheusRoot != null) "--prometheus-root ${cfg.prometheusRoot}"} \
                    --config-file ${configFileFormat.generate "config.json" cfg.config}
                '';
              };
            };
          };
        });
    };
}
