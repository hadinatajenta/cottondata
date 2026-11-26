#!/usr/bin/env python3
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from PIL import Image

def main() -> None:
    root = Path(__file__).resolve().parent.parent
    assets = root / "assets"
    master = assets / "AppIcon-1024.png"
    if not master.exists():
        sys.exit(f"Missing {master}")

    img = Image.open(master).convert("RGBA")

    for sz in (128, 256, 512):
        resized = img.resize((sz, sz), Image.Resampling.LANCZOS)
        resized.save(assets / f"AppIcon-{sz}.png", "PNG", optimize=True)

    ico_sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
    img.save(assets / "AppIcon.ico", format="ICO", sizes=ico_sizes)

    if sys.platform == "darwin":
        with tempfile.TemporaryDirectory() as tmp:
            iconset = Path(tmp) / "AppIcon.iconset"
            iconset.mkdir()
            for sz in (16, 32, 128, 256, 512):
                img.resize((sz, sz), Image.Resampling.LANCZOS).save(iconset / f"icon_{sz}x{sz}.png")
                img.resize((sz * 2, sz * 2), Image.Resampling.LANCZOS).save(iconset / f"icon_{sz}x{sz}@2x.png")
            subprocess.run(
                ["iconutil", "-c", "icns", str(iconset), "-o", str(assets / "AppIcon.icns")],
                check=True,
            )

    for name in ("AppIcon-1024.png", "AppIcon-512.png", "AppIcon-256.png", "AppIcon-128.png", "AppIcon.ico", "AppIcon.icns"):
        p = assets / name
        if p.exists():
            print(f"OK: {p} ({p.stat().st_size:,} bytes)")

if __name__ == "__main__":
    main()
