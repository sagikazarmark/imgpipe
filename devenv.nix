{
  pkgs,
  lib,
  config,
  multiverse,
  ...
}:

let
  # wasm-bindgen-test-runner must match the wasm-bindgen crate in Cargo.lock
  # exactly: from nixpkgs if it has that version, otherwise from
  # nixpkgs-multiverse.
  lock = builtins.fromTOML (builtins.readFile (config.devenv.root + "/Cargo.lock"));
  wasmBindgen =
    (lib.findFirst (p: p.name == "wasm-bindgen") (throw "no wasm-bindgen in Cargo.lock") lock.package)
    .version;
  versionedAttr = "wasm-bindgen-cli_${lib.replaceStrings [ "." ] [ "_" ] wasmBindgen}";
  wasm-bindgen-cli =
    if pkgs.wasm-bindgen-cli.version == wasmBindgen then
      pkgs.wasm-bindgen-cli
    else
      pkgs.${versionedAttr} or multiverse.wasm-bindgen-cli.${wasmBindgen};
in
{
  # The C codecs (`webp-libwebp`, `png-optimize`) for wasm32-unknown-unknown:
  # an unwrapped clang (the wrapped one only targets the host), llvm-ar, and
  # libc headers from wasi-libc. The few libc functions they call come from
  # imgpipe's `wasm-libc-shim` feature; threads are compiled out.
  env = {
    CC_wasm32_unknown_unknown = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
    AR_wasm32_unknown_unknown = "${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-ar";
    CFLAGS_wasm32_unknown_unknown = "-isystem ${pkgs.pkgsCross.wasi32.wasilibc.dev}/include/wasm32-wasip1 -O2 -UWEBP_USE_THREAD -U_THREAD_SAFE";
  };

  packages = with pkgs; [
    just

    # Runs the wasm32 tests (wasm-bindgen-test-runner).
    nodejs
    wasm-bindgen-cli

    # MozJPEG's x86 SIMD (`jpeg-mozjpeg`). Without it mozjpeg-sys still
    # builds, in plain C.
    nasm

    cargo-deny
    cargo-hack
    cargo-semver-checks
    cargo-release
  ];

  languages = {
    rust = {
      enable = true;
      channel = "stable";
      targets = [ "wasm32-unknown-unknown" ];
    };
  };

  profiles = {
    # The minimum supported Rust version (`rust-version` in Cargo.toml) in
    # place of the latest stable.
    msrv.module = {
      languages.rust.version = lib.versions.pad 3 (builtins.fromTOML (builtins.readFile ./Cargo.toml))
      .workspace.package.rust-version;
    };
  };
}
