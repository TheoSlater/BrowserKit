# BrowserKit Wry provenance

- Upstream package: Wry 0.57.0.
- Vendored upstream base: `792d0359ba6501a4fc360ece17de2ae42329a47c` (verify against source history before next sync).
- BrowserKit vendored/fork commit: no BrowserKit fork commit exists yet; workspace vendors an uncommitted tree.
- Last synchronized: 2026-09-10.

## Applied patches

- Source: Wry PR #1745, commit `e117e1e`.
  Description: position `gtk::Fixed` child WebViews with `Fixed::move_` and retain size request across relayout.
  Platform: Linux GTK3/WebKitGTK.
  Modified: yes; minimal code and inline provenance comment retained, without upstream changelog file.

## BrowserKit-specific patches

- Cargo feature declaration `v2_42` added so existing conditional code is checked without an `unexpected_cfgs` warning.
