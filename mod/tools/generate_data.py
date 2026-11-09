#!/usr/bin/env python3
"""Generate the lane tables of horizon-legacy-join.

For lane 763 (1.20.1), the id maps come out of PrismarineJS/minecraft-data's
own protocol.json documents — one for the lane (data/pc/1.20 = 763), one for
the engine's inner session (data/pc/1.20.3 = 765). Both numbering the packet
ids through the same per-state `"mapper"` tables, so a packet identity is a
name match between the two, under prismarine's own conventions:

    lane 763 id (1.20.1 protocol.json)
      --name--> inner 765 id (1.20.3 protocol.json)

...composed into `LANE_763_<STATE>_<CB|SB>` (and `_DOWN`) const arrays between
the markers in legacy_data/src/lib.rs. UP is indexed by the lane id and holds
the 765-era id; DOWN is the reverse. A lane packet with no 765-era counterpart
stays None and the bridge drops it (the pinned client cannot read it).

Any input failure keeps the committed tables (the release ships what was
committed; CI reruns this every build).

Usage: python3 tools/generate_data.py [--keep-committed]
"""
from __future__ import annotations

import json
import sys
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LIB = REPO / "legacy_data" / "src" / "lib.rs"
BEGIN = "// ---- 8< ---- generated tables (edit only by tools/generate_data.py)"
END = "// ---- >8 ---- end generated tables"

MD_RAW = "https://raw.githubusercontent.com/PrismarineJS/minecraft-data/{rev}/{path}"
MD_DATA_REV = "9b995542ab4a17837dee785439954e43d031b33e"  # master Nov 8 2026

# minecraft-data state key -> lib.rs state number.
STATES = {
    "handshaking": 0,
    "status": 1,
    "login": 2,
    "configuration": 3,
    "play": 4,
}
LANE = 763
LANE_DIR = "1.20"  # catalog key: 1.20 and 1.20.1 speak the same protocol (763)
INNER_DIR = "1.20.3"  # protocol 765: the inner session's numbering
DIRECTIONS = (("toClient", "clientbound"), ("toServer", "serverbound"))


def fetch(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "horizon-legacy-join"})
    with urllib.request.urlopen(request, timeout=60) as reply:
        return reply.read()


def fetch_json(url: str):
    try:
        return json.loads(fetch(url))
    except Exception as exc:  # noqa: BLE001 - the caller's fallback applies
        print(f"w: {url}: {exc}", file=sys.stderr)
        return None


def protocol_dir(catalog: dict, slot: str) -> str:
    entry = (catalog.get("pc") or {}).get(slot) or {}
    path = entry.get("protocol") or ""
    if not path:
        raise SystemExit(f"minecraft-data has no protocol for {slot}")
    return path


def protocol_mapping(path: str) -> dict[tuple[int, str], dict[int, str]]:
    """(state #, direction) -> {packet id: prismarine packet name}."""
    protocol = fetch_json(MD_RAW.format(rev=MD_DATA_REV, path=f"data/{path}/protocol.json"))
    if not isinstance(protocol, dict):
        raise SystemExit(f"could not read data/{path}/protocol.json")
    out: dict[tuple[int, str], dict[int, str]] = {}
    for state, number in STATES.items():
        for suffix, direction in DIRECTIONS:
            node = ((protocol.get(state) or {}).get(suffix) or {}).get("types") or {}
            packet = node.get("packet") or []
            maps: dict[int, str] = {}
            # packet: ["container", [ ... {name: '...',
            #   type: ["mapper", {"mappings": {"0x..": "<packet name>"}}]} ...]]
            if isinstance(packet, (list, tuple)) and len(packet) == 2:
                for choice in packet[1] if isinstance(packet[1], (list, tuple)) else []:
                    kind = choice.get("type") if isinstance(choice, dict) else None
                    if not (isinstance(kind, (list, tuple)) and len(kind) == 2 and kind[0] == "mapper"):
                        continue
                    for key, name in (kind[1] or {}).get("mappings", {}).items():
                        try:
                            ident = int(str(key), 0)
                        except ValueError:
                            continue
                        maps[ident] = name
            out[(number, direction)] = maps
    return out


def compose(
    lane_map: dict[int, str],
    inner_map: dict[int, str],
) -> tuple[list[int | None], list[int | None]]:
    """(lane -> inner, inner -> lane) over the prismarine names, one direction."""
    up: list[int | None] = [None] * 256
    by_name: dict[str, int] = {}
    for inner_id, inner_name in inner_map.items():
        by_name.setdefault(inner_name, inner_id)  # the lowest 765 id wins
    for lane_id, lane_name in lane_map.items():
        if 0 <= lane_id < 256 and lane_name in by_name:
            up[lane_id] = by_name[lane_name]
    down: list[int | None] = [None] * 256
    for lane_id, inner_id in enumerate(up):
        if inner_id is not None and down[inner_id] is None:
            down[inner_id] = lane_id
    return up, down


def order_name(number: int, direction: str) -> str:
    labels = ["HANDSHAKE", "STATUS", "LOGIN", "CONFIG", "PLAY"]
    suffix = "CB" if direction == "clientbound" else "SB"
    return f"LANE_{LANE}_{labels[number]}_{suffix}"


def rust_table(name: str, values: list[int | None]) -> str:
    cells = [f"Some({v})" if v is not None else "None" for v in values]
    return (
        "#[rustfmt::skip]\n"
        "#[allow(dead_code)] // both directions are generated; a lane drives either up or down\n"
        "static " + name + ": [Option<u8>; 256] = [\n    "
        + ", ".join(cells) + ",\n];"
    )


def write_tables(statics: list[str]) -> None:
    body = LIB.read_text(encoding="utf-8")
    if BEGIN not in body or END not in body:
        raise SystemExit("legacy_data/src/lib.rs does not carry the markers")
    start = body.index(BEGIN) + len(BEGIN)
    end = body.index(END)
    body = body[:start] + "\n" + "\n\n".join(statics) + "\n" + body[end:]
    LIB.write_text(body, encoding="utf-8")


def generate() -> int:
    catalog = fetch_json(MD_RAW.format(rev=MD_DATA_REV, path="data/dataPaths.json"))
    if not isinstance(catalog, dict):
        raise SystemExit("could not read minecraft-data's dataPaths.json")
    lane = protocol_mapping(protocol_dir(catalog, LANE_DIR))
    inner = protocol_mapping(protocol_dir(catalog, INNER_DIR))
    statics: list[str] = []
    for state, number in STATES.items():
        for _, direction in DIRECTIONS:
            values, reverse = compose(
                lane.get((number, direction), {}),
                inner.get((number, direction), {}),
            )
            reference = order_name(number, direction)
            statics.append(rust_table(reference, values))
            statics.append(rust_table(reference + "_DOWN", reverse))
    write_tables(statics)
    print("generated lane 763 (1.20.1) tables: lane id -> inner (765) by name")
    return len(statics)


def main() -> None:
    args = sys.argv[1:]
    if "--keep-committed" in args:
        print("keeping the committed tables")
        return
    try:
        count = generate()
    except SystemExit as exc:
        print(f"w: keeps the committed tables: {exc}", file=sys.stderr)
        return
    print(f"{count} lane tables into legacy_data/src/lib.rs")


if __name__ == "__main__":
    main()
