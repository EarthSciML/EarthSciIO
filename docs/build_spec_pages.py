#!/usr/bin/env python3
"""Publish spec/*.md as docs pages under docs/content/generated/spec/.

The files under spec/ stay the source of truth; this copies them with Hugo front
matter, turns links between spec documents into site links, and points links to
anything else in the repo at GitHub. Output is never committed.
"""

import re
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "spec"
OUT = ROOT / "docs" / "content" / "generated" / "spec"
GITHUB = "https://github.com/EarthSciML/EarthSciIO/blob/main"

ORDER = [
    "README",
    "cache-format",
    "registries",
    "offline-mode",
    "conformance",
    "prefetch",
    "cloud-future",
    "parquet-bindings-handoff",
]
LINK = re.compile(r"\]\(([^)\s]+)\)")


def rewrite(target: str, name: str) -> str:
    if re.match(r"[a-z]+:|#", target):
        return target
    path, _, frag = target.partition("#")
    frag = "#" + frag if frag else ""
    if path.endswith(".md") and "/" not in path:
        stem = path[:-3]
        return ("/generated/spec/" if stem == "README" else f"/generated/spec/{stem}/") + frag
    resolved = (SPEC / path).resolve()
    try:
        rel = resolved.relative_to(ROOT)
    except ValueError:
        return target
    return f"{GITHUB}/{rel.as_posix()}{frag}"


def convert(text: str, name: str) -> str:
    out, fence = [], False
    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            fence = not fence
        out.append(line if fence else LINK.sub(lambda m: f"]({rewrite(m.group(1), name)})", line))
    return "\n".join(out) + "\n"


def main() -> None:
    shutil.rmtree(OUT, ignore_errors=True)
    OUT.mkdir(parents=True)
    names = sorted(
        (p.stem for p in SPEC.glob("*.md")), key=lambda s: (ORDER.index(s) if s in ORDER else 99, s)
    )
    for weight, stem in enumerate(names, start=1):
        text = (SPEC / f"{stem}.md").read_text(encoding="utf-8")
        title = next(
            (line[2:].strip() for line in text.splitlines() if line.startswith("# ")), stem
        )
        body = re.sub(r"\A# .*\n", "", text, count=1)
        front = f'---\ntitle: "{title.replace(chr(34), chr(39))}"\nweight: {weight}\n---\n\n'
        note = f"*Source of truth: [`spec/{stem}.md`]({GITHUB}/spec/{stem}.md).*\n\n"
        dest = OUT / ("_index.md" if stem == "README" else f"{stem}.md")
        dest.write_text(front + note + convert(body, stem), encoding="utf-8")
    print(f"wrote {len(names)} spec pages to {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
