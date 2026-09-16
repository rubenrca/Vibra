# GPUI patch used by Vibra

This directory vendors the published `gpui` 0.2.2 crate, under its original
Apache-2.0 license (`LICENSE-APACHE`). The crate's `.cargo_vcs_info.json` identifies
upstream revision `69e2130295c2649963eb639fc70b4f2ee8ea1624` and notes a dirty tree;
the published crate, rather than that revision alone, is the source of this copy.

## Metal alpha compositing

In `src/platform/mac/metal_renderer.rs`, both `build_pipeline_state` and
`build_path_sprite_pipeline_state` now use `OneMinusSourceAlpha` for the destination
alpha factor, matching source-over RGB compositing. Path rasterization already
used this factor and remains unchanged.

Previously, a 0.94-alpha window base followed by a 0.07-alpha panel produced
`min(1, 0.94 + 0.07) = 1`, hiding the native backdrop. Source-over produces
`0.07 + 0.94 * (1 - 0.07) = 0.9442`, retaining translucency. Opaque text and icons
still produce alpha 1. The continuous base keeps corners and gaps covered.

The root Cargo patch applies to both the application and its tests. No Cargo
registry sources are modified. Remove this override when an upstream release
contains the correction and has been validated with Vibra.
