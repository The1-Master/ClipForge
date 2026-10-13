"""Private operator annotation CLI. Credentials use a masked prompt or stdin only."""
from __future__ import annotations
import argparse
import getpass
import json
from pathlib import Path
import sys


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--annotation", required=True)
    parser.add_argument("--credential-stdin", action="store_true")
    args = parser.parse_args()
    root = Path(args.root).resolve(strict=True)
    annotation = Path(args.annotation).resolve(strict=True)
    if not annotation.is_relative_to(root) or annotation.stat().st_size > 16384:
        raise ValueError("Use a bounded annotation JSON file inside the workspace.")
    sys.path.insert(0, str(root / "app"))
    from automation import learning
    if learning.ROOT.resolve() != root:
        raise ValueError("Set CLIPFORGE_WORKSPACE to this exact workspace before running annotation.")
    document = json.loads(annotation.read_text(encoding="utf-8"))
    if args.credential_stdin:
        value = sys.stdin.buffer.readline(1026)
        if len(value) > 1025:
            raise ValueError("The private credential input exceeds its limit.")
        credential = value.decode("utf-8").strip()
    else:
        credential = getpass.getpass("Independent reviewer credential: ")
    try:
        print(json.dumps(learning.save_grounding(document, credential), ensure_ascii=False))
    finally:
        credential = ""


if __name__ == "__main__":
    main()
