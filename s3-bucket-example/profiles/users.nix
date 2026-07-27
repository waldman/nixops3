{ ... }:
{
  users.users.example = {
    isNormalUser = true;
    hashedPassword = "!";
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExampleDummyPublicKeyNixOpS3Demo example@nixops3"
    ];
  };
}
