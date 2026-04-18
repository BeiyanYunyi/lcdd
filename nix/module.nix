{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    literalExpression
    mkEnableOption
    mkIf
    mkOption
    types
    ;

  cfg = config.programs.lcdd;
  tomlFormat = pkgs.formats.toml { };

  generatedConfig =
    if cfg.config == null then null else tomlFormat.generate "lcdd-config.toml" cfg.config;

  effectiveConfigFile = if cfg.configFile != null then cfg.configFile else generatedConfig;
in
{
  options.programs.lcdd = {
    enable = mkEnableOption "the lcdd ASUS cooler LCD daemon";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.system}.default;
      defaultText = literalExpression "self.packages.\${pkgs.system}.default";
      description = "Package providing the lcdd executable.";
    };

    config = mkOption {
      type = types.nullOr tomlFormat.type;
      default = null;
      example = literalExpression ''
        {
          source.path = "/var/lib/lcdd/background.png";
          dashboard.slots = [
            {
              title = "CPU";
              subtitle = "usage";
              metric = "cpu_usage_percent";
            }
          ];
        }
      '';
      description = "Structured lcdd configuration rendered to TOML.";
    };

    configFile = mkOption {
      type = types.nullOr (types.either types.path types.str);
      default = null;
      example = literalExpression "/etc/lcdd/config.toml";
      description = "Existing lcdd config file passed to the daemon with --config.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = (cfg.config == null) != (cfg.configFile == null);
        message = "programs.lcdd: set exactly one of programs.lcdd.config or programs.lcdd.configFile.";
      }
    ];

    systemd.user.services.lcdd = mkIf (effectiveConfigFile != null) {
      Install.WantedBy = [ "default.target" ];

      Service = {
        ExecStart = "${cfg.package}/bin/lcdd --config ${effectiveConfigFile}";
        Restart = "on-failure";
        RestartSec = 2;
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectControlGroups = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
      };
    };
  };
}
