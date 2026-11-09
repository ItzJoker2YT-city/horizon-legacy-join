#!/usr/bin/env python3
"""Generate the lane tables of horizon-legacy-join.

For every lane the data band admits — every advertised release protocol in
47..763, oldest first (1.8.x through 1.20.1) — the id tables come out of
PrismarineJS/minecraft-data's protocol.json:
the lane's own document and the engine's inner session (1.20.3 = protocol
765). All of them — pre-flattening eras included — express each state and
direction through the same `types.packet` mapper, so a packet identity is a
name match between the two documents:

    lane id (lane's protocol.json)
      --name--> inner 765 id (1.20.3 protocol.json)

composed into `LANE_<protocol>_<STATE>_<CB|SB>` (and `_DOWN`) const arrays
between the markers in legacy_data/src/lib.rs. The engine reads only two of
the four directions per state: clientbound UP (lane id -> inner id, what a
legacy server sent) and serverbound DOWN (inner id -> lane id, what Horizon
sends); only those live in the marker block, plus the generated lane list
`GENERATED_LANES` that `covers()` consults. The other half-directions the
situation never asks are not emitted.

Lanes are derived, never hand-listed: every protocolVersions.json entry with
releaseType "release" and a version in the band 47..763 becomes one (the
band is this mod's whole span; upstream's own adapter floor starts at 764,
and a fixed list would drift from the data). A lane's document resolves
first by exact release name (a pc slot named like the release is the
release's own protocol.json — prismarine's per-slot version.json claims are
occasionally stale), then by any slot whose version.json claims the number.
The engine's gate holds no list of its own: it admits exactly
GENERATED_LANES through the data crate's `covers()`.

The committed file is what a released .hnmod ships; CI reruns this every
build and keeps the committed tables on any network failure.

Usage: python3 tools/generate_data.py [--keep-committed]
"""
from __future__ import annotations

import json
import re
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
STATE_LABELS = ["HANDSHAKE", "STATUS", "LOGIN", "CONFIG", "PLAY"]
# The lane band: every advertised release protocol this mod covers. Upstream's
# own adapter floor starts at 764 (1.20.2); its inner session speaks 765.
BAND = (47, 763)
# Numeric banding alone is not enough: the 1.4..1.6 eras numbered their
# protocols (49..78) inside the same span 1.8's 47 opens. This mod's floor is
# the 1.8 era, so the name decides the floor.
FLOOR = (1, 8)
INNER_DIR = "1.20.3"  # protocol 765: the inner session's numbering
DIRECTIONS = (("toClient", "clientbound"), ("toServer", "serverbound"))


def fetch(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "horizon-legacy-join"})
    with urllib.request.urlopen(request, timeout=60) as reply:
        return reply.read()


def fetch_json(path: str):
    try:
        return json.loads(fetch(MD_RAW.format(rev=MD_DATA_REV, path=path)))
    except Exception as exc:  # noqa: BLE001 - the caller's fallback applies
        print(f"w: {path}: {exc}", file=sys.stderr)
        return None


def slot_documents(catalog: dict) -> dict[int, str]:
    """protocol number -> the document path data/<dir>/protocol.json, verified
    through every slot's own version.json claim (protocol.json says nothing)."""
    out: dict[int, str] = {}
    for slot, entries in (catalog.get("pc") or {}).items():
        directory = (entries or {}).get("protocol") or ""
        if not directory:
            continue
        info = fetch_json(f"data/{directory}/version.json")
        if not isinstance(info, dict):
            continue
        number = info.get("version") or info.get("protocol")
        if isinstance(number, int) and number not in out:
            out[number] = directory
    return out


# The lone 1.20.1-style bands aside, prismarine's protocolVersions.json leaves
# release entries untagged (it marks only 24 of 844 entries 'release'); a
# version's own name is the reliable discriminator. A real release is a plain
# dotted number: 1.8, 1.19.4, 26.2 — never 15w40b, 1.20.2-pre1, 1.16-rc1,
# 26.4-snapshot-7 or the classic-era letters.
RELEASE_NAME = re.compile(r"\d+(?:\.\d+){0,2}\Z")


def is_release(entry) -> bool:
    if entry.get("releaseType") == "snapshot":
        return False
    return RELEASE_NAME.fullmatch(entry.get("minecraftVersion") or "") is not None


def release_band(index: list) -> list[tuple[int, str]]:
    """(protocol, release name) for every release entry in the band, oldest
    first, one lane per protocol (the first name met — the index runs newest
    first — is the last release that spoke the protocol)."""
    names: dict[int, str] = {}
    for entry in index:
        if not isinstance(entry, dict) or not is_release(entry):
            continue
        number = entry.get("version")
        if not isinstance(number, int) or not BAND[0] <= number <= BAND[1]:
            continue
        name = entry.get("minecraftVersion") or "?"
        if tuple(int(part) for part in name.split(".")) < FLOOR:
            continue
        names.setdefault(number, name)
    return sorted(names.items())


def named_slots(catalog: dict, index: list) -> dict[int, str]:
    """protocol number -> a pc slot path through release entries by exact
    minecraftVersion name. prismarine's per-slot version.json claims are
    sometimes stale (1.11.2 says 315, whole 1.14.x says one number), while a
    slot named exactly like the release is the release's own document."""
    out: dict[int, str] = {}
    for entry in index:
        if not isinstance(entry, dict) or not is_release(entry):
            continue
        number = entry.get("version")
        name = entry.get("minecraftVersion")
        directory = ((catalog.get("pc") or {}).get(name) or {}).get("protocol")
        if isinstance(number, int) and name and directory and number not in out:
            out[number] = directory
    return out


def packet_names_by_id(protocol_json: dict) -> dict[tuple[int, str], dict[int, str]]:
    """(state #, direction) -> {packet id: prismarine packet name}."""
    out: dict[tuple[int, str], dict[int, str]] = {}
    for state, number in STATES.items():
        for suffix, direction in DIRECTIONS:
            node = ((protocol_json.get(state) or {}).get(suffix) or {}).get("types") or {}
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
        by_name.setdefault(inner_name, inner_id)  # the lowest inner id wins
    for lane_id, lane_name in lane_map.items():
        if 0 <= lane_id < 256 and lane_name in by_name:
            up[lane_id] = by_name[lane_name]
    down: list[int | None] = [None] * 256
    for lane_id, inner_id in enumerate(up):
        if inner_id is not None and down[inner_id] is None:
            down[inner_id] = lane_id
    return up, down


def rust_table(name: str, values: list[int | None]) -> str:
    cells = [f"Some({v})" if v is not None else "None" for v in values]
    return (
        "#[rustfmt::skip]\n"
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
    catalog = fetch_json("data/dataPaths.json")
    if not isinstance(catalog, dict):
        raise SystemExit("could not read minecraft-data's dataPaths.json")
    index = fetch_json("data/pc/common/protocolVersions.json")
    if not isinstance(index, list):
        raise SystemExit("could not read minecraft-data's protocolVersions.json")
    candidates = release_band(index)
    slots = named_slots(catalog, index)
    for number, directory in slot_documents(catalog).items():
        slots.setdefault(number, directory)  # the claim pass fills only gaps
    inner_path = f"data/{(catalog.get('pc') or {}).get(INNER_DIR, {}).get('protocol')}/protocol.json"
    inner_json = fetch_json(inner_path)
    if not isinstance(inner_json, dict):
        raise SystemExit("could not read the inner session's protocol.json")
    inner = packet_names_by_id(inner_json)

    statics: list[str] = []
    up_arms: list[str] = []
    down_arms: list[str] = []
    covered: list[int] = []
    for protocol, name in candidates:
        slot = slots.get(protocol)
        if slot is None:
            print(f"w: lane {protocol} ({name}): no minecraft-data slot resolves; skipped")
            continue
        lane_json = fetch_json(f"data/{slot}/protocol.json")
        if not isinstance(lane_json, dict):
            print(f"w: lane {protocol} ({name}): its protocol.json did not load; skipped")
            continue
        lane = packet_names_by_id(lane_json)
        mapped_total = 0
        for number in range(5):
            label = STATE_LABELS[number]
            clientbound_up, _ = compose(
                lane.get((number, "clientbound"), {}),
                inner.get((number, "clientbound"), {}),
            )
            _, serverbound_down = compose(
                lane.get((number, "serverbound"), {}),
                inner.get((number, "serverbound"), {}),
            )
            statics.append(rust_table(f"LANE_{protocol}_{label}_CB", clientbound_up))
            statics.append(rust_table(f"LANE_{protocol}_{label}_SB_DOWN", serverbound_down))
            up_arms.append(f"        ({protocol}, {number}) => &LANE_{protocol}_{label}_CB,")
            down_arms.append(
                f"        ({protocol}, {number}) => &LANE_{protocol}_{label}_SB_DOWN,"
            )
            mapped_total += sum(v is not None for v in clientbound_up)
            mapped_total += sum(v is not None for v in serverbound_down)
        covered.append(protocol)
        print(f"lane {protocol} ({slot}) [{name}]: {mapped_total} mapped ids in 10 tables")
    for number, name in candidates:
        if number not in covered:
            print(f"w: lane {number} ({name}): kept out of GENERATED_LANES")
    if not covered:
        raise SystemExit("no lane generated; keeping the committed tables")
    lines = [
        "/// The id tables this build ships: one lane per advertised release in",
        "/// the band 47..763 that resolved (tools/generate_data.py derives the",
        "/// set from minecraft-data). lane id -> the inner session's 765-era id",
        "/// by prismarine packet name (clientbound), and the reverse",
        "/// (serverbound), per state and lane. Ids the pinned era does not",
        "/// know by name stay None: the engine drops such packets instead of",
        "/// sending them into the lane wire.",
        "",
        "/// The lane -> inner id map for one lane and state (clientbound side).",
        "pub fn table_up(protocol: i32, state: u8) -> &'static [Option<u8>; 256] {",
        "    match (protocol, state) {",
        *up_arms,
        "        _ => &IDENTITY,",
        "    }",
        "}",
        "",
        "/// The inner -> lane id map for one lane and state (serverbound side).",
        "pub fn table_down(protocol: i32, state: u8) -> &'static [Option<u8>; 256] {",
        "    match (protocol, state) {",
        *down_arms,
        "        _ => &IDENTITY,",
        "    }",
        "}",
        "",
        "/// The protocols whose tables are generated above; `covers` reads it.",
        "static GENERATED_LANES: &[i32] = &[" + ", ".join(str(p) for p in covered) + "];",
        "",
    ]
    write_tables(["\n".join(lines)] + statics)
    print(f"generated {len(covered)} lanes of {len(candidates)} release candidates: {covered}")
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
    print(f"{count} lane entries into legacy_data/src/lib.rs")


if __name__ == "__main__":
    main()
