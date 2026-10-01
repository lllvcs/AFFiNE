# AFFiNE self-hosted fork — development record

> **What this file is.** The record of what this fork changed against upstream
> and what happened while doing it: the incidents, how each was actually
> diagnosed, what was verified and what was not. Written for a developer who is
> about to change this code and wants the history, not the rules.
>
> The **rules** distilled from this record live in
> [`DEVELOP_SKILL.MD`](./DEVELOP_SKILL.MD) (English, agent-facing).
> Configuration and deployment *usage* live in
> [`README.md`](./README.md) / [`README.zh-CN.md`](./README.zh-CN.md).

- Baseline: upstream AFFiNE source at version `0.27.5`, branch `canary`
- Images: `ghcr.io/lllvcs/affine` (built) → `lvcs/affine` (published)
- This machine has no docker and no old clients: protocol and end-to-end
  behaviour can only be verified on the deployment instance.

---

## 1. Change list

| Area | Commits | What changed |
| --- | --- | --- |
| Self-hosting limits | `c5844b1` | blob 10 GiB, storage quota 2^53−1, seat limit `i32::MAX`, history 100 years |
| BYOK + copilot diagnostics | `c5844b1` `a16e5f3` `4d1f8b8` | list models from BYOK providers; route failures carry slot, required capabilities, deployment and BYOK state; the UI states that chat needs text output **and** tool calling |
| OIDC without `email_verified` | `42bbe52` | `trustUnverifiedEmail` (explicit `false` still rejected), finer error reasons, 3 Rust unit tests |
| OIDC redirect URI | `c9d5013` | scheme follows the request: externalUrl host → request scheme (allow-list only) → `server.https` |
| Sign-in method switches | `28ac1be` `8bc95d7` `b57a6b9` `4b5a1c1` `7100627` `ed18a8c` | `auth.signInMethods.{password,magicLink,oauth}`, readable from `config.json` and the environment, enforced server-side |
| Realtime sync for released clients | `9b9f22e` `6912abd` | legacy `space:join` and the `sync-025`/`sync-026` rooms are served again; a rejected join names the client version |
| Self-hosting docs | `9bb4520` `f422eeb` `3fb9695` | `server.hosts` semantics, the two config readers, the two-list rule |
| Image pipeline | `b4781c0` … `fdf8f1b` | build and publish split, registry-to-registry copy, Pro public key gate |

---

## 2. Incident log

Each entry: symptom → what it actually was → what fixed it.

### 2.1 Clients could not sync at all (`no compatible target` in the AI panel too)

**Symptom:** every client connected and immediately disconnected; new workspaces
were invisible on the second device; `DOC_NOT_FOUND` for a workspace's own root
document.

**Cause:** 0.27.5 replaced the room-based sync protocol with `space:join-batch`
and **deleted** the old handshake. Released clients (0.27.4 desktop, 0.27.1
mobile) therefore had their join refused — and the refusal was logged nowhere.

**Fix:** additive port of the legacy protocol (`9b9f22e`) plus a log line naming
the rejected version (`6912abd`). The root document only exists once a client can
push it, which is why the 404 was a symptom, not a separate bug.

### 2.2 Login failed with `INVALID_OAUTH_RESPONSE`

**Cause:** the native OAuth code required the `email_verified` claim; Synology
SSO advertises `aud, email, exp, groups, iat, iss, sub, username` and never sends
it.

**Fix:** `oauth.providers.oidc.trustUnverifiedEmail` (fail-closed by default; an
explicit `false` is still rejected) plus specific error reasons so the next case
is not diagnosed from one opaque string.

### 2.3 `redirect_uri` came out `http://` behind a TLS proxy

**Cause:** the origin helper followed the request **host** but hardcoded the
scheme from `server.https`, which describes the listener, not the client.

**Fix:** rank the scheme sources (`c9d5013`). Note what was *not* done: enabling
`trust proxy` would have made the framework trust client-supplied addresses and
silently weaken IP rate limiting.

### 2.4 The sign-in switches: six commits and four wrong turns

This is the most instructive sequence in the fork, so it is recorded in full.

| Round | Belief | Reality |
| --- | --- | --- |
| 1 | the image predates the change | the TypeScript code was in the image (`grep -c` on the bundle) and the env vars were injected — neither proved the value was *read* |
| 2 | where the value is written matters | the environment was skipped because the key was registered as native-owned while enforcement is in TypeScript |
| 3 | fixing ownership would settle it | that made the environment work and **broke the file**: the Node layer never reads `config.json` |
| 4 | a hybrid (native-owned + env seeding) fixes both | it fixed the file *and* the environment in the code, but the value still did not arrive |

**The actual cause:** the native runtime parses `config.json` and holds the value,
but only the `node_owned()` projection crosses to the Node layer — and
`auth.signInMethods.*` was not in that projection list. The value was parsed,
stored, and never handed over.

**What ended the loop:** a log line printing the resolved value
(`7100627`): `sign-in method policy: password=true magicLink=true oauth=true`
while the file asked for `false/false`. From that single line the gap was found
by reading `file.rs`.

**Lessons** (now rules R1, R2, R3, R5 in `DEVELOP_SKILL.MD`): ownership decides
the source; the projection decides delivery; and when a resolved value is
invisible from the wire, add the log line instead of another theory.

### 2.5 The service crash-looped on `crypto.privateKey`

**Cause:** enabling persistent BYOK armed a boot-time guard requiring a stable
key; the first value handed over was a random string, which the native accepts
but the Node layer does not (`1E08010C:DECODER routines::unsupported`).

**Fix:** generate a real EC P-256 PEM with the image's own Node, reuse the value
already in `app_configs` when present (a new key invalidates stored credentials),
and document that it must be a single JSON line with `\n` escapes.

### 2.6 CORS/WebSocket rejections from every entry point

**Cause:** `server.hosts` was not configured with the entry points actually used;
the list feeds the allowed-origin set. A URL-shaped entry would never have
matched (`http://http://host`).

**Fix:** config only — bare hosts, no scheme, and verify against the startup line
`Telemetry allowed origins updated: …`.

---

## 3. Architecture notes

- **Config ownership.** The environment and defaults are resolved by the Node
  layer (`getDefaultConfig`), then the native runtime's config is applied on top
  and finally the Node-side overrides. Native-owned keys are skipped by the first
  step — except those whose descriptor declares an env name (after `4b5a1c1`), so
  the environment can seed them while the file still wins.
- **Two native lists.** `NATIVE_APP_CONFIG_KEYS` (store/validate) and
  `node_owned()` (project to Node). A key both sides consume must be in both.
- **Two config paths.** Node prefers `/app/config.json`, falling back to
  `$HOME/.affine/config/config.json`; native follows
  `AFFINE_BACKEND_RUNTIME_CONFIG_PATH`. Point both at one file.
- **Sync rooms.** `sync-025` (0.25, one update per message), `sync-026`
  (0.26/0.27, batch), `sync-027` (0.27.5, in-memory subscriptions plus
  invalidation). Legacy peers are detected by room membership and authorized per
  request.
- **Versions.** A client's version is a build label; the sync gateway requires
  `>=0.27.5` for the batch protocol, and the served frontend reports the same
  label as the server, so relabelling an image is never a downgrade.

---

## 4. Verification manual

```sh
# Rust (native layer) — cargo may not be on PATH
CARGO_HOME=E:/AFFiNE/.cargo RUSTUP_HOME=$HOME/.rustup \
  CARGO_TARGET_DIR=E:/AFFiNE/.scratch/target \
  $HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/cargo.exe \
  check -p affine_server_native --lib

# TypeScript (server layer)
node_modules/.bin/tsc -p packages/backend/server/tsconfig.json --noEmit
node node_modules/oxlint/bin/oxlint <changed files>
```

Traps that produced wrong conclusions here:

1. A composite project with a stale `dist/tsconfig.tsbuildinfo` reports on the
   *previous* content — delete it and re-run when a result looks suspicious.
2. `tsc --listFiles | grep <file>` is the way to prove a file is actually
   compiled (one hit = in the program).
3. `grep -c` on a minified bundle counts lines, not occurrences.
4. Windows filesystems are case-insensitive: `rm NAME.MD` deletes `NAME.md`.
5. Without docker there is no local end-to-end: state that explicitly.

---

## 5. Working with the user

1. One problem at a time; the user picks the order, the rest stays parked in one
   line.
2. Hand over commands that decide something, with a result → conclusion table.
   `docker compose exec … psql …` the user can paste beats a screenshot request.
3. Verify the contract in the code *before* asking for a production change — most
   rework here came from assuming instead of reading.
4. Commit messages carry cause, blast radius, and a `Verified:` /
   `Not verified:` split.

---

## 6. Open items

- The sign-in switches have no automated test (the auth specs need a live
  database and the native module).
- `db.datasourceUrl` is native-owned but not projected; the server reads
  `DATABASE_URL` instead.
- The legacy sync bridge has no end-to-end verification against a real 0.27.4
  desktop / 0.27.1 mobile client.
- The auth switches' instance behaviour was still being confirmed when this
  record was written.
