# Source and asset attribution

- **Ara 3D Gen**, Josh Ola and Ara 3D Gen contributors:
  https://github.com/olatunde85/3d-gen, revision
  `a4947ffdd14acf2ff018cb3568c941595f86d86e`.
  MIT source retained under `vendor/ara`, including its original LICENSE.
  Native macro morphing, fitting, anatomy, skeleton/skin algorithms and export
  conventions are ports/adaptations. Procedural meshes and paint masks were
  converted offline to reusable native assets. Runtime selection, fitting,
  material customization, skinning, playback and export are Rust.
- **MakeHuman Community** graphical assets: source revision
  `a8bc2d54ff0ac92e78ff71431b1023eda42bf482`, plus the official CC0 system asset
  archive. Base mesh, targets, proxies and textures are CC0 graphical data.
  The upstream application has a separate code license; no MakeHuman application
  code was copied. See `assets/native/LICENSE.txt` and
  `assets/makehuman/UPSTREAM_LICENSE.md`.
- **CMU Graphics Lab Motion Capture Database**:
  http://mocap.cs.cmu.edu/. Source funding: NSF EIA-0196217.
  Selected reference clips include CMU-derived movement. These may be used in
  commercial/non-commercial products, but motion data may not be resold on its
  own. See `vendor/ara/scripts/ASSETS_LICENSE.txt`. The native motion file is an
  offline sample of the reference evaluator, not a JavaScript runtime.
- **MPFB2**, MakeHuman Community:
  https://github.com/makehumancommunity/mpfb2, revision
  `6ff1eb358f6c1959ced77e79e6dd2406d9a71f49`.
  Referenced documented MHM/target formats and workflows. No GPL Blender
  implementation code was incorporated into this crate. MPFB's application,
  plugin panels, Rigify integration and Blender materials are outside this
  native game-editor implementation.

The Human Studio code is MIT OR Apache-2.0 subject to retained third-party MIT
notices. Dependencies retain their individual licenses.
