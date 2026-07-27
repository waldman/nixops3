{ lib, ... }:
{
  systemd.services.nixops3 = {
    description = "NixOpS3 configuration daemon";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    wantedBy = [ "multi-user.target" ];

    # nixos-rebuild and nixos-generate-config live in the system PATH, which
    # systemd does not inherit. Extend it explicitly.
    environment = {
      PATH = lib.mkForce "/run/current-system/sw/bin:/run/wrappers/bin";
    };

    serviceConfig = {
      Type        = "simple";
      ExecStart   = "/usr/local/bin/nixops3 --daemon";
      Restart     = "on-failure";
      RestartSec  = "30s";
      RuntimeDirectory     = "nixops3 nixops3/secrets";
      RuntimeDirectoryMode = "0700";
      StateDirectory       = "nixops3";
      StateDirectoryMode   = "0755";
    };
  };

  environment.localBinInPath = true;
}
