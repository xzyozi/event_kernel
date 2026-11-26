"""QG-003: core/ のクレート属性を検査する（EK-ENV-001 4.4節、ADR-004）。

- クレートルート（core/src/lib.rs）に `#![no_std]` と `#![forbid(unsafe_code)]` がある。
- core/src/ の Rust コードに、`std` の参照がない（`alloc` と `core` だけを使う）。

使い方: python .github/scripts/check_core_attrs.py [リポジトリのルート]
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REQUIRED_ATTRS = ("#![no_std]", "#![forbid(unsafe_code)]")
STD_REF = re.compile(r"(?<![\w:])(?:::)?std::|\bextern\s+crate\s+std\b")


def strip_comments(line: str) -> str:
    """行コメントを除く。文字列中の `//` は、core/src/ では使わない前提とする。"""
    return line.split("//", 1)[0]


def check(root: Path) -> list[str]:
    errors: list[str] = []
    src = root / "core" / "src"
    lib = src / "lib.rs"
    if not lib.is_file():
        return [f"{lib.relative_to(root)} がありません"]
    lines = [strip_comments(x).strip() for x in lib.read_text(encoding="utf-8").splitlines()]
    for attr in REQUIRED_ATTRS:
        if attr not in lines:
            errors.append(f"core/src/lib.rs に {attr} がありません")
    for path in sorted(src.rglob("*.rs")):
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if STD_REF.search(strip_comments(line)):
                errors.append(f"{path.relative_to(root).as_posix()}:{number}: std を参照しています")
    return errors


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.cwd()
    errors = check(root.resolve())
    for error in errors:
        print(f"ERROR: {error}")
    if errors:
        return 1
    print("QG-003: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())