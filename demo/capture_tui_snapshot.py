#!/usr/bin/env python3
"""Render tested Ratatui snapshots as reproducible SVG screenshots."""

from __future__ import annotations

import html
from pathlib import Path


REPOSITORY = Path(__file__).resolve().parent.parent
SNAPSHOTS = (
    (
        REPOSITORY
        / "apps/fixtrace-tui/tests/snapshots/render_snapshots__wide_layout_snapshot.snap",
        REPOSITORY / "docs/screenshots/tui-main.svg",
    ),
    (
        REPOSITORY
        / "apps/fixtrace-tui/tests/snapshots/render_snapshots__approval_modal_snapshot.snap",
        REPOSITORY / "docs/screenshots/tui-approval.svg",
    ),
)


def snapshot_lines(source: str) -> list[str]:
    markers = [index for index, line in enumerate(source.splitlines()) if line == "---"]
    if len(markers) < 2:
        raise ValueError("snapshot metadata delimiter was not found")
    return source.splitlines()[markers[1] + 1 :]


def render(source: Path, output: Path) -> None:
    lines = snapshot_lines(source.read_text(encoding="utf-8"))
    font_size = 15
    line_height = 21
    padding = 24
    width = 140 * 9 + padding * 2
    height = len(lines) * line_height + padding * 2
    tspans = "\n".join(
        f'<tspan x="{padding}" dy="{0 if index == 0 else line_height}">{html.escape(line)}</tspan>'
        for index, line in enumerate(lines)
    )
    svg = f"""<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">
  <rect width="100%" height="100%" rx="14" fill="#09111d"/>
  <circle cx="28" cy="20" r="5" fill="#ff5f57"/>
  <circle cx="44" cy="20" r="5" fill="#febc2e"/>
  <circle cx="60" cy="20" r="5" fill="#28c840"/>
  <text x="{padding}" y="{padding + 18}" fill="#dce8f7" font-family="SFMono-Regular, Menlo, Consolas, monospace" font-size="{font_size}" xml:space="preserve">{tspans}</text>
</svg>
"""
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(svg, encoding="utf-8")
    print(output)


for snapshot, destination in SNAPSHOTS:
    render(snapshot, destination)
