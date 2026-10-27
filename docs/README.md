# EarthSciIO docs

The site is built from [EarthSciML/earthsci.dev](https://github.com/EarthSciML/earthsci.dev);
this repo keeps content and configuration only.

- `site.toml` — project id, title, nav, prebuild commands.
- `content/` — hand-written pages. `content/generated/` is build output (never committed).
- `build_spec_pages.py` — publishes `spec/*.md` as pages (`spec/` stays the source of truth).
- `api/` — API reference inputs: `config.toml`, `groups.toml`, `guides/`, runnable `examples/`;
  the manifest is `../api-surface.json`.

Preview locally: run `build_spec_pages.py`, the binding scripts and `render.py` from a
checkout of earthsci.dev (`tools/api_docs/README.md` there), then `tools/bundle.py` and
`tools/assemble.py --local io=<bundle>` and `hugo server` (README there).

`.github/workflows/docs.yml` publishes the bundle to the `docs-bundle` branch and notifies
earthsci.dev on every push to `main`; it needs the `SITE_DISPATCH_TOKEN` secret.
