"""Private operator commands; run as the service account, never log issued tokens."""
import argparse
import json

from .service import Config, Store


def main():
    parser = argparse.ArgumentParser()
    command = parser.add_mutually_exclusive_group(required=True)
    command.add_argument("--maintain", action="store_true")
    command.add_argument("--check", action="store_true")
    command.add_argument("--issue-token", choices=("operator", "trainer", "reviewer"))
    args = parser.parse_args()
    store = Store(Config.from_environment())
    if args.maintain:
        print(json.dumps(store.maintain()))
    elif args.issue_token:
        print(json.dumps(store.issue(args.issue_token)))
    else:
        with store.connect() as db:
            healthy = db.execute("PRAGMA quick_check").fetchone()[0] == "ok"
        if not healthy:
            raise SystemExit("Database check failed")
        print('{"healthy":true,"apiVersion":1}')


if __name__ == "__main__":
    main()

