# Semantic editor

A native GNOME prototype of a TeXmacs-style semantic editor for math-heavy notes. The
document is a tree, rendered block by block with [Typst](https://typst.app), and edited
directly on the rendered page with a structural caret inside formulas. Files are `.sem`
(JSON); export to `.typ` and PDF.

Rust, gtk4-rs + libadwaita, Typst 0.15.1. GPL-3.0-or-later.

## Documents

- [`spec.md`](spec.md) — goals, architecture, milestones, and the wishlist of upcoming
  milestones (M8+).
- [`PLAN.md`](PLAN.md) — what was built and how: pinned versions, build setup, design
  decisions with their reasons, and what each milestone's tests check.
- The git history has the details of each change.

## Workspace

| Crate | Role |
| --- | --- |
| `model` | document tree, cursor, ops, undo (no GTK, no Typst) |
| `typst-bridge` | serializer, compile worker, render cache, layout → node mapping |
| `editor-widget` | `SemEditor` GTK widget: drawing, caret, input, hit-testing |
| `app` | window, actions, files, export |

## Build and run

Builds use the GNOME 51 SDK Flatpak only (needs `org.gnome.Sdk//51` and the
`org.freedesktop.Sdk.Extension.rust-stable` extension).

```sh
# tests and lints inside the SDK
build-aux/sdk cargo test
build-aux/sdk cargo clippy --all-targets

# build, install and run the app
flatpak-builder --user --install --force-clean build-dir \
    build-aux/io.github.molnarandris.SemanticEditor.Devel.json
flatpak run io.github.molnarandris.SemanticEditor.Devel
```

GNOME Builder picks up the same manifest. Network access is needed during the build
(cargo) and on first use of CeTZ diagrams (Typst package download).
