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
| **alpha (this repo)** | Engine + hooks fully wired: lane detection on the join path, a per-connection bridge (an upstream `Session` pinned to 1.20.3, stamped as its lane's driver through an injected state), the handshake rewritten to the server's own protocol, the login flow, and the configuration phase fabricated for servers that predate 1.20.2 (era registries, tags, known packs, the finish). **Every advertised release 1.8.x..1.20.1 has a lane** — 35 of the 36 legacy release protocols, id tables name-matched from PrismarineJS/minecraft-data per release. The lone gap: **1.14.2 (485)**, which no minecraft-data document covers — those servers are refused until the data grows. Id-level alpha: era payload layouts are not translated yet, so pre-flattening joins (roughly 1.8..1.14, where packet bodies diverge hard) degrade or die rather than play fully; newer lanes translate furthest. The server list shows 1.8.x–1.20.1 entries as *incompatible* (display only — see How it works); selecting and joining them works. |
| later alphas | Payload-layout translations per era (chat signing, the 1.13 flattening, pre-flattening item formats, login-era bodies), a data path for 1.14.2, and runtime coverage checks against real era servers. |

The design mirrors upstream's engine (`crates/horizon-protocol/src/adapter`): one
session per connection, per-version tables, translations release by release. It
reuses upstream's machinery where it can (the adapter's own legacy name tables,
registry splitting, text handling) and adds the lanes upstream never generated.

### Who covers what

| Range | Owner | How |
|---|---|---|
| 1.20.2 (764) .. 26.3 (777) — every release between: 1.20.3/4, 1.20.5/6, 1.21/1.21.1, 1.21.2/3, 1.21.4, 1.21.5, 1.21.6, 1.21.7/8, 1.21.9/10, **1.21.11 (774)**, 26.1/26.1.1/26.1.2, 26.2 | Horizon upstream | its own adapter; `ServerVersion::from_protocol` resolves each release |
| 1.8.x (47) .. 1.20.1 (763) — every release between: 1.9.x, 1.10.x, 1.11.x, 1.12.x, 1.13.x, 1.14.x, 1.15.x, 1.16.x, 1.17.x, 1.18.x, 1.19.x, 1.20/1.20.1 | this mod | per-release id tables generated from minecraft-data; the engine's gate admits exactly the generated lane set |

The split is enforced inside the engine: a protocol upstream already resolves
never becomes a lane of this mod, so as broader tables arrive they can only
widen coverage downward — 1.21.11 and friends stay on upstream's own session.

## Installing

1. Download the newest `alpha.N` release from GitHub Releases.
2. Unzip it into the Horizon launcher's mods folder (the launcher builds the game
   with it on the next `native-build`). The bundle is pinned to one Horizon commit
   (the Nightly it was made against); the launcher Mismatch may offer "Build anyway".

The mod carries the `HORIZON_OLDER_SERVERS` gate with it: the built-in support
stays active, and the mod's own lanes activate alongside it.

## How it works (one page)

Every hook is a *cancellable* head injection (`horizon-mixins` kind `HEAD`):
the helper either does the legacy work itself and breaks out of the target
with its answer, or returns *continue* and the untouched original body runs —
upstream's own adapter behavior stays on for 1.20.2..26.2 servers and plain
connections.

- `horizon_client::net::older_server`: a pinged protocol that upstream cannot
  name but this mod's generated lane set covers is resolved and recorded on the
  joining thread (the hook also answers upstream's `HORIZON_OLDER_SERVERS`
  gate, so the built-in support stays active).
- `Connection::adapt`: when a covered lane was just recorded, a *stamped
  legacy session* — an upstream [`Session`](https://github.com/horizon-rust/horizon/blob/main/crates/horizon-protocol/src/adapter/session.rs)
  pinned to 1.20.3 (765), stamped with the lane's protocol number through an
  injected `[[state]]` whose constructor sits on `Session::new` — is attached
  to the connection's own adapter fields (both halves). Upstream versions
  run upstream's body unchanged.
- `ConnReader::recv` / `ConnWriter::send`: on a stamped session, the bridge in
  `adapter::legacy_join` renumbers and rewrites packets both ways, queues
  multi-packet translations, fabricates the configuration phase (era
  registries, tags, known packs, the finish) for servers that go login → play
  directly, and keeps the wire quiet where the era never speaks (the login
  acknowledgment). Without a stamp, the original body runs.
- The engine (the `legacy_join` module) is injected into `adapter.rs`, so it
  reads upstream's own private tables and legacy registries; its packet id
  maps live in the engine's workspace crate (`mod/legacy_data`, generated by
  `mod/tools/generate_data.py`).

Alpha-1 display limitation: `ServerVersion::from_protocol` cannot be hooked —
the planner's cancellation contract rejects an `Option<Self>` result — so the
server list labels 1.8.x–1.20.1 entries *incompatible*. The list entry's Join
button joins the address anyway, and the bridge takes it from there (the
display can switch to the "joinable" behavior once upstream's contract has a
hole for resolutions).

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
