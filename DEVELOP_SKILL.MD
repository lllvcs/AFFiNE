---
name: affine-selfhosted-fork
description: Use when developing on the AFFiNE self-hosted fork (config, auth, sync, image pipeline) or when a self-hosted setting "does not work". Rules with their reason and verification step; the incident history lives in DEVELOP_EXPERIMENT.MD.
---

# AFFiNE self-hosted fork — development rules (agent-facing)

> **When to use this file.** Before changing configuration, auth, sync or the
> image pipeline in this fork; and when a setting "does not work" in a
> self-hosted deployment. Deployment and configuration *usage* live in
> [`README.md`](./README.md); the incident record behind these rules lives in
> [`DEVELOP_EXPERIMENT.MD`](./DEVELOP_EXPERIMENT.MD).
>
> Written as rules: **do X → because Y → verify with Z.** Each one is
> self-contained; read the rule you need, not the file.

- Baseline: upstream AFFiNE source at version `0.27.5`, branch `canary`
- Images: `ghcr.io/lllvcs/affine` (build artifact) → `lvcs/affine` (published)
- This machine has no docker and no old clients: protocol and end-to-end
  behaviour can only be verified on the deployment instance.

## Quick index

| Rule | Use when |
| --- | --- |
| R1 Register native keys in two lists | you add a config key the server also reads |
| R2 Ownership decides the source | a key works from env but not the file (or back) |
| R3 The Node layer never reads `config.json` | you expect TS code to see a file value |
| R4 One file for both readers | "half of the config applies" |
| R5 Log every skip and reject | a failure is invisible in every log |
| R6 Judge clients by their build label | a client cannot join or sync |
| R7 Port removed protocols additively | released clients must keep working |
| R8 Do not reach for `trust proxy` | generated URLs have the wrong scheme |
| R9 `server.hosts` takes bare hosts | CORS/WebSocket rejections |
| R10 `crypto.privateKey` has two consumers | the server crash-loops on a key |
| R11 Trust nothing until it is re-run | a typecheck or grep looks too clean |
| R12 Declare what you did not verify | before writing "done" |

---

## R1. Register a native-owned key in **two** lists

**Do.** When a key is owned by the native runtime *and* read by the server, add
it to both:

| List | File | Meaning |
| --- | --- | --- |
| `NATIVE_APP_CONFIG_KEYS` | `packages/backend/native/src/runtime/config/store.rs` | the native runtime stores and validates it |
| `node_owned()` | `packages/backend/native/src/runtime/config/file.rs` | it is projected to the Node layer |

**Because.** The native parses `config.json` and holds the value, but only the
projection crosses the native→Node boundary. Register it in the first list only
and `config.json` becomes silently inert for the server: parsed, held, never
handed over. `auth.signInMethods` was exactly this and took four rounds to find.

**Verify.** `grep -n "yourKey" <both files>` and then, on the instance, the
startup/preflight log line that prints the resolved value.

---

## R2. Ownership decides which source works

**Do.** Choose the owner by *who enforces the key*: enforcement in the server →
it is a Node-layer (TS) key; enforcement in the runtime → native key. When both
sources must work, keep it native-owned and make sure the environment can still
seed it.

**Because.** `getDefaultConfig(excludedKeys)` skips exactly the native-owned
keys, so their environment variables are never read; TS-owned keys can only ever
come from the environment or the defaults (R3). Moving a key between the layers
therefore trades the mounted file for the environment — every flip costs the user
a restart round.

**Verify.** Set the same value in `config.json` *and* as an env var; the file
wins. Then unset the file value and confirm the env still works.

---

## R3. The Node layer never reads `config.json`

**Do.** Do not expect a TS-reachable config item to pick up a value from the
mounted file. Pass it through the environment, or make the runtime project it
(R1).

**Because.** `OVERRIDE_CONFIG_TOKEN` is only ever populated by code
(`ConfigModule.override()`); `env.ts` opens the file solely to construct the
native handle. The file reaches the server *only* through the native's
projection.

**Verify.** `grep -rn "OVERRIDE_CONFIG_TOKEN" packages/backend/server/src`.

---

## R4. Keep both readers on the same file

**Do.** Mount one file at `/app/config.json` **and** point
`AFFINE_BACKEND_RUNTIME_CONFIG_PATH` at the same path.

**Because.** The Node reader prefers `/app/config.json` and falls back to
`$HOME/.affine/config/config.json`; the native reader only follows its env
variable. When they disagree you get "half the config applies" — some sections
work, others are silently on defaults, and the startup banner looks healthy.

**Verify.** `docker compose exec <svc> sh -c 'printenv | grep RUNTIME_CONFIG'`
and `ls -la /app/config.json`.

---

## R5. Log every skip and reject path

**Do.** Any branch that rejects a client, drops a config row or returns a
"no compatible target"-style error gets a log line naming the input it judged.
When the resolved value of a config item is invisible from the wire, log it at
the branch that reads it.

**Because.** These paths used to be silent: a rejected WebSocket join only
showed as "connects, disconnects"; a skipped BYOK profile vanished. The line
`sign-in method policy: password=… magicLink=… oauth=…` ended four rounds of
guessing in one paste. Keep the machine-readable code as the leading token of a
failure string and append context after it — and grep every consumer of that
string first.

**Verify.** Trigger the path once and confirm the line appears with the values
you expect.

---

## R6. Judge a client by its build label, not by the source

**Do.** To find out which protocol a client speaks, read its self-reported
version (`x-<app>-version` header, join payload) and the server's floor. Never
"downgrade" an image by relabelling the build.

**Because.** The client version is a build-time label: relabelling does not
change the code a client runs, and when the server serves the frontend too, the
browser reports the same label — so relabelling an image to match old clients
breaks the browser client that was working. It also misled this investigation
for two rounds.

**Verify.** Ask for the join frame's `clientVersion` or the request header, not
for the image tag.

---

## R7. Port a removed protocol additively

**Do.** When released clients must keep working across a protocol change: fetch
the old release's own source at its tag as the reference, add the legacy
handlers *next to* the new ones, detect legacy peers by a marker the new path
already sets (room membership), keep the old per-request authorization for them,
fan output out to both paths, and log an acceptance line.

**Because.** 0.27.5 replaced the room protocol with `space:join-batch` and
**deleted** the old handlers, so every released client (0.27.4 desktop, 0.27.1
mobile) was rejected on join — a socket that opens and closes, no sync, and a
workspace root document that never gets pushed. Lowering the floor alone would
only convert a loud rejection into a silent no-sync.

**Verify.** The instance log must show the acceptance line with the client's own
version; a protocol layer cannot be verified without a real old client.

---

## R8. Never fix URL schemes with `trust proxy`

**Do.** For client-facing URLs (OAuth `redirect_uri`, email links), rank the
sources: administrator-configured canonical URL for its own host → the request's
scheme (forwarded-proto) and only for hosts the administrator listed → the
listener flag (this last branch is what background jobs and the existing specs
rely on).

**Because.** The listener flag describes how the *server* listens, not how the
*client* reached it; a TLS-terminating proxy with the flag legitimately `false`
produced `http://` callbacks. Enabling `trust proxy` to read the forwarded header
would also make the framework trust a client-supplied address and silently
disable IP rate limiting.

**Verify.** Preflight the OAuth flow from both entry points and read the
`redirect_uri` in the response.

---

## R9. `server.hosts` takes bare hosts

**Do.** Write `["100.64.0.1", "nas.local:3010"]` — no scheme; the scheme comes
from `server.https`, and the port is appended automatically only for `localhost`
and bare IPs.

**Because.** A URL-shaped entry is concatenated into `http://http://host` and can
never match a request origin. This list is also the CORS/WebSocket allow-list, so
the symptom is a rejected realtime connection, not a config error.

**Verify.** The startup line `Telemetry allowed origins updated: …` must list
every entry point you use.

---

## R10. `crypto.privateKey` has two consumers

**Do.** Generate it in the **strictest** consumer's format — a real EC P-256 PEM
— using the image's own Node:
`generateKeyPairSync('ec', { namedCurve: 'prime256v1' })`, exported as
`pkcs8/pem`, pasted as a single JSON line with `\n` escapes. **Reuse the existing
value** when the database still has one (`select value from app_configs where id
= 'crypto.privateKey'`); warn that a new key invalidates stored credentials.

**Because.** The runtime only requires a non-empty, stable string (a KDF root),
but the Node layer parses the same value with `createPrivateKey()` and crash-loops
on a random string (`error:1E08010C:DECODER routines::unsupported`).

**Verify.** The service starts and the log prints `recognized as …` for your
`server.externalUrl`.

---

## R11. Trust nothing until it has been re-run

**Do.** After editing a widely referenced type or list, delete
`packages/backend/server/dist/tsconfig.tsbuildinfo` and re-run
`node_modules/.bin/tsc -p packages/backend/server/tsconfig.json --noEmit`. Prove a
file is really compiled with
`tsc -p … --noEmit --listFiles | grep <file>` (one hit = it is in the program).
When cargo is not on PATH, call the toolchain directly with explicit caches:
`CARGO_HOME=… RUSTUP_HOME=… CARGO_TARGET_DIR=… ~/.rustup/toolchains/*/bin/cargo.exe check -p affine_server_native --lib`.

**Because.** A composite project with a stale build info file reports results for
the previous content (both false red and false green). `grep -c` on a minified
bundle counts *lines*, not occurrences, so "2" proves nothing until you check
what it matched. Windows filesystems are also case-insensitive: `rm foo.MD` can
delete `foo.md`. 

**Verify.** Re-run the check after the edit, not before.

---

## R12. Declare what you did not verify

**Do.** End every change with two lists: what was executed here (typecheck,
lint, cargo check) and what can only happen on the instance (protocol, OIDC
round trip, client compatibility). Put both in the commit message.

**Because.** This environment cannot run docker. "Should work" has been wrong
more than once; the user can only test what you tell them to test.

---

## Pre-commit checklist

1. `cargo check -p affine_server_native --lib` (clean).
2. `tsc -p packages/backend/server/tsconfig.json --noEmit` (clean, build info fresh).
3. `oxlint <changed files>`.
4. New native key? Both lists updated (R1) and the source precedence decided (R2).
5. New reject/skip path? A log line names the input (R5).
6. Commit message: cause, blast radius, `Verified:` and `Not verified:`.

## Open items

- The sign-in switches have **no automated test** (the auth specs need a live
  database and the native module). Add controller-level cases once the behaviour
  is confirmed on the instance.
- `db.datasourceUrl` is native-owned but **not projected**; the server reads
  `DATABASE_URL`. Project it if the database URL should be settable from the
  file (it is a secret — the projection does not filter those).
- The legacy sync bridge has **no end-to-end verification** (needs a real 0.27.4
  desktop and 0.27.1 mobile client each).
