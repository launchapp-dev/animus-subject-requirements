# Changelog

## v0.1.7

- Advertise `subject_kind:requirement` capability marker so the daemon's
  plugin preflight (which scans `capabilities` for `subject_kind:` prefixes)
  recognizes this plugin as satisfying the `subject_backend` role for
  `kind=requirement`.
- Switch entry point from `subject_backend_main` to
  `subject_backend_main_with_capabilities`, passing
  `["subject_kind:requirement"]` as `extra_capabilities`. Mirrors the
  pattern used by `animus-web-ui` for `$ui/web`.
- Add `subject_kind:requirement` to `[capabilities].methods` in `plugin.toml`
  so tooling that reads the manifest sees the same marker.
- Bump `animus-protocol` pin from `v0.1.8` to `v0.1.13` for the
  `_with_capabilities` entrypoint.
