{ pkgs, ... }:
{
  networking.hostName = "test-01.example.com";

  users.users.example-2 = {
    isNormalUser = true;
    hashedPassword = "!";
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample2DummyPublicKeyNixOpS3Demo example-2@nixops3"
    ];
  };

  environment.systemPackages = with pkgs; [
    telnet
  ];
}
