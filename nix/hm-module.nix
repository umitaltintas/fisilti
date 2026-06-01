# Home-manager module for Fısıltı speech-to-text
#
# Provides a systemd user service for autostart.
# Usage: imports = [ fisilti.homeManagerModules.default ];
#        services.fisilti.enable = true;
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.fisilti;
in
{
  options.services.fisilti = {
    enable = lib.mkEnableOption "Fısıltı speech-to-text user service";

    package = lib.mkOption {
      type = lib.types.package;
      defaultText = lib.literalExpression "fisilti.packages.\${system}.fisilti";
      description = "The Fısıltı package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.fisilti = {
      Unit = {
        Description = "Fısıltı speech-to-text";
        After = [ "graphical-session.target" ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        # bin/handy matches the Cargo binary name (src-tauri/Cargo.toml).
        ExecStart = "${cfg.package}/bin/handy";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
