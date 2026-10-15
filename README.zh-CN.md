# AFFiNE — 自托管分支（fork）

[English README](./README.md)

本仓库是 **[AFFiNE](https://github.com/toeverything/AFFiNE) 的源码级分支**，目的是把 AFFiNE 跑在自己的机器上。上游的编辑器做得很好，但它的优化目标是自家云服务；自托管场景的约束不一样，其中几条是**硬阻塞**而不是偏好——比如一个只肯跟未发布客户端说话的同步协议、一个从不返回 `email_verified` 的 SSO 提供方、以及本来就不该出现在自己机器上的各种配额。

下面写的是：这个分支**在上游源码之上**改了什么、怎么部署、以及哪些配置真的会生效。目标是这份文档本身够用——不需要读源码就能把实例跑起来。

- 上游项目：<https://github.com/toeverything/AFFiNE>
- 本分支：<https://github.com/lllvcs/AFFiNE>（分支 `canary`）
- 镜像：`ghcr.io/lllvcs/affine`（GHCR）· `lvcs/affine`（Docker Hub）

---

## 1. 本分支相对上游的改动

### 1.1 真正放开自托管配额限制

上游把云服务的配额判断留在自托管同样会走到的代码路径里，于是个人部署会撞到坐席数、存储、历史版本等限制。本分支为自托管抬高这些上限：blob 限额 10 GiB、存储配额 2^53−1、坐席上限 `i32::MAX`、历史保留 100 年，并提供 `unlimitedCopilot` / `copilotByok` 开关。

*提交：* `c5844b1`（及后续修补）。

### 1.2 Copilot：BYOK 模型列表与可诊断的路由失败

* **直接列出自己提供方的模型**：BYOK 配置可以查询提供方的模型列表，不必手打模型 id（`c5844b1`）。
* **`no_compatible_target` 不再是死胡同**：AFFiNE 的对话**总是带 tools**（工作区检索、读取文档），因此对话路由要求模型声明 `tool_calling`——这一点旧报错从未提及。现在原生层的路由失败会带上槽位、所需能力、工作区、部署类型与 BYOK 状态，服务端把原生语义错误映射成可操作的 HTTP 错误（`a16e5f3`）；前端的 BYOK 覆盖提示也明确写出"对话需要文本输出**且**支持工具调用"，中英文都有（`4d1f8b8`）。
* 被策略静默跳过的 BYOK profile 现在会在原生层打日志，而不是无声消失（`a16e5f3`）。

### 1.3 OIDC：提供方从不返回 `email_verified`

小型自建身份提供方——本分支就是为群晖 SSO 写的——其 discovery 声明 `claims_supported: aud, email, exp, groups, iat, iss, sub, username`，**从不返回 `email_verified`**，于是严格的校验把每次登录都拒成 `INVALID_OAUTH_RESPONSE`。

`oauth.providers.oidc.trustUnverifiedEmail`（默认 `false`，失败即关闭）在**该 claim 完全缺失**时信任邮箱地址；显式 `email_verified: false` 仍然拒绝，`args.claim_email_verified` 可以重映射 claim 名。同时 OAuth 的错误原因细化了（`missing_id_token`、`missing_email_verified_claim`、`email_not_verified`、`userinfo_subject_mismatch`、`missing_subject`、`missing_email`、`id_token_*`），不再是一句笼统的话。附 3 个 Rust 单元测试（`42bbe52`）。

**`redirect_uri` 现在跟随请求**，而不是只看 `server.https`：当请求主机与 `server.externalUrl` 的主机一致时，用 externalUrl 的协议（它由管理员配置，客户端无法影响）；其它主机则用客户端**实际使用的协议**（取自 `x-forwarded-proto`），没有请求上下文时回退到 `server.https`——并且只对 `server.hosts` 里列出的主机生效。这样 TLS 终止在反代上的实例可以保持 `server.https: false`（内网走明文 HTTP、不必在内网配证书），而 OIDC 回调仍然是 `https://`。这也是**不能**打开 `trust proxy` 的原因：那会让每个客户端自行申报来源地址，从而绕过登录限流。

### 1.4 实时同步兼容已发布客户端

AFFiNE 0.27.5 用 `space:join-batch` 取代了基于房间的同步协议，并**删除了旧握手**。后果很实在：用这份源码构建的服务端会拒绝所有已发布的客户端——0.27.4 桌面版、0.27.1 手机版——表现为 WebSocket 连上就断、完全无法同步，工作区根文档永远推不上去（在新建工作区上表现为 `DOC_NOT_FOUND`）。

本分支把旧协议**并行**加回来了（`9b9f22e`）：

* `space:join`、`space:join-awareness`、`space:leave-awareness` 重新有处理器，门槛 `>=0.25.0`，并按版本分到 `sync-025` / `sync-026` 房间；
* 文档更新同时广播到旧房间和新客户端的 per-doc 通道，老客户端与新客户端能互相看到编辑；
* 老连接按**每请求直接鉴权**（`assertDocActionAllowed`），与 0.27.4 一致——因为它们没有批量协议的内存态订阅；
* join 被拒时会打印客户端版本与门槛（`6912abd`），被接受时打印 `Legacy sync join accepted: client=… version=… protocol=…`。这类故障以前在任何日志和任何客户端里都看不见。

### 1.5 每种登录方式都可单独开关

上游没有关闭"邮箱+密码"或"魔法链接"的办法：`auth.allowSignup` 只有描述符、没有强制点，登录页会照常展示服务端其实会接受的方式。`auth.signInMethods.{password,magicLink,oauth}`（默认全 `true`）解决了这件事（`28ac1be`）：

* 登录接口**直接拒绝**已关闭的方式（`POST /api/auth/sign-in` 的密码分支与 magic-link 分支，以及 `POST /api/auth/magic-link`）；
* 登录预检把它报成 `available: false`，而客户端本来就在读这个字段，因此**前端零改动**就不会再提供该方式；
* `oauth: false` 时 OAuth 端点拒绝发起流程，解析器也不再广告任何提供方。

仅 OIDC 的配置见 [§3.3](#33-仅-oidc-登录)。

### 1.6 自托管文档

`server.hosts` 收的是**裸主机名而不是 URL**；协议由 `server.https` 决定，端口只对 `localhost` 和裸 IP 自动补（`9bb4520`）。写错这里，通常就是 `Blocked CORS request` / `Blocked WebSocket CORS request` 以及实时同步不工作的原因——见 [§6](#6-故障排查)。

### 1.7 镜像流水线：构建与发布分离

镜像流程被重做成"构建"和"发布"两个可独立执行的手工步骤，并且保证 `latest` 不会指向非镜像产物：

* `Build Images`（`build-images.yml`）构建镜像并**只推到 GHCR**，作为交接产物；
* `Publish Docker Image`（`docker-publish.yml`）手工触发：把 GHCR 镜像**仓对仓**复制到 Docker Hub，并在两个仓库上移动 `latest` / `<channel>` / `<version>`。不重新构建，所以多平台、数 GB 的镜像发布只需几秒；
* 自托管的 native 构建会内嵌 Pro **公钥**，解析不到时构建**直接失败**，而不是产出一个无法校验授权的镜像（`fdf8f1b`）。

*提交：* `b4781c0`、`24f5675`、`d7d383b`、`d96b436`、`fa43b17`、`a49759d`、`4c85772`、`e1bcbd4`、`fdf8f1b`。

### 1.8 「导出 PDF」在自托管域名下可用

`pdfmake` 在写文档之前会用 `XMLHttpRequest` 逐个拉取字体，而 `cdn.affine.pro` 只对 AFFiNE 自家的来源返回 `Access-Control-Allow-Origin`，所以在任何其它域名上导出都会中断于

```
Access to XMLHttpRequest at 'https://cdn.affine.pro/fonts/Inter-Regular.woff'
from origin 'https://<你的域名>' has been blocked by CORS policy
Uncaught (in promise) TypeError: Failed to fetch
```

并且不产出任何文件。现在导出器会**先**从部署自身的资源根目录取字体（`<publicPath>/fonts/…`，画布字体一直用的是这里），CDN 只作为第二选择；同时会校验响应体真的是字体——路径不存在时 SPA 回退会返回 `200 text/html`，只看 `response.ok` 会把它当成字体收下。

镜像里现在带了一份中文字体：`SarasaGothicCL-Regular.ttf`，是 Sarasa Gothic CL 的 2.5 MB 子集，覆盖 GB2312 加上拉丁、希腊、西里尔、假名与中文标点。可用 `python tools/pdf-fonts/subset-fonts.py --coverage gbk` 重新生成或加宽，详见 [`tools/pdf-fonts/README.md`](./tools/pdf-fonts/README.md)。中文字体缺失时导出仍会进行（用 Inter 并打警告）：拉丁文本正常，中文不行；一款字体都加载不到时会直接报出所有尝试过的 URL，而不是一直挂住。

### 1.9 AI 助手：失败原因能被看见了

自托管上 BYOK 配置好、点「测试」也通过，对话仍可能失败，而且原因被藏在服务端日志里。这条链路上有两个坑，现在都补上了：

* **原生错误前缀没被剥掉。** native 层把所有错误包成 `[affine-runtime:<code>] <message>`，`BackendRuntimeCoreOperations.measured` 会剥掉它，但**流式**返回的错误（`executeCopilotStream`）不经过那个包装，于是 `mapNativeSemanticError` 匹配不上 `no_compatible_target`，原样抛给 SSE 层，用户只看到 `Internal Server Error: An internal error occurred`。现在匹配前先剥前缀，用户会看到真正的可操作提示（写明了去 Settings → Workspace → AI 检查什么）。
* **`profiles=0` 说不清为什么。** 以前「密钥加在别的工作区」「密钥被停用」「本地租约过期」「没有授权」四种情况长得一模一样：SQL 里直接 `WHERE enabled = TRUE` 把停用的行滤掉了，本地租约的每条过滤也都是静默的。现在 `load_profiles` 会把每条跳过原因收集起来，在一条 profile 都没加载到时打印一行 `[affine-copilot] no BYOK profile was loaded (…): <原因>`。
* **不支持工具调用的模型不再等于整个 AI 面板不可用。** 见下。

**关于 tool calling：不是必须的。** 模型只会输出文本也能对话：路由找不到支持工具调用的模型时，会**丢掉工具定义重试一次**，打印 `[affine-copilot] no tool-capable model for chat.default: retried without tool definitions …`，然后按普通对话补全返回。代价是助手失去了「伸进工作区」的能力——内置的 `Chat With AFFiNE AI` 提示词自带 15 个工具（`docSearch`/`docRead`/`artifactSearch`/`webSearch`/`docCompose`/`codeArtifact` …），没有工具调用它们一个都不会执行。对话输入框里的「Workspace All Docs」开关负责关掉其中 8 个；给模型勾上「动作」才是打开其余工具的方式。设置面板把两件事分开显示：「写作与聊天」需要文本输出，「动作与结构化输出」才需要工具调用。

---

## 2. 部署

### 2.1 docker compose

```yaml
services:
  affine:
    image: ghcr.io/lllvcs/affine:latest   # 或 Docker Hub 的 lvcs/affine:latest
    restart: unless-stopped
    ports:
      - '3010:3010'
    volumes:
      - ./config/config.json:/app/config.json:ro   # 见 §2.2
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

镜像启动时会自己跑迁移（`affine_migration_job`），全新数据库在第一次启动时完成初始化。

### 2.2 配置文件必须放在哪里

这是自托管 AFFiNE 最容易踩的坑：有**两个读取方**，读的是**两个不同路径**。

| 读取方 | 读取路径 |
| --- | --- |
| Node 服务端（TS） | 优先 `/app/config.json`，否则 `$HOME/.affine/config/config.json`（镜像里 `$HOME` 是 `/root`） |
| 原生运行时（Rust） | 由 `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` 指定的文件 |

两者不一致时，会出现"日志显示配置已生效、但一半配置其实没生效"的情况，`server.hosts` 之类也进不了 CORS 允许列表。**做法：把同一份文件挂到 `/app/config.json`，并让 `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` 指向同一个文件**——上面的 compose 就是这么写的。

### 2.3 开启 BYOK 后必须提供 `crypto.privateKey`

`copilot.byok.enabled: true`（持久化 BYOK）要求一个**稳定**的私钥，否则服务端拒绝启动：

```
[affine-runtime:invalid_state] stable crypto.privateKey is required when persistent BYOK is enabled
```

它的值必须是真正的 **EC P-256 私钥（PEM / PKCS#8）**——随便一个随机字符串会让 Node 侧解析失败并报 `error:1E08010C:DECODER routines::unsupported`，因为服务端是用 `createPrivateKey()` 解析它的。用镜像自带的 Node 生成：

```sh
docker compose exec affine node -e "
const {generateKeyPairSync}=require('crypto');
const {privateKey}=generateKeyPairSync('ec',{namedCurve:'prime256v1'});
console.log(JSON.stringify({crypto:{privateKey:privateKey.export({format:'pem',type:'pkcs8'}).toString()}},null,2));
"
```

把打印出来的 `crypto` 段整块粘进 `config.json`（`\n` 是 JSON 字符串里的转义换行，必须保持在**一行**内），然后重启。

> **注意**：这个私钥同时用于加密存储的 BYOK API key（HKDF 派生信封）。**换掉它会让已存的凭据解不开**，需要重新填一次 API key。如果数据库里还能读到旧值，优先用旧值：
> `select value from app_configs where id = 'crypto.privateKey';`

### 2.4 首次部署检查清单

1. `server.externalUrl` 设成你**实际访问**的地址（如 `https://note.example.com`）。
2. 所有入口（Tailscale IP、局域网 IP、反代域名）都写进 `server.hosts`——裸主机名、不带协议；主机名要显式带端口。
3. 启用 BYOK 时必须已有 `crypto.privateKey`（见 §2.3）。
4. OIDC 配好并**成功登录一次**之后，再关闭其它登录方式。
5. 服务端能访问，且启动日志里 `Telemetry allowed origins updated: …` 包含你用到的每个入口。

---

## 3. 配置参考

### 3.1 `config.json`

下列键都会被本分支读取；标 *(native)* 的由原生运行时校验，也可以用 §3.2 的环境变量提供。

```jsonc
{
  "$schema": "https://github.com/toeverything/affine/releases/latest/download/config.schema.json",
  "deployment": { "type": "selfhosted" },

  "server": {
    "name": "AFFiNE",
    "externalUrl": "https://note.example.com",
    "https": false,
    "host": "localhost",                                    // 环境变量 AFFINE_SERVER_HOST
    "hosts": ["100.64.0.1", "nas.local:3010"],             // 仅 config.json
    "port": 3010,                                           // 环境变量 AFFINE_SERVER_PORT
    "listenAddr": "0.0.0.0",                                // 环境变量 LISTEN_ADDR
    "path": ""                                              // 环境变量 AFFINE_SERVER_SUB_PATH
  },

  "crypto": { "privateKey": "-----BEGIN PRIVATE KEY-----\n…\n-----END PRIVATE KEY-----\n" },

  "oauth": {
    "providers": {
      "oidc": {
        "issuer": "https://idp.example.com/webman/sso",
        "clientId": "…",
        "clientSecret": "…",
        "allowPrivateNetwork": false,        // IdP 在内网时需要打开
        "trustUnverifiedEmail": false,       // 见 §1.3
        "args": { "scope": "openid email", "claim_email_verified": "email_verified" }
      }
    }
  },

  "auth": {
    "allowSignup": true,
    "allowSignupForOauth": true,
    "requireEmailDomainVerification": false,
    "newAccountActionDelay": 0,
    "signInMethods": { "password": true, "magicLink": true, "oauth": true },  // 见 §1.5
    "session": { "ttl": 2592000, "ttr": 86400 },
    "token": { "accessTokenTtl": 3600, "refreshIdleTtl": 2592000,
               "refreshAbsoluteTtl": 31536000, "refreshGracePeriod": 30,
               "refreshRetention": 2592000 }
  },

  "copilot": {
    "enabled": true,
    "byok": {
      "enabled": true,
      "allowCustomEndpoint": true,      // 使用任何非官方端点都必需
      "allowPrivateEndpoint": true,
      "allowedProviders": []            // 空 = 全部允许
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

`db.datasourceUrl`（`DATABASE_URL`）与 `mailer.*`（`MAILER_*`）在官方镜像里通过环境变量配置；只有需要用"魔法链接邮件"登录时才需要配 `mailer.*`。

### 3.2 环境变量

| 变量 | 对应配置 | 说明 |
| --- | --- | --- |
| `AFFINE_SERVER_EXTERNAL_URL` | `server.externalUrl` | 生成链接与允许来源列表用的基础地址 |
| `AFFINE_SERVER_HOST` | `server.host` | 单个主机名，默认 `localhost` |
| `AFFINE_SERVER_PORT` | `server.port` | 默认 `3010` |
| `AFFINE_SERVER_HTTPS` | `server.https` | 布尔，默认 `false` |
| `AFFINE_SERVER_SUB_PATH` | `server.path` | 子路径部署，如 `/affine` |
| `LISTEN_ADDR` | `server.listenAddr` | 默认 `0.0.0.0` |
| `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` | — | 原生运行时读取的 JSON 配置路径（见 §2.2） |
| `AFFINE_PRIVATE_KEY` | `crypto.privateKey` | §2.3 生成的 PEM 私钥 |
| `AFFINE_AUTH_SIGN_IN_PASSWORD` | `auth.signInMethods.password` | 布尔：`1`/`true` 为开，其它值为关 |
| `AFFINE_AUTH_SIGN_IN_MAGIC_LINK` | `auth.signInMethods.magicLink` | 同上 |
| `AFFINE_AUTH_SIGN_IN_OAUTH` | `auth.signInMethods.oauth` | 同上 |
| `DATABASE_URL` | `db.datasourceUrl` | PostgreSQL 连接串 |
| `REDIS_SERVER_HOST` / `_PORT` / `_DATABASE` / `_USERNAME` / `_PASSWORD` | `redis.*` | |
| `MAILER_HOST` / `_PORT` / `_USER` / `_PASSWORD` / `_SENDER` / `_SERVERNAME` / `_IGNORE_TLS` | `mailer.*` | 魔法链接邮件 |
| `GA4_MEASUREMENT_ID`、`GA4_API_SECRET` | 遥测 | 可选 |

`server.hosts` **没有**环境变量，只能写在 `config.json` 里。你在别的 compose 文件里见过的某些环境变量（例如 `AFFINE_INDEXER_ENABLED`）在这份代码里**没有接线**，请改在 `config.json` 中设置对应键。

上面的布尔开关：值为 `1` 或 `true`（不分大小写）表示启用，其它值表示关闭。若同一开关也在 `config.json` 里出现，**以文件为准**——环境变量只填充文件未设置的值。

### 3.3 仅 OIDC 登录

如果你跑 SSO 的目的就是不要本地密码、不要魔法链接：

```json
{
  "auth": {
    "signInMethods": { "password": false, "magicLink": false, "oauth": true }
  }
}
```

或者不动文件、用环境变量试：

```sh
AFFINE_AUTH_SIGN_IN_PASSWORD=false AFFINE_AUTH_SIGN_IN_MAGIC_LINK=false AFFINE_AUTH_SIGN_IN_OAUTH=true
```

> ⚠️ **别把自己锁在外面**：先用 OIDC 登录一次并确认该账号有后台权限，关掉之后就再没有别的入口了。留一份改动前的 `config.json`，出问题时重启容器即可回退。

登录页仍会显示邮箱输入框——可用性是按**邮箱**判定的；输入后会提示该邮箱不可用于登录，OIDC 按钮不受影响。

---

## 4. 镜像、标签与发布

| 仓库 | 镜像 |
| --- | --- |
| GHCR | `ghcr.io/lllvcs/affine` |
| Docker Hub | `lvcs/affine` |

标签：`latest`（最近发布）、渠道标签（`canary`）、版本标签（`0.27.5`），以及带短提交号的构建标签（如 `canary-d96b436`）。

发布刻意分成两个手工步骤：

1. 运行 **Build Images**（`build-images.yml`，也可通过 `docker-build.yml` 触发）——构建并推到 GHCR。输入含 `build-type`、`app-version`、`git-short-hash`、`image-namespace`、`platforms`、`build-admin`、`build-mobile`。
2. 运行 **Publish Docker Image**（`docker-publish.yml`），输入 `source-tag`（默认取最近一次成功构建）、`dockerhub`、`moving-tags`。

`latest` 只在第 2 步、且只指向第 1 步产出的镜像，因此不可能指向非镜像产物。

> ⚠️ **不要把构建版本号改到低于源码版本。** 网页端由这份源码构建，会把构建版本号上报给同步网关，而网关对批量协议要求 `>=0.27.5`。把镜像标成 `0.27.4` 会让浏览器端被拒（WebSocket join 被踢），尽管源码没变。

---

## 5. 升级

1. `docker compose pull`（或为你自己的改动重新构建镜像）。
2. `docker compose up -d`——迁移会自动执行。
3. 看服务端日志前 30 行：迁移任务必须跑完，并且服务端会为你的 `server.externalUrl` 打印 `recognized as …`。

`config.json` 的改动重启后生效；环境变量的改动一律需要重启。

### 5.1 与上游同步

`upstream` 指向 `https://github.com/toeverything/AFFiNE.git`。本分支自己的提交都压在 `canary` 上，所以同步就是一次普通合并：

```sh
git fetch upstream canary
git merge upstream/canary          # 解决冲突后跑一遍 §7 里的检查
git push origin canary
```

最近一次同步：上游 `0b69a6d`（22 个提交——依赖升级，含 Electron 41 / eslint 10 / esbuild 0.28 / nodemailer 10 / dompurify 3.4.16 等安全更新，m4a 音频切片、transcript 截断、blob 读缓存与同步时序，`packages/frontend/native` 的 obsidian/onenote 导入器重写，以及 latex/katex 0.18）。合并无冲突：双方只碰到 `packages/backend/native/index.d.ts`、`packages/backend/server/src/native.ts`、Electron 的 `package.json` 和两个 `i18n` 资源文件，且改动区域互不重叠。

会动到 `yarn.lock` 的同步之后需要跑 `yarn install`。

---

## 6. 故障排查

**实时同步起不来，日志有 `Blocked CORS request` / `Blocked WebSocket CORS request`。** 允许来源列表由 `server.externalUrl`、`server.host`、`server.hosts`、`server.port` 决定。把你实际访问的来源加进去（裸主机名、不带协议），并确认启动日志 `Telemetry allowed origins updated: …` 里列出了它。

**客户端连上就断，日志出现 `Rejected WebSocket join …`。** 网关会拒绝低于协议门槛的客户端。自 `9b9f22e` 起，支持范围是：旧握手 `>=0.25.0`，`space:join-batch` 为 `>=0.27.5`；日志会写明客户端上报的版本。已发布的客户端（手机 0.27.1、桌面 0.27.4）走旧路径，并会打印 `Legacy sync join accepted`。

**新建工作区报 `DOC_NOT_FOUND`（`Doc <id> under Space <id> not found`）。** 工作区根文档是**客户端**创建并通过同步推上去的；WebSocket 进不来，它就永远不存在。先修好上面的同步链路，再用能正常同步的客户端打开该工作区（或新建一个）。

**AI 对话报 `no_compatible_target`。** 先看服务端日志里紧跟其后的那行 `[affine-copilot] no BYOK profile was loaded (…): …`（自本次改动起），它直接说明密钥为什么没被加载：没加在**当前**工作区、被停用了、本地租约过期，或者没有授权。四者症状完全一样，别靠猜。

确认密钥本身没问题后，再看能力要求：**现在只有文本输出的模型也能对话**——路由找不到支持工具调用的模型时会丢掉工具定义重试一次。如果仍然报 `no_compatible_target`，紧挨着的那行诊断会逐个列出已加载的 profile 以及是否匹配：找 `matchesSlot=false` 和它后面的 `capabilities=[…]`。想要助手能检索工作区、读取文档，就给该模型勾上「动作」；只想要普通对话，不勾也行。注意：BYOK 的"测试"按钮是直接探测你的提供方，**不走路由决策**，所以测试通过不代表对话可用。

**AI 对话能答，但明显没读当前文档、也没搜工作区。** 这就是降级到「无工具」的表现：选中的模型没声明工具调用，所以这次请求没有下发任何工具。给模型勾上「动作」，或换一个声明了工具调用的模型即可恢复。

**AI 对话报 `Internal Server Error: An internal error occurred`，日志里有 `copilot route unavailable: …`。** 旧镜像上这是错误映射失效的表现（见 §1.9），真正的失败原因就在那行 native 诊断里：`slot=` 是请求的槽位，`workspace=` 是发起对话的工作区，`profiles=` 是加载到的 BYOK profile 数量。`profiles=0` 表示那条工作区下没有任何**已启用**的密钥参与路由——用下面的 SQL 对照 `workspace=` 的值即可：

```sql
select id, workspace_id, provider, name, enabled, sort_order
from ai_workspace_byok_configs
order by workspace_id, sort_order;
```

**「导出 PDF」没有任何文件产出，控制台里 `cdn.affine.pro` 的字体请求被 CORS 拦截。** 导出器在访问 CDN 而不是你的实例。确认 `<你的域名>/fonts/Inter-Regular.woff2` 返回的是 `font/woff2`（而不是 `text/html`），并且镜像里有 `packages/frontend/core/public/fonts/SarasaGothicCL-Regular.ttf`；见 §1.8 与 [`tools/pdf-fonts/README.md`](./tools/pdf-fonts/README.md)。

**`affine_server` 无限重启。** 看报错内容：缺 `crypto.privateKey` 或格式不对会分别报 `stable crypto.privateKey is required …` 与 `1E08010C:DECODER routines::unsupported`，见 §2.3。

**改了配置好像没生效。** 两个读取方、两个路径——见 §2.2。

---

## 7. 开发

本分支的改动集中在这些位置：

| 模块 | 路径 |
| --- | --- |
| 原生运行时配置、同步网关、认证 | `packages/backend/native/src/runtime/**`、`packages/backend/server/src/core/sync/gateway.ts` |
| 认证 HTTP 层 | `packages/backend/server/src/core/auth/{controller,service,config}.ts` |
| OAuth/OIDC | `packages/backend/server/src/plugins/oauth/*`、`packages/backend/native/src/runtime/backend_runtime/auth_session/*` |
| Copilot 诊断 | `packages/backend/native/src/runtime/backend_runtime/copilot/*`、`packages/backend/server/src/plugins/copilot/runtime/native-errors.ts` |
| 前端登录 / BYOK | `packages/frontend/core/src/components/sign-in/*`、`.../setting/workspace-setting/byok/*` |
| 配置管线 | `packages/backend/server/src/base/config/*` |
| PDF 导出字体 | `blocksuite/affine/shared/src/adapters/pdf/pdf-fonts.ts`、`packages/frontend/core/public/fonts/*`、`tools/pdf-fonts/*` |
| 镜像流水线 | `.github/workflows/*.yml` |

验证命令（都在仓库根目录执行）：

```sh
# 原生层（Rust）——cargo 不在 PATH 时补上你的 rustup 工具链路径
cargo check -p affine_server_native --lib

# 服务端（TypeScript）
node_modules/.bin/tsc -p packages/backend/server/tsconfig.json --noEmit

# lint
node node_modules/oxlint/bin/oxlint <改动文件>

# schema 变更后重新生成 GraphQL / native 绑定
yarn affine <task>
```

后端测试基于 `ava`，需要 PostgreSQL、Redis 和已构建的 native 模块：
`yarn workspace @affine/server test`。涉及同步协议或认证面的改动**值得配一个真实客户端的验证**，因为它们依赖的东西（socket 房间、权限、客户端版本）无法用单元测试覆盖。

> **native 所有的键存在于两份名单里。** `NATIVE_APP_CONFIG_KEYS`
> （`packages/backend/native/src/runtime/config/store.rs`）决定 native 运行时存储与校验哪些键；
> `node_owned()`（`packages/backend/native/src/runtime/config/file.rs`）决定其中哪些**允许 Node 层读取**。
> 两侧都要消费的键必须**同时**登记在两处；只登记第一处，`config.json` 里写了也对服务端无效——
> 值被解析、被持有，但从不交付。`auth.signInMethods` 就是这种情况，是那条诊断日志
> （`sign-in method policy: password=… magicLink=… oauth=…`）让它现形的。
> 例外是 Node 层通过专用通道消费的键（`crypto.privateKey` 走配置事件、`redis.*` 走 `redis_node_options_json()`）。

---

## 8. 上游与许可

本分支跟随上游 AFFiNE，并沿用其许可：见 [`LICENSE`](./LICENSE) 与 [`LICENSE-MIT`](./LICENSE-MIT)。AFFiNE 的一切功劳属于
[TOEVERYTHING PTE. LTD. 及其贡献者](https://github.com/toeverything/AFFiNE)。
本仓库的改动即 [§1](#1-本分支相对上游的改动) 所列的那些——它们的存在只是为了让自托管部署能正常工作，也希望能对别人有用。

`packages/frontend/core/public/fonts/SarasaGothicCL-Regular.ttf` 是 [Sarasa Gothic](https://github.com/be5invis/Sarasa-Gothic)（作者 Renzhi Li 及贡献者）的子集，按 SIL Open Font License 1.1 分发——许可全文随字体一同提供，见 `SarasaGothicCL-LICENSE.txt`。同目录下的 `Inter-*.woff2` 来自 Inter 项目，同样是 OFL。
