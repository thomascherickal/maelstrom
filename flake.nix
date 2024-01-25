{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };

        craneLib = crane.lib.${system};
        all = craneLib.buildPackage {
          # NOTE: we need to force lld otherwise rust-lld is not found for wasm32 target
          CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER = "lld";

          pname = "all";
          src = craneLib.cleanCargoSource (craneLib.path ./.);
          strictDeps = true;

	  nativeBuildInputs = [
	    pkgs.pkg-config
	    pkgs.rustc-wasm32.llvmPackages.lld
	  ];

          buildInputs = [
	    pkgs.openssl
            # Add additional build inputs here
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            # Additional darwin specific inputs can be set here
            pkgs.libiconv
          ];

          # Additional environment variables can be set directly
          # MY_CUSTOM_VAR = "some value";
        };
      in
      {
        packages.default = all;

        devShells.default = craneLib.devShell {
          # Automatically inherit any build inputs from `my-crate`
          inputsFrom = [ all ];

          # Extra inputs (only used for interactive development)
          # can be added here; cargo and rustc are provided by default.
          packages = [
            pkgs.bat
            pkgs.cargo-audit
            pkgs.cargo-edit
            pkgs.cargo-nextest
            pkgs.cargo-watch
            pkgs.ripgrep
            pkgs.rust-analyzer
            pkgs.stgit
          ];

          CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER = "lld";
        };
      });
}
