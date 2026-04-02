{
  description = "GreenCell UPS monitor and control tool";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11";
  };

  outputs =
    { self, nixpkgs }:
    let
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs [
          "x86_64-linux"
          "aarch64-linux"
        ] (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "gcups";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.libusb1 ];
          meta = {
            description = "GreenCell UPS monitor — reads battery and charging status via USB HID";
            homepage = "https://github.com/zommiommy/gcups-rs";
          };
        };
      });

      overlays.default = final: prev: {
        gcups = self.packages.${final.system}.default;
      };
    };
}
