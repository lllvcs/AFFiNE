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
| PDF export fonts | this change | fonts resolve against the deployment's own origin first; Sarasa Gothic CL subset shipped in `@affine/core/public/fonts`; missing fonts degrade with a warning instead of hanging |
| AI route diagnostics | this change | the `[affine-runtime:…]` prefix is stripped before matching, so route failures reach the user as actionable messages; every skipped BYOK profile is named |

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

### 2.7 PDF export died on fonts it was never allowed to fetch

**Symptom:** "Export to PDF" produced no file. The console showed five blocked
requests to `https://cdn.affine.pro/fonts/{Inter-Regular,Inter-SemiBold,
Inter-Italic,Inter-SemiBoldItalic}.woff` and `SarasaGothicCL-Regular.ttf`, each
`blocked by CORS policy: No 'Access-Control-Allow-Origin' header`, then
`Uncaught (in promise) TypeError: Failed to fetch (url: …)` from
`npm-async-pdfmake`. The UI notification never appeared — the promise simply
never settled.

**Cause:** the PDF adapter hardcoded absolute `cdn.affine.pro` URLs in
`pdfMake.fonts`. `pdfmake` fetches font entries over `XMLHttpRequest`, and the
CDN answers with `Access-Control-Allow-Origin` only for AFFiNE's own origins
(`curl -I -H 'Origin: https://app.affine.pro'` → header present;
`-H 'Origin: https://note.6868688.xyz'` → header absent, `vary: accept-encoding`
only). Every other consumer of those same files had already been switched to the
deployment's own `/fonts/` — the canvas fonts via
`environment.publicPath + 'fonts/'` in `view.ts`, the typst renderer via a local
fallback — so the exporter was the last one talking to the CDN.

Three more traps on the way:

* A SPA fallback answers `200 text/html` for `<host>/fonts/SarasaGothicCL-Regular.ttf`
  when the file does not exist. `response.ok` is therefore worthless as an
  existence probe; the payload's magic bytes have to be checked.
* `pdfMake.vfs` is `undefined` until something assigns it. Only `vfs_fonts` does,
  and the exporter never loads it, so pre-seeding the virtual file system (which
  is what makes the fonts cost one fetch per session instead of one per export)
  needs `pdfMake.vfs = { ...(pdfMake.vfs ?? {}), [url]: base64 }`.
* pdfkit's standard fonts are **not** available in the browser build: it reads
  their AFM metrics through the same virtual file system
  (`File 'data/Helvetica-Bold.afm' not found`), and those only ship with the Node
  build. "Fall back to Helvetica" is not an option here.

**Fix:** `blocksuite/affine/shared/src/adapters/pdf/pdf-fonts.ts` resolves each
font against `<publicPath>/fonts/` first and `cdn.affine.pro` second, verifies
the payload is a font, and caches the bytes in `pdfMake.vfs`. Missing CJK face →
Inter plus a warning; nothing loadable → `PdfFontsUnavailableError` listing every
URL that was tried. The CJK face itself is now shipped as a 2.45 MB TTF subset of
Sarasa Gothic CL in `packages/frontend/core/public/fonts/`. WOFF2 would have been
half the size but fontTools' WOFF2 writer applies the glyf/loca transform, which
the fontkit copy inside pdfmake 0.2.20 cannot decode (`TTFSubset.encode` →
`Cannot read properties of undefined (reading 'version')`), so TTF it is — see
`tools/pdf-fonts/README.md`.

### 2.8 The AI panel said `Internal Server Error` and `profiles=0` explained nothing

**Symptom:** a BYOK key was configured, the settings "test" button passed, the
coverage panel said *"AI chat is not covered: it needs a model that outputs text
and supports tool calling"*, and every chat turn failed with
`Internal Server Error: An internal error occurred. Identify: INTERNAL_SERVER_ERROR`.
The server log held the truth:

```
[affine-copilot] copilot route unavailable: no_compatible_target: slot=chat.default
  required={… "features":["tool_calling"] …} workspace=7bd8daf4-… deployment=selfhosted
  copilot.byok.enabled=true serverByok=true localByok=true profiles=0
  (no BYOK profile was loaded for this workspace…)
ERROR [Sse] internal_server_error ({"endpoint":"chat_object","stage":"stream"})
```

**Cause:** two independent defects, and the visible one hid the real one.

1. *The user-facing mapping never ran.* `runtime/error.rs` wraps every native
   error as `[affine-runtime:<code>] <message>`. `BackendRuntimeCoreOperations.measured`
   strips that prefix, but the copilot **stream** path bypasses it: the callback in
   `copilot-operations.ts` pushes `error.message` straight into the event stream,
   and `capability-runtime.ts` rebuilds an `Error` from it. `routeFailureReason`
   matches `message === reason || message.startsWith("reason:")`, which the
   prefixed string satisfies neither, so `mapNativeSemanticError` returned the
   error unchanged and the SSE layer turned it into `internal_server_error`. The
   `BadRequest` carrying the actionable guidance existed, was unit-tested, and was
   unreachable — the spec fed it the bare string.
2. *`profiles=0` had four possible meanings and said none of them.*
   `load_server_profiles` filtered `enabled = TRUE` in SQL, so a disabled key and
   a key stored in another workspace produced the identical diagnostic; and
   `load_local_profiles` dropped expired leases, workspace mismatches, disabled
   providers and policy rejections silently. The coverage panel made it worse by
   reporting a disabled key as "not covered — needs tool calling", which sends the
   reader after the wrong thing.

**Fix:** strip the prefix inside `nativeErrorMessage` (one place, covers every
caller), and have `load_profiles` collect a reason per branch — selecting
`enabled` instead of filtering on it — then log a single
`[affine-copilot] no BYOK profile was loaded (…): …` line when nothing came back.
The coverage string now also says the key must be enabled and that it only applies
to the workspace it was added in.

**Answer to the question it raised:** yes, chat requires tool calling. The
built-in `Chat With AFFiNE AI` prompt declares 15 tools, the native layer adds
`ModelFeature::ToolCalling` to `chat.default` whenever `!request.tools.is_empty()`,
and the "Workspace All Docs" switch in the chat input only filters eight of them
(`docSearch`, `docRead`, `docCanvasRead`, the four `frontend*` readers) — so
`artifactRead`, `artifactSearch`, `webSearch`, `docCompose` and `codeArtifact`
still make the requirement binding.

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

# Frontend / blocksuite (whole monorepo, builds project references first)
node_modules/.bin/tsc -b tsconfig.json
node_modules/.bin/vitest run --project @blocksuite/affine blocksuite/affine/all/src/__tests__/adapters/

# PDF fonts: does the instance serve them itself?
curl -sI https://<host>/fonts/Inter-Regular.woff2 | grep -i content-type
curl -sI -H 'Origin: https://<host>' https://cdn.affine.pro/fonts/Inter-Regular.woff | grep -i access-control

# AI route diagnostics: the mapping tests are ava specs (CI), see trap 8 for a
# local substitute.
node_modules/.bin/ava packages/backend/server/src/plugins/copilot/runtime/__tests__/

# Which BYOK keys exist, in which workspace, enabled?
docker compose exec postgres psql -U affine -c \
  "select id, workspace_id, provider, name, enabled from ai_workspace_byok_configs order by workspace_id, sort_order;"
```

Traps that produced wrong conclusions here:

1. A composite project with a stale `dist/tsconfig.tsbuildinfo` reports on the
   *previous* content — delete it and re-run when a result looks suspicious.
2. `tsc --listFiles | grep <file>` is the way to prove a file is actually
   compiled (one hit = in the program).
3. `grep -c` on a minified bundle counts lines, not occurrences.
4. Windows filesystems are case-insensitive: `rm NAME.MD` deletes `NAME.md`.
5. Without docker there is no local end-to-end: state that explicitly.
6. `tsc -p <package>` alone resolves cross-package imports through the referenced
   projects' **stale `dist`** and invents errors that are not real (and hides
   exports that are). Run `tsc -b tsconfig.json` before believing a frontend
   error — this is R11 in the other direction.
7. Tests that sort with `localeCompare` are locale-dependent: on this machine
   `Intl.DateTimeFormat().resolvedOptions().locale` is `zh-SG`, so
   `'入口'.localeCompare('duplicate') === -1` and
   `markdown.unit.spec.ts > … duplicate names` fails here while passing on CI.
8. The server package's `ava` cannot start **at all** in this checkout: its
   `require: ['./src/prelude.ts']` prelude is resolved against
   `node_modules/.cache/ava`, so every spec dies with `ERR_MODULE_NOT_FOUND`
   before running. To execute a server-side module here, build it
   (`tsc -b packages/backend/server/tsconfig.json`) and drive the compiled
   `dist/**` from a scratch script, or run the TypeScript through vitest with
   `unplugin-swc` configured for legacy decorators + metadata (the NestJS
   `@Field` decorators in `errors.gen.ts` need both; `tsx`/esbuild does not emit
   them).

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
