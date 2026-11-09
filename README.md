# Horizon Legacy Join

A Horizon native mod (`.hnmod`, mixin bundle) that lets **Horizon join older Minecraft
servers: any release from 1.8.x up to 1.20.1** (protocol 47 → 763). Why: Horizon speaks
protocol 777 (26.3) and its built-in adapter only covers servers from 1.20.2 upward;
this mod extends the same adapter machinery down through the legacy protocols.

- Self-contained: no external proxy, no server plugin, no `ViaProxy` — the game does it by itself.
- Works on the launcher's own native build (the mod changes the game's own Rust source
  before it compiles, through [horizon-mixins](https://github.com/horizon-rust/horizon-mixins)).
- Every CI build on this repo publishes a fresh **alpha/beta release** of the bundle.

## Status

| Milestone | Content |
|---|---|
| **alpha-1 (this repo)** | Engine + hooks fully wired: legacy detection (the server list shows old servers as joinable), a per-connection bridge (an upstream `Session` pinned to 1.20.3 driven by a lane-wire adapter), the handshake rewritten to the server's own protocol, the login flow, and the configuration phase fabricated for servers that predate 1.20.2 (era registries, tags, known packs, the finish). Lane **763 (1.20.1)** has generated packet id tables (lane → inner 765-era ids, name-matched from PrismarineJS/minecraft-data; 108 play entries plus status and login). Older lanes are listed in the registry and gated on their tables, which arrive release by release. |
| later alphas | Tables for the lanes toward 1.8 (per-release protocol.json runs), payload-layout translations per era (chat signing, the 1.13 flattening, pre-flattening item formats), and full coverage 47..763. |

The design mirrors upstream's engine (`crates/horizon-protocol/src/adapter`): one
session per connection, per-version tables, translations release by release. It
reuses upstream's machinery where it can (the adapter's own legacy name tables,
registry splitting, text handling) and adds the lanes upstream never generated.

## Installing

1. Download the newest `alpha.N` release from GitHub Releases.
2. Unzip it into the Horizon launcher's mods folder (the launcher builds the game
   with it on the next `native-build`). The bundle is pinned to one Horizon commit
   (the Nightly it was made against); the launcher Mismatch may offer "Build anyway".

The mod replaces the `HORIZON_OLDER_SERVERS` gate: the built-in support stays
active, and the mod's own lanes activate alongside it.

## How it works (one page)

- `horizon_client::net::older_servers_enabled` is overwritten to `true`.
- `horizon_client::net::older_server` is overwritten: it pings for the protocol
  number; unknown-to-upstream protocols that the mod's lane registry covers are
  resolved on this connection and recorded for the next step.
- `ServerVersion::from_protocol` is overwritten so the server list marks those
  servers *joinable* (the displayed version name still comes from the ping json).
- `Connection::adapt` is overwritten: a legacy connection gets the mod's bridge
  (`crate::adapter::legacy_join::Bridge`, a support module injected into
  `adapter.rs`) attached to both halves; upstream versions keep upstream's `Session`.
- `ConnReader::recv` / `ConnWriter::send` are overwritten (their original bodies
  kept as the non-legacy path): the bridge renumbers and rewrites packets both
  ways, queues multi-packet translations, and fabricates the configuration phase
  for servers that go login → play directly.

## Layout

```
mod/horizon-mixin.toml     the bundle manifest (injections, support module, state)
mod/helpers/               one .rs file per hook point (compiled into the game)
mod/engine/legacy_bridge.rs the injected support module (the whole lane engine)
mod/tools/generate_data.py composes the lane tables from PrismarineJS/minecraft-data's
                           per-version protocol documents (name-matched lane -> 765-era ids)
mod/legacy_data/           the generated tables, a workspace crate of the engine's own
.github/workflows/         CI: apply the mixins, cargo-check, package, release
```

The bundle lives in `mod/` (a `--mods` root holds one directory per mod,
each with its own `horizon-mixin.toml`; the tool refuses a manifest
sitting in the root itself).

## Building yourself

CI (`.github/workflows/release.yml`) does everything: it clones Horizon at the
pinned commit, regenerates lane data when needed, runs `horizon-mixins plan/apply`,
`cargo check`s the affected crates offline-locked, zips the bundle as
`horizon-legacy-join-<version>.hnmod`, and publishes a release. Locally you need
rustup 1.95+ and Python 3.11.

## License

MIT (see LICENSE). Horizon is MIT, horizon-mixins is MIT.
