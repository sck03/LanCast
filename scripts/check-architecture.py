#!/usr/bin/env python3
"""Check dependency boundaries; enforcement stays active under python -O."""
import pathlib
import sys
from architecture import check


def main():
    errors = check(pathlib.Path(__file__).resolve().parents[1])
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("PASS: Rust manifest/source boundaries, isolated WSS control, Apple contracts/media and platform player dependencies")
    return 0


if __name__ == "__main__":
    sys.exit(main())
