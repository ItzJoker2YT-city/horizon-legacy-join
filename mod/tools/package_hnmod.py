#!/usr/bin/env python3
"""Package the mixin bundle as the launcher's native-mod format (.hnmod).

The launcher reads a native bundle as a zip that carries the mod's
`horizon-native.json` at its root and the whole mixin bundle under
`mixins/<mod id>/` (horizon-mods/src/index/native.rs::Bundle::read): identity
comes from the bundle's own horizon-mixin.toml — [mod] id and version,
[compatibility] horizon_revision — and the json also records the exact source
revisions the build was tested against: the Horizon commit and the
horizon-mixins commit. One file, one mod: the json id must equal the mixin
manifest's.

The mod id a native manifest accepts is [a-z0-9_]; this packer fails on
anything else. So does a stray Cargo.lock (the staged workspace owns the
lock), a path with backslashes, or a source file the native bundle rules do
not allow (.rs/.toml, a crate's text files, data folders).

Usage: python3 tools/package_hnmod.py --game <version> --horizon <40hex>
            --mixins <40hex> --out <path> [--root <mod dir>]
"""
import argparse
import json
import tomllib
import zipfile
from pathlib import Path

# Never bundled: work-dir scaffolding (.git), build outputs (target), caches,
# the stager's plan scratch, and tools/ — the build's own tooling (the data
# generator and this packer), which the native bundle's file rules do not
# carry; runtime sources and their data live outside it.
SKIP_DIRS = (".git", "target", "__pycache__", "plan", "tools")
SKIP_FILES = (".gitignore", "Cargo.lock")


def is_revision(value: str) -> bool:
    return (
        len(value) == 40
        and all(b in "0123456789abcdef" for b in value.lower())
    )


def walk(root: Path) -> list[tuple[str, Path]]:
    """(relative path with / separators) below root, skipping the build's own."""
    out: list[tuple[str, Path]] = []
    for path in sorted(root.rglob("*")):
        if path.is_dir():
            continue
        rel = path.relative_to(root).as_posix()
        if any(part in SKIP_DIRS for part in rel.split("/")):
            continue
        if path.name in SKIP_FILES:
            continue
        out.append((rel, path))
    return out


def allowed(name: str) -> bool:
    """native.rs::allowed — what a bundle's `mixins/<id>/` may carry."""
    parts = name.split("/")
    kind_text = (
        len(parts) <= 2
        and (parts[-1].startswith(("README", "LICENSE", "LICENCE", "COPYING", "NOTICE"))
             or parts[-1] == ".gitignore")
    )
    data = len(parts) >= 2 and (
        parts[0] == "data" or (len(parts) >= 3 and parts[1] == "data")
    )
    return name.endswith((".rs", ".toml")) or data or kind_text


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=None, help="the mod directory (default: this file's parent's parent)")
    ap.add_argument("--game", required=True, help="the Minecraft version the build is for")
    ap.add_argument("--horizon", required=True, help="the Horizon commit the build pins")
    ap.add_argument("--mixins", required=True, help="the horizon-mixins commit the build applies with")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    for name, value in (("horizon", args.horizon), ("mixins", args.mixins)):
        if not is_revision(value):
            raise SystemExit(f"--{name} {value}: a source commit is 40 hex digits, not a mutable ref")
    root = Path(args.root or Path(__file__).resolve().parent.parent)
    toml = root / "horizon-mixin.toml"
    if not toml.exists():
        raise SystemExit(f"{toml}: the mod directory carries the manifest in horizon-mixin.toml")
    with open(toml, "rb") as handle:
        mixin = tomllib.load(handle)
    mod = mixin.get("mod") or {}
    mod_id = mod.get("id")
    version = mod.get("version")
    horizon_revision = (mixin.get("compatibility") or {}).get("horizon_revision")
    if not (isinstance(mod_id, str) and mod_id and all(
        b in "abcdefghijklmnopqrstuvwxyz0123456789_" for b in mod_id
    ) and len(mod_id) <= 64):
        raise SystemExit(f"mod id {mod_id!r}: a native manifest's id is [a-z0-9_] of 1..64")
    if not isinstance(version, str) or not version:
        raise SystemExit(f"[mod] version {version!r}: required")
    if not isinstance(horizon_revision, str) or not is_revision(horizon_revision):
        raise SystemExit(f"[compatibility] horizon_revision {horizon_revision!r}: required, 40 hex digits")
    if horizon_revision != args.horizon.lower():
        raise SystemExit(
            f"the mixin manifest pins {horizon_revision}, the build tests {args.horizon}"
        )
    entries = walk(root)
    bad = [name for name, _ in entries if "\\" in name or not allowed(f"{name}")]
    if bad:
        raise SystemExit("files a native bundle does not allow: " + ", ".join(bad))

    manifest = {
        "schema": 1,
        "id": mod_id,
        "version": version,
        "game": args.game,
        "horizon": args.horizon.lower(),
        "mixins": args.mixins.lower(),
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as bundle:
        bundle.writestr("horizon-native.json", json.dumps(manifest, indent=2) + "\n")
        for name, path in entries:
            bundle.writestr(f"mixins/{mod_id}/{name}", path.read_bytes())
    # Read what the launcher will read: the manifest at the root, the mixin
    # manifest under mixins/<id>/, and nothing outside that prefix.
    with zipfile.ZipFile(out) as check:
        names = check.namelist()
        again = json.loads(check.read("horizon-native.json"))
        assert again["id"] == mod_id and again["schema"] == 1
        source = check.read(f"mixins/{mod_id}/horizon-mixin.toml")
        assert source.strip()  # the launcher reads the manifest's identity here
        assert all(name.startswith("mixins/") for name in names if name != "horizon-native.json")
    print(
        f"built {out} ({out.stat().st_size} bytes): mod {mod_id} v{version},"
        f" game {args.game}, horizon {args.horizon[:10]}, mixins {args.mixins[:10]},"
        f" {len(entries)} sources"
    )


if __name__ == "__main__":
    main()
