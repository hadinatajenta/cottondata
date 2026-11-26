#!/usr/bin/env python3
"""Generate assets/fonts/Phosphor-Subset.ttf containing only Phosphor icons used by CottonData."""

from __future__ import annotations

import glob
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

EXTRA_ICONS = [
    "DOTS_SIX_VERTICAL",
    "ARROW_CIRCLE_UP",
    "SORT_DESCENDING",
    "SORT_ASCENDING",
    "ARROWS_DOWN_UP",
    "FUNNEL",
    "MAGNIFYING_GLASS",
    "COPY",
    "ARROW_CLOCKWISE",
    "CHECK",
    "X",
    "WARNING",
    "INFO",
    "DOWNLOAD_SIMPLE",
    "ARROW_SQUARE_OUT",
    "SHIELD_CHECK",
    "SPARKLE",
    "DATABASE",
    "TABLE",
    "LIST",
    "TAG",
    "PALETTE",
    "ARROW_UP",
    "ARROW_DOWN",
    "CARET_UP",
    "CARET_DOWN",
    "TRASH",
    "PENCIL_SIMPLE",
    "PLUS",
    "GEAR",
    "CLOCK",
    "LIGHTNING",
]

ICON_USAGE_RE = re.compile(r"\bicon::([A-Z0-9_]+)\b")
CONST_DEF_RE = re.compile(r'pub const ([A-Z0-9_]+): &str = "\\u\{([0-9A-Fa-f]+)\}";')


def find_phosphor_crate() -> Path:
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    pattern = str(cargo_home / "registry" / "src" / "*" / "egui-phosphor-0.9.0")
    matches = sorted(glob.glob(pattern))
    if not matches:
        fallback = str(cargo_home / "registry" / "src" / "*" / "egui-phosphor-*")
        matches = sorted(glob.glob(fallback))
    if not matches:
        raise RuntimeError(
            f"Could not locate egui-phosphor in {cargo_home}/registry/src. Run `cargo fetch` first."
        )
    return Path(matches[-1])


def find_pyftsubset() -> str:
    for candidate in (shutil.which("pyftsubset"), "/opt/homebrew/bin/pyftsubset"):
        if candidate and Path(candidate).is_file():
            return candidate
    raise RuntimeError(
        "pyftsubset not found on PATH or /opt/homebrew/bin/pyftsubset. Install fonttools."
    )


def collect_used_icons(src_dir: Path) -> set[str]:
    icons = set(EXTRA_ICONS)
    for rs_file in src_dir.rglob("*.rs"):
        text = rs_file.read_text(encoding="utf-8")
        for match in ICON_USAGE_RE.finditer(text):
            icons.add(match.group(1))
    return icons


def parse_regular_codepoints(regular_rs: Path) -> dict[str, int]:
    mapping: dict[str, int] = {}
    text = regular_rs.read_text(encoding="utf-8")
    for match in CONST_DEF_RE.finditer(text):
        mapping[match.group(1)] = int(match.group(2), 16)
    return mapping


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    src_dir = repo_root / "crates" / "desktop" / "src"
    output_path = repo_root / "assets" / "fonts" / "Phosphor-Subset.ttf"

    phosphor_dir = find_phosphor_crate()
    regular_rs = phosphor_dir / "src" / "variants" / "regular.rs"
    source_ttf = phosphor_dir / "res" / "Phosphor.ttf"
    if not regular_rs.is_file() or not source_ttf.is_file():
        raise RuntimeError(f"Expected regular.rs and res/Phosphor.ttf under {phosphor_dir}")

    used_icons = collect_used_icons(src_dir)
    codepoints = parse_regular_codepoints(regular_rs)

    missing = sorted(name for name in used_icons if name not in codepoints)
    if missing:
        raise RuntimeError(f"Unknown egui-phosphor regular icon name(s): {', '.join(missing)}")

    unicodes = sorted({f"U+{codepoints[name]:04X}" for name in used_icons})
    pyftsubset = find_pyftsubset()
    output_path.parent.mkdir(parents=True, exist_ok=True)

    cmd = [
        pyftsubset,
        str(source_ttf),
        f"--unicodes={','.join(unicodes)}",
        f"--output-file={output_path}",
        "--layout-features=*",
        "--no-hinting",
    ]
    subprocess.run(cmd, check=True)

    orig_size = source_ttf.stat().st_size
    new_size = output_path.stat().st_size
    savings = (1.0 - new_size / orig_size) * 100.0 if orig_size else 0.0
    print(
        f"Subsetted {len(unicodes)} icons -> {output_path.relative_to(repo_root)} "
        f"({new_size:,} B vs {orig_size:,} B, -{savings:.1f}%)"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
