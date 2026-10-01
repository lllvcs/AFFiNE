# AFFiNE — self-hosted fork

[中文说明 / Chinese README](./README.zh-CN.md)

This repository is a **source-level fork of [AFFiNE](https://github.com/toeverything/AFFiNE)**,
kept for running AFFiNE on your own hardware. Upstream ships a great editor but
optimises for its cloud; a self-hosted instance has different constraints, and
several of them are hard blockers rather than preferences (a server that only
speaks to unreleased clients, an SSO provider that never publishes
`email_verified`, quotas that were never meant to be reachable on your own box).

Everything below is what this fork changes **on top of the upstream source**, how
to deploy it, and which configuration actually does something. It is written to
be enough on its own — you should not need to read the source to get an instance
running.

- Upstream project: <https://github.com/toeverything/AFFiNE>
- Fork: <https://github.com/lllvcs/AFFiNE> (branch `canary`)
- Images: `ghcr.io/lllvcs/affine` (GHCR) · `lvcs/affine` (Docker Hub)

---

## 1. What this fork changes

### 1.1 Self-hosting limits are actually lifted

Upstream keeps cloud quotas in the code paths a self-hosted instance also runs
through, so a personal deployment can hit seat, storage or history limits that
make no sense there. This fork raises them for self-hosted deployments: blob
limit 10 GiB, storage quota 2^53−1, seat limit `i32::MAX` and a history period of
100 years, with the `unlimitedCopilot` / `copilotByok` flags available.

*Commits:* `c5844b1` (and follow-ups).

### 1.2 Copilot: BYOK model listing and diagnoseable routing failures

* **List models from your own provider.** BYOK profiles can query the provider's
  model list instead of forcing you to type model ids by hand (`c5844b1`).
* **"No compatible model" is no longer a dead end.** AFFiNE's chat always sends
  tools (workspace search / reading docs), so the chat route requires a model that
  declares `tool_calling` — a fact the old error message never mentioned. Native
  routing failures now carry the slot, the required capabilities, the workspace,
  the deployment type and the BYOK state, and the server turns the native
  semantic errors into actionable HTTP errors (`a16e5f3`). The frontend's BYOK
  coverage indicator also states that chat needs text output **and** tool calling,
  in both English and Chinese (`4d1f8b8`).
* Profiles silently skipped by policy are now logged on the native side instead
  of disappearing without a trace (`a16e5f3`).

### 1.3 OIDC: providers that never publish `email_verified`

Small self-hosted identity providers — Synology SSO is the example this fork was
built against — advertise `claims_supported: aud, email, exp, groups, iat, iss,
sub, username`. They never send `email_verified`, and the strict check rejected
every login with `INVALID_OAUTH_RESPONSE`.

`oauth.providers.oidc.trustUnverifiedEmail` (default `false`, fail-closed) trusts
the address **when the claim is absent**. An explicit `email_verified: false` is
still rejected, `args.claim_email_verified` remaps the claim name, and the OAuth
error reasons are now specific (`missing_id_token`, `missing_email_verified_claim`,
`email_not_verified`, `userinfo_subject_mismatch`, `missing_subject`,
`missing_email`, `id_token_*`) instead of one opaque string. Three Rust unit tests
cover it (`42bbe52`).

### 1.4 Realtime sync works with released clients

AFFiNE 0.27.5 replaced the room-based sync protocol with `space:join-batch` and
**deleted the older handshake**. The practical result: a server built from that
source rejects every released client — the 0.27.4 desktop app and the 0.27.1
mobile app — with a WebSocket that connects and immediately disconnects, no sync,
and a workspace root document that never gets pushed (which shows up as
`DOC_NOT_FOUND` for a brand-new workspace).

This fork re-adds the legacy protocol **next to** the batch one (`9b9f22e`):

* `space:join`, `space:join-awareness` and `space:leave-awareness` are handled
  again, with the `>=0.25.0` floor and the `sync-025` / `sync-026` room split.
* Doc updates are broadcast to both the legacy rooms and the new per-document
  path, so old and new clients see each other's edits.
* Legacy sockets are authorized per request (`assertDocActionAllowed`), as 0.27.4
  did, because they hold no in-memory document subscriptions.
* A rejected join now logs the client version and the floor it failed
  (`6912abd`), and an accepted legacy join logs
  `Legacy sync join accepted: client=… version=… protocol=…`. This class of
  failure used to be invisible in every log and every client.

### 1.5 Each sign-in method is switchable

Upstream has no way to turn off email+password or magic-link sign-in:
`auth.allowSignup` is declared but never enforced, and the login page offers
methods the server will accept. `auth.signInMethods.{password,magicLink,oauth}`
(default: all `true`) fixes that (`28ac1be`):

* the sign-in endpoint **refuses** a disabled method (both the password and the
  magic-link branch of `POST /api/auth/sign-in`, plus `POST /api/auth/magic-link`);
* the login preflight reports it as `available: false`, which is what the client
  already reads, so the login page stops offering it without any client change;
* with `oauth: false` the OAuth endpoints refuse to start a flow and the resolver
  stops advertising providers.

See [§3.3](#33-oidc-only-instance) for the OIDC-only recipe.

### 1.6 Self-hosting documentation

`server.hosts` takes **bare hosts, not URLs**; the scheme comes from
`server.https` and the port is appended automatically only for `localhost` and
bare IPs (`9bb4520`). Getting this wrong is the usual cause of
`Blocked CORS request` / `Blocked WebSocket CORS request` and non-working
realtime sync — see [§6](#6-troubleshooting).

### 1.7 Image builds: build and publish are separate steps

The image pipeline was reworked so that building and publishing can be done
independently, and so that `latest` can never point at something that is not an
image:

* `Build Images` (`build-images.yml`) produces the image and pushes it to GHCR
  only; that is the hand-off artifact.
* `Publish Docker Image` (`docker-publish.yml`) is manual: it copies the GHCR
  image to Docker Hub **registry-to-registry** and moves `latest` / `<channel>` /
  `<version>` on both registries. Nothing is rebuilt, so publishing a
  multi-gigabyte multi-platform image takes seconds.
* The self-hosted native builds embed the Pro **public** key, and the build
  fails loudly if it cannot be resolved, instead of producing an image that
  cannot verify licences (`fdf8f1b`).

*Commits:* `b4781c0`, `24f5675`, `d7d383b`, `d96b436`, `fa43b17`, `a49759d`,
`4c85772`, `e1bcbd4`, `fdf8f1b`.

---

## 2. Deploy

### 2.1 docker compose

```yaml
services:
  affine:
    image: ghcr.io/lllvcs/affine:latest   # or lvcs/affine:latest from Docker Hub
    restart: unless-stopped
    ports:
      - '3010:3010'
    volumes:
      - ./config/config.json:/app/config.json:ro   # see §2.2
      - ./storage:/root/.affine/storage
    environment:
      - AFFINE_BACKEND_RUNTIME_CONFIG_PATH=/app/config.json
    depends_on:
      - postgres
      - redis

  postgres:
    image: postgres:16
    restart: unless-stopped
    volumes:
      - ./postgres:/var/lib/postgresql/data
    environment:
      - POSTGRES_USER=affine
      - POSTGRES_PASSWORD=change-me
      - POSTGRES_DB=affine

  redis:
    image: redis:7
    restart: unless-stopped
    volumes:
      - ./redis:/data
```

The image runs its own migrations on start (`affine_migration_job`), so a fresh
database is initialised on the first boot.

### 2.2 Where the configuration file has to live

This is the single most confusing part of self-hosting AFFiNE. There are **two
readers** with **two different paths**:

| Reader | Path it reads |
| --- | --- |
| Node server (TS) | `/app/config.json` first, otherwise `$HOME/.affine/config/config.json` (in the image `$HOME` is `/root`) |
| Native runtime (Rust) | the file named by `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` |

If the two disagree you get a server that logs changes as applied while half of
them stay inert, and settings such as `server.hosts` never reach the CORS
allow-list. **Mount one file at `/app/config.json` and point
`AFFINE_BACKEND_RUNTIME_CONFIG_PATH` at the same file** — that is what the
compose above does.

### 2.3 `crypto.privateKey` is required once BYOK is on

`copilot.byok.enabled: true` (persistent BYOK) requires a **stable** private key,
otherwise the server refuses to start:

```
[affine-runtime:invalid_state] stable crypto.privateKey is required when persistent BYOK is enabled
```

The value must be a real **EC P-256 private key in PEM (PKCS#8)** form — a random
string fails with `error:1E08010C:DECODER routines::unsupported`, because the
Node server parses it with `createPrivateKey()`. Generate one with the image's
own Node:

```sh
docker compose exec affine node -e "
const {generateKeyPairSync}=require('crypto');
const {privateKey}=generateKeyPairSync('ec',{namedCurve:'prime256v1'});
console.log(JSON.stringify({crypto:{privateKey:privateKey.export({format:'pem',type:'pkcs8'}).toString()}},null,2));
"
```

Paste the printed `crypto` block into `config.json` (the `\n` escapes are part of
the JSON string — keep it on one line) and restart.

> **Note.** This key also encrypts the stored BYOK API keys (HKDF-derived
> envelope). Changing it makes previously stored credentials undecryptable — you
> will have to enter the API keys again. If an older key is still readable in
> your database, prefer it:
> `select value from app_configs where id = 'crypto.privateKey';`

### 2.4 First-run checklist

1. `server.externalUrl` set to the URL you actually browse (e.g.
   `https://note.example.com`).
2. Every entry point listed in `server.hosts` (Tailscale IP, LAN IP, reverse
   proxy hostname) — bare hosts, no scheme, with the port only for hostnames.
3. `crypto.privateKey` present if BYOK is enabled (§2.3).
4. OIDC provider configured and a login tested **before** you disable the other
   sign-in methods.
5. Server reachable and the log line
   `Telemetry allowed origins updated: …` contains each entry point you use.

---

## 3. Configuration reference

### 3.1 `config.json`

Every key below is read by this fork; keys marked *(native)* are validated by the
native runtime and can also come from the environment variable in §3.2.

```jsonc
{
  "$schema": "https://github.com/toeverything/affine/releases/latest/download/config.schema.json",
  "deployment": { "type": "selfhosted" },

  "server": {
    "name": "AFFiNE",
    "externalUrl": "https://note.example.com",
    "https": false,
    "host": "localhost",                                    // env AFFINE_SERVER_HOST
    "hosts": ["100.64.0.1", "nas.local:3010"],             // config.json only
    "port": 3010,                                           // env AFFINE_SERVER_PORT
    "listenAddr": "0.0.0.0",                                // env LISTEN_ADDR
    "path": ""                                              // env AFFINE_SERVER_SUB_PATH
  },

  "crypto": { "privateKey": "-----BEGIN PRIVATE KEY-----\n…\n-----END PRIVATE KEY-----\n" },

  "oauth": {
    "providers": {
      "oidc": {
        "issuer": "https://idp.example.com/webman/sso",
        "clientId": "…",
        "clientSecret": "…",
        "allowPrivateNetwork": false,        // reach an IdP on a private network
        "trustUnverifiedEmail": false,       // §1.3
        "args": { "scope": "openid email", "claim_email_verified": "email_verified" }
      }
    }
  },

  "auth": {
    "allowSignup": true,
    "allowSignupForOauth": true,
    "requireEmailDomainVerification": false,
    "newAccountActionDelay": 0,
    "signInMethods": { "password": true, "magicLink": true, "oauth": true },  // §1.5
    "session": { "ttl": 2592000, "ttr": 86400 },
    "token": { "accessTokenTtl": 3600, "refreshIdleTtl": 2592000,
               "refreshAbsoluteTtl": 31536000, "refreshGracePeriod": 30,
               "refreshRetention": 2592000 }
  },

  "copilot": {
    "enabled": true,
    "byok": {
      "enabled": true,
      "allowCustomEndpoint": true,      // required for any non-official endpoint
      "allowPrivateEndpoint": true,
      "allowedProviders": []            // empty = allow all
    }
  },

  "indexer": {
    "enabled": false,
    "provider": { "type": "embedded", "endpoint": "", "apiKey": "", "username": "", "password": "" }
  },

  "redis": { "host": "redis", "port": 6379, "username": "", "password": "", "db": 0 },
  "storages": { "avatar": { "storage": {} }, "blob": { "storage": {} } },
  "payment": { "enabled": false }
}
```

`db.datasourceUrl` (`DATABASE_URL`) and `mailer.*` (`MAILER_*`) are configured
through the environment in the official images; `mailer.*` is only needed if you
want magic-link email.

### 3.2 Environment variables

| Variable | Maps to | Notes |
| --- | --- | --- |
| `AFFINE_SERVER_EXTERNAL_URL` | `server.externalUrl` | Base URL used to generate links and the allowed-origin list |
| `AFFINE_SERVER_HOST` | `server.host` | Single host, default `localhost` |
| `AFFINE_SERVER_PORT` | `server.port` | Default `3010` |
| `AFFINE_SERVER_HTTPS` | `server.https` | Boolean, default `false` |
| `AFFINE_SERVER_SUB_PATH` | `server.path` | For sub-path deployments, e.g. `/affine` |
| `LISTEN_ADDR` | `server.listenAddr` | Default `0.0.0.0` |
| `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` | — | Path to the JSON config read by the native runtime (§2.2) |
| `AFFINE_PRIVATE_KEY` | `crypto.privateKey` | PEM key from §2.3 |
| `AFFINE_AUTH_SIGN_IN_PASSWORD` | `auth.signInMethods.password` | Boolean: `1`/`true` = on, anything else = off |
| `AFFINE_AUTH_SIGN_IN_MAGIC_LINK` | `auth.signInMethods.magicLink` | idem |
| `AFFINE_AUTH_SIGN_IN_OAUTH` | `auth.signInMethods.oauth` | idem |
| `DATABASE_URL` | `db.datasourceUrl` | PostgreSQL connection string |
| `REDIS_SERVER_HOST` / `_PORT` / `_DATABASE` / `_USERNAME` / `_PASSWORD` | `redis.*` | |
| `MAILER_HOST` / `_PORT` / `_USER` / `_PASSWORD` / `_SENDER` / `_SERVERNAME` / `_IGNORE_TLS` | `mailer.*` | Magic-link email |
| `GA4_MEASUREMENT_ID`, `GA4_API_SECRET` | telemetry | Optional |

`server.hosts` has **no** environment variable — it exists only in
`config.json`. Some environment variables you may have seen in other
compose files (for example `AFFINE_INDEXER_ENABLED`) are not wired to anything
in this codebase; configure those keys in `config.json` instead.

For the boolean switches above, `1`/`true` (case-insensitive) means enabled and
anything else means disabled. If a switch is also present in `config.json`, the
file wins — the environment only fills in values the file does not set.

### 3.3 OIDC-only instance

The usual reason to run SSO at all: no local passwords, no magic links, one
identity provider.

```json
{
  "auth": {
    "signInMethods": { "password": false, "magicLink": false, "oauth": true }
  }
}
```

Or, without touching the file:

```sh
AFFINE_AUTH_SIGN_IN_PASSWORD=false AFFINE_AUTH_SIGN_IN_MAGIC_LINK=false AFFINE_AUTH_SIGN_IN_OAUTH=true
```

> ⚠️ **Do not lock yourself out.** Sign in through OIDC first and confirm the
> account has admin access; afterwards there is no other way in. Keep a copy of
> the previous `config.json` so you can roll back by restarting the container.

The login page still shows the email field, because method availability is
resolved per email address; entering one reports that the address cannot sign in.
The OIDC buttons are unaffected.

---

## 4. Images, tags and publishing

| Registry | Image |
| --- | --- |
| GHCR | `ghcr.io/lllvcs/affine` |
| Docker Hub | `lvcs/affine` |

Tags: `latest` (newest published), the channel tag (`canary`), the version tag
(`0.27.5`) and per-build tags containing the short commit hash
(e.g. `canary-d96b436`).

Publishing is deliberately two manual steps:

1. run **Build Images** (`build-images.yml`; also reachable through
   `docker-build.yml`) — builds and pushes to GHCR. Inputs include
   `build-type`, `app-version`, `git-short-hash`, `image-namespace`,
   `platforms`, `build-admin`, `build-mobile`.
2. run **Publish Docker Image** (`docker-publish.yml`) with `source-tag`
   (defaults to the newest successful build), `dockerhub` and `moving-tags`.

`latest` only ever moves in step 2 and only onto an image produced by step 1, so
it cannot end up pointing at a non-image artifact.

> ⚠️ **Do not lower the build version number below the source version.** The web
> client is built from this source and reports the build version to the sync
> gateway, which requires `>=0.27.5` for the batch protocol. Relabelling an image
> as `0.27.4` therefore breaks the browser client (its WebSocket join is
> rejected) even though the source is unchanged.

---

## 5. Upgrading

1. `docker compose pull` (or rebuild an image for your own changes).
2. `docker compose up -d` — migrations run automatically.
3. Watch the first ~30 lines of the server log: the migration job must finish and
   the server must print `recognized as …` for your `server.externalUrl`.

Changes to `config.json` are picked up on restart; environment variables always
need a restart.

---

## 6. Troubleshooting

**Realtime sync never starts, `Blocked CORS request` / `Blocked WebSocket CORS
request` in the log.** The allowed-origin list comes from `server.externalUrl`,
`server.host`, `server.hosts` and `server.port`. Add the exact origin you browse
(bare host entries, no scheme) and confirm the startup line
`Telemetry allowed origins updated: …` lists it.

**A client connects and immediately disconnects, `Rejected WebSocket join …`.**
The gateway refuses clients below the protocol floor. Since `9b9f22e` the
supported range is `>=0.25.0` for the legacy handshake and `>=0.27.5` for
`space:join-batch`; the log line names the version the client sent. Released
clients (0.27.1 mobile, 0.27.4 desktop) use the legacy path and log
`Legacy sync join accepted`.

**A fresh workspace shows `DOC_NOT_FOUND` (`Doc <id> under Space <id> not
found`).** The workspace root document is created by a client and pushed over
sync; if the socket cannot join, it never lands. Fix the sync path above, then
re-open the workspace (or create a new one) with a working client.

**AI chat fails with `no_compatible_target`.** Chat always sends tools, so the
route needs a model declaring `tool_calling`; a model with only text output is
rejected. Either declare tool calling for the model in your gateway, or turn off
the tools that chat enables by default (workspace search, reading docs). Since
`a16e5f3` the error names the required capability instead of only the reason.
Note that the BYOK "test" button probes your provider directly and does **not**
run the routing decision, so a green test does not prove that chat will work.

**`affine_server` restarts in a loop.** Read the error: a missing or malformed
`crypto.privateKey` produces `stable crypto.privateKey is required …` or
`1E08010C:DECODER routines::unsupported`. See §2.3.

**Config edits seem to have no effect.** Two readers, two paths — see §2.2.

---

## 7. Development

The fork's own changes live in a small number of places:

| Area | Path |
| --- | --- |
| Native runtime config, sync gateway, auth | `packages/backend/native/src/runtime/**`, `packages/backend/server/src/core/sync/gateway.ts` |
| Auth HTTP surface | `packages/backend/server/src/core/auth/{controller,service,config}.ts` |
| OAuth/OIDC | `packages/backend/server/src/plugins/oauth/*`, `packages/backend/native/src/runtime/backend_runtime/auth_session/*` |
| Copilot diagnostics | `packages/backend/native/src/runtime/backend_runtime/copilot/*`, `packages/backend/server/src/plugins/copilot/runtime/native-errors.ts` |
| Frontend sign-in / BYOK | `packages/frontend/core/src/components/sign-in/*`, `.../setting/workspace-setting/byok/*` |
| Config plumbing | `packages/backend/server/src/base/config/*` |
| Image pipeline | `.github/workflows/*.yml` |

Verification commands (all runnable from the repository root):

```sh
# native (Rust) — add your rustup toolchain path if cargo is not on PATH
cargo check -p affine_server_native --lib

# server (TypeScript)
node_modules/.bin/tsc -p packages/backend/server/tsconfig.json --noEmit

# lint
node node_modules/oxlint/bin/oxlint <changed files>

# generated GraphQL/native bindings, when the schema changes
yarn affine <task>
```

Backend tests use `ava` and need PostgreSQL, Redis and the built native module:
`yarn workspace @affine/server test`. Changes that touch the sync protocol or the
auth surface deserve a test against a real client, because the pieces they
interact with (socket rooms, permissions, client versions) cannot be reproduced
in a unit test.

---

## 8. Upstream and licence

This fork tracks upstream AFFiNE and keeps its licence: see
[`LICENSE`](./LICENSE) and [`LICENSE-MIT`](./LICENSE-MIT). All credit for AFFiNE
belongs to [TOEVERYTHING PTE. LTD. and its contributors](https://github.com/toeverything/AFFiNE).
Changes made here are the ones listed in [§1](#1-what-this-fork-changes) — patches
that exist to make a self-hosted deployment work, and are offered back in the
hope they are useful.
