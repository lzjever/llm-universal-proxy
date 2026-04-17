#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import pathlib
import re
import sys

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    tomllib = None


ROOT_DIR = pathlib.Path(__file__).resolve().parent.parent
RUST_TOOLCHAIN_ACTION_REF = "3c5f7ea28cd621ae0bf5283f0e981fb97b8a7af9"


def _read(path: pathlib.Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError as exc:
        rel_path = path.relative_to(ROOT_DIR)
        raise RuntimeError(f"missing required file: {rel_path}") from exc


def _load_toml(path: pathlib.Path) -> dict:
    if tomllib is None:
        raise RuntimeError("python3 with tomllib is required")
    return tomllib.loads(_read(path))


def package_name() -> str:
    return _load_toml(ROOT_DIR / "Cargo.toml")["package"]["name"]


def cargo_version() -> str:
    return _load_toml(ROOT_DIR / "Cargo.toml")["package"]["version"]


def rust_toolchain() -> str:
    return _load_toml(ROOT_DIR / "rust-toolchain.toml")["toolchain"]["channel"]


def changelog_version() -> str:
    changelog = _read(ROOT_DIR / "CHANGELOG.md")
    match = re.search(r"^## v([0-9][^\s]*) - ", changelog, re.MULTILINE)
    if match is None:
        raise RuntimeError("failed to locate latest changelog version")
    return match.group(1)


def cargo_lock_version() -> str:
    lock_text = _read(ROOT_DIR / "Cargo.lock")
    pattern = re.compile(
        r'\[\[package\]\]\nname = "' + re.escape(package_name()) + r'"\nversion = "([^"]+)"',
        re.MULTILINE,
    )
    match = pattern.search(lock_text)
    if match is None:
        raise RuntimeError("failed to locate package version in Cargo.lock")
    return match.group(1)


FIELDS = {
    "package_name": package_name,
    "version": cargo_version,
    "lock_version": cargo_lock_version,
    "changelog_version": changelog_version,
    "rust_toolchain": rust_toolchain,
    "rust_toolchain_action_ref": lambda: RUST_TOOLCHAIN_ACTION_REF,
}


def cmd_get(field: str) -> int:
    getter = FIELDS.get(field)
    if getter is None:
        valid = ", ".join(sorted(FIELDS))
        raise SystemExit(f"unknown field `{field}`; expected one of: {valid}")
    print(getter())
    return 0


def cmd_github_output() -> int:
    lines = [f"{name}={FIELDS[name]()}\n" for name in ("version", "rust_toolchain")]
    github_output = os.environ.get("GITHUB_OUTPUT")
    if github_output:
        with open(github_output, "a", encoding="utf-8") as handle:
            handle.writelines(lines)
        return 0
    for line in lines:
        print(line, end="")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Read repository metadata.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    get_parser = subparsers.add_parser("get", help="Read a single metadata field.")
    get_parser.add_argument("field", choices=sorted(FIELDS))

    subparsers.add_parser(
        "github-output",
        help="Emit GitHub Actions step outputs for version and rust toolchain.",
    )
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        if args.command == "get":
            return cmd_get(args.field)
        if args.command == "github-output":
            return cmd_github_output()
        parser.error(f"unsupported command: {args.command}")
        return 2
    except (KeyError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
