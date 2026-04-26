{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      flake-parts,
      rust-overlay,
      crane,
      ...
    }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      flake = {
        homeManagerModules = {
          default = import ./nix/module.nix { inherit self; };
          lcdd = import ./nix/module.nix { inherit self; };
        };
      };

      perSystem =
        { system, pkgs, ... }:
        let
          craneLib = (crane.mkLib pkgs).overrideToolchain (
            p: p.rust-bin.selectLatestNightlyWith (toolchain: toolchain.minimal)
          );
          buildInputs = with pkgs; [
            dejavu_fonts
            gcc
            pkg-config
            fontconfig
            freetype
            systemdLibs # udev is alias of systemdLibs in nixpkgs
            hidapi
            wayland
            libGL

            vulkan-loader
            vulkan-validation-layers
            vulkan-tools
          ];
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          devShells.default = pkgs.mkShell {
            name = "lcdd-dev-shell";
            inherit buildInputs;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath buildInputs;
          };
          packages.default =
            with pkgs;
            let
              assetFilter =
                path: _type:
                (builtins.match ".*/assets/test.jpg$" path != null)
                || (builtins.match ".*/src/image/dashboard/blur.wgsl$" path != null);
              assetOrCargo = path: type: (assetFilter path type) || (craneLib.filterCargoSources path type);
            in
            craneLib.buildPackage {
              src = lib.cleanSourceWith {
                src = ./.;
                filter = assetOrCargo;
                name = "source";
              };
              # Add extra inputs here or any other derivation settings
              # doCheck = true;
              inherit buildInputs;
              nativeBuildInputs = [ makeBinaryWrapper ];
              postInstall = ''
                wrapProgram $out/bin/lcdd --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath buildInputs}
              '';
              CI = "true";
              meta.mainProgram = "lcdd";
            };
        };
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
    };
}
