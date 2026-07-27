{ ... }:
{
  imports = [
    <nixops3/profiles/users.nix>
    <nixops3/profiles/default_packages.nix>
  ];

  networking.hostName = "generic-node";
  services.openssh.enable = true;
}
