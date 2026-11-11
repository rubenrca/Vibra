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

## Text truncation

In `src/elements/text.rs`, a measured text layout is reused only when it was
truncated for the same width (`TextLayoutInner::truncate_width`), and each
measurement starts from the untruncated text runs. Upstream reused the first
measurement of any text that does not wrap, so a truncating flex item that
shrank after its first measurement kept the full string and was clipped at its
edge with no ellipsis.

## Max-content size of wrapping text

In the same function, a max-content measurement of text that wraps no longer
reuses a layout wrapped at a definite width; it shapes the unwrapped line and
answers that size, leaving the wrapped layout as the one painted. Upstream reused
any earlier layout for a measurement without a width. When a flex row was first
laid out at min-content, the 0px layout of its text came back as the text's
max-content size, so the flex base size was 0 and a short label (a space name on
an agent card) was painted a letter per line.

## Uniform list bottom padding

In `src/elements/uniform_list.rs`, the last rendered item index counts the
list's bottom padding, as the first already counts its top padding. Upstream
stopped rendering at the padded bounds, so while scrolling, rows left a blank
band in the bottom padding although the content mask covers it. Vibra pads the
Changes graph so its last commit scrolls clear of a fade laid over the list's foot.

## Vendored footprint

The upstream example programs and their assets are omitted, along with their
`[[example]]` targets in the normalized `Cargo.toml`. They are not used by Vibra;
the example GIF alone occupied 4.47 MB. The standalone GPUI `Cargo.lock` is also
omitted: Vibra's root lockfile pins the dependency graph used by the app and its
tests. Library sources, build inputs, upstream tests, provenance and license files
are retained. `Cargo.toml.orig` remains the original upstream manifest for reference.
