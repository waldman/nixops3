{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    mc
    vim
  ];
}
