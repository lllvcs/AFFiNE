# AFFiNE 自托管分支：开发与踩坑记录

> 这份文档面向**在本次 fork 上继续开发的人**，记录改动落点、架构契约和实际踩过的坑。
> 部署与配置用法请看 [`README.md`](./README.md) / [`README.zh-CN.md`](./README.zh-CN.md)；本文只讲"为什么这样改、哪里容易翻车、怎么验证"。

- 基线：上游 AFFiNE 源码，`package.json` 版本 `0.27.5`，分支 `canary`
- 镜像：`ghcr.io/lllvcs/affine`（GHCR，构建产物）→ `lvcs/affine`（Docker Hub，发布产物）
- 本机无法起完整栈（没有 docker）：**协议层与端到端行为必须在部署实例上验证**，本文标注了哪些结论是本地可验证的、哪些不是

---

## 1. 改动清单（按主题）

| 主题 | 提交 | 一句话 |
| --- | --- | --- |
| 自托管配额放开 | `c5844b1` | blob 10 GiB、存储配额 2^53−1、坐席 `i32::MAX`、历史 100 年 |
| BEYOK 模型列表 + Copilot 路由可诊断 | `c5844b1` `a16e5f3` `4d1f8b8` | 路由失败带上槽位/所需能力/部署类型；前端提示"对话需 text 输出 **且** tool calling" |
| OIDC 兼容不返回 `email_verified` 的 IdP | `42bbe52` | `trustUnverifiedEmail`（显式 false 仍拒），错误原因细化，3 个 Rust 单测 |
| `redirect_uri` 跟随请求协议 | `c9d5013` | externalUrl 主机优先 → 否则请求协议（仅白名单主机）→ 否则 `server.https` |
| 登录方式开关 | `28ac1be` `8bc95d7` `b57a6b9` `4b5a1c1` `7100627` `ed18a8c` | `auth.signInMethods.{password,magicLink,oauth}`，config.json 与环境变量均可，服务端强制 |
| 实时同步兼容已发布客户端 | `9b9f22e` `6912abd` | 回移 `space:join` / `sync-025`/`sync-026` 房间；join 被拒时打印版本 |
| 自托管文档 | `9bb4520` `f422eeb` `3fb9695` | `server.hosts` 语义、双读取方、两份名单规则 |
| 镜像流水线 | `b4781c0`…`fdf8f1b` | 构建与发布分离、仓对仓复制、Pro 公钥守门 |

---

## 2. 架构契约（改代码前必须知道）

### 2.1 配置系统是三层，且**所有权决定来源**

```
Rust AuthConfigFile/AuthConfigFile 等  ──schema+默认值──┐
        │ descriptors(含 envName/envType)              │
        ▼                                              │
TS defineNativeModuleConfig → ConfigItem<…> 叶子        │  native 层
        │                                              │
        ▼                                              │
消费方：native 运行时 / TS 服务端                        ┘
```

- **TS 侧永远不从 `config.json` 读值**：`OVERRIDE_CONFIG_TOKEN` 只由 `ConfigModule.override()` 以代码注入，`env.ts` 打开文件只是为了构造 native 句柄。
- 因此：**TS 所有的键**只能来自环境变量或默认值；**native 所有的键**来自文件/DB（经原生层投影给 Node）。
- `getDefaultConfig(excludedKeys)` 会**跳过** native 所有的键；`4b5a1c1` 之后，声明了 `env` 的 native 键不再被跳过（环境变量可作为兜底，文件/DB 值仍优先）。

**两份名单（最容易翻车的地方）**

| 名单 | 位置 | 作用 |
| --- | --- | --- |
| `NATIVE_APP_CONFIG_KEYS` | `native/src/runtime/config/store.rs` | 这个键归 native 所有、存储、校验 |
| `node_owned()` 投影 | `native/src/runtime/config/file.rs` | 其中哪些**交给 Node 层读**（整模块 + 显式叶子键） |

> 新增一个 **native 所有、但服务端也要读** 的键，必须**同时**登记到两处。只登第一处时：文件里写了、native 解析了、也持有，但**不投影给 Node** → 服务端继续用自己的默认值。`auth.signInMethods` 就是这种情况，排查了四轮才定位。

TS 侧还有两个类型级白名单：`AuthConfig` 里的叶子用 `ConfigItem<T>` 标记，`base/config/types.ts` 的 `NodeConfig` 用 `Pick<…>` 决定服务端能读到哪些模块键；环境变量类型白名单在 `base/config/register.ts`（`string | integer | boolean`）。

### 2.2 配置文件有两个读取方、两条路径

| 读取方 | 路径 |
| --- | --- |
| Node 服务端 | `/app/config.json` 优先，否则 `$HOME/.affine/config/config.json`（镜像里 `$HOME=/root`） |
| native 运行时 | `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` 指向的文件 |

两者不一致时会出现"一半配置生效"的假象。**正确做法：同一份文件挂到 `/app/config.json`，并让 `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` 指向它。**

### 2.3 同步协议与客户端版本

- 0.27.5 用 `space:join-batch` 取代旧握手，**并删除了旧 handler**（不是加闸门）。房间：`sync-025`（0.25 逐条）、`sync-026`（0.26/0.27 批量）、`sync-027`（0.27.5 内存态订阅 + invalidation）。
- 客户端上报的版本是**构建标签**，不是源码版本：改标签不会"降级"客户端，反而会让由服务端托管的网页端一起被批量闸门拒掉。
- 要让旧客户端可用，正确做法是**加法式回移**：新 handler 旁边加旧 handler、按房间区分协议、旧客户端走**每请求鉴权**（`assertDocActionAllowed`，因为它们没有内存态订阅）、文档更新**双发**（旧房间 + 新 fan-out）、join 被拒时打印版本。

---

## 3. 踩过的坑（按代价排序）

### 3.1 静默跳过是这类系统最难查的 bug
`rejectJoin()` 原本只有 `client.disconnect()`，没有任何日志 → 表现是"连上就断"，服务端和客户端都看不到原因，我们靠盲猜花了数轮。**结论：给每一个"拒绝/跳过"分支加一行日志**，并在解析到关键值的地方打点（`sign-in method policy: password=… magicLink=… oauth=…` 一行就结束了四轮猜测）。

### 3.2 同一能力分散在两层时，别让键的"所有权"跨层
登录开关的强制在 TS，键却一度登记成 native 所有 → 环境变量被跳过、文件值不投影，两个来源轮流失效。**结论：键所有权的选择，以"谁强制它"为准。**

### 3.3 密钥有两个消费者，格式要求不同
`crypto.privateKey`：native 侧只要求**非空且稳定**（HKDF 根），Node 侧用 `createPrivateKey()` 解析、**必须是合法 PEM**。随机字符串会让服务端 `1E08010C:DECODER routines::unsupported` 崩溃重启。**结论**：换 key 会让已存密文不可解（先从 `app_configs` 取回旧值），生成时用镜像自带 node 的 `generateKeyPairSync('ec', { namedCurve: 'prime256v1' })` 保证格式一致。

### 3.4 不要把 `trust proxy` 当万能钥匙
生成 URL 的协议曾经写死取 `server.https`（描述"服务器怎么监听"，不是"客户端怎么访问"）。想用 `trust proxy` 修它会让框架信任客户端自报地址，**顺带削弱登录限流**。**结论**：在只影响生成 URL 的位置单独读 `x-forwarded-proto`，并按"admin 配置的 externalUrl 主机 → 请求协议（仅白名单主机）→ `server.https`"排序；最后一条是后台任务与既有单测依赖的行为，必须保留。

### 3.5 `server.hosts` 收裸主机名
协议来自 `server.https`，端口只对 `localhost` 与裸 IP 自动补；写成 URL 会被拼成 `http://http://host` 永不匹配。它也是 CORS/WebSocket 允许列表的来源，写错表现为实时同步连接被拒。

### 3.6 构建标签不能低于源码版本
网页端由镜像自身构建、上报构建标签，同步网关对批量协议要求 `>=0.27.5`。把镜像标成 `0.27.4` 会让浏览器端 join 被踢——而"源码没变"这点具有极强的误导性。

### 3.7 既有单测是免费的契约检查
`base/helpers/__tests__/url.spec.ts` 立刻抓出我给"无请求上下文"分支设的默认值错误（默认 http 会让所有无请求场景从 https 变 http）。**改这类全局 helper 前先读同目录的 spec。**

---

## 4. 本地验证手册（以及陷阱）

```sh
# Rust（native 层）：cargo 不在 PATH 时用完整路径 + 指定缓存目录
CARGO_HOME=E:/AFFiNE/.cargo RUSTUP_HOME=$HOME/.rustup \
  CARGO_TARGET_DIR=E:/AFFiNE/.scratch/target \
  $HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/cargo.exe \
  check -p affine_server_native --lib

# TS（服务端）
node_modules/.bin/tsc -p packages/backend/server/tsconfig.json --noEmit
node node_modules/oxlint/bin/oxlint <changed files>
```

- **复合工程 + 陈旧 `.tsbuildinfo` 会给假结果**：改完 `types.ts` 这类被广泛引用的文件后，若结果没变，先删 `packages/backend/server/dist/tsconfig.tsbuildinfo` 再跑。
- 想确认某个文件**真的**参与了编译：`tsc -p … --noEmit --listFiles | grep <file>`（命中 1 次即在内）。
- 用 `grep -c` 检查"镜像里有没有这段代码"时要意识到 minified bundle 行数极少，`grep -c` 数的是**行**不是次数。
- **本机没有 docker / 没有旧客户端**：协议层、OIDC 往返、客户端兼容性都无法本地验证。**提交信息里必须写明"未验证的部分"**，不要用"应该可用"收尾。

---

## 5. 协作与交付经验

1. **一次只推进一个问题**：用户会指定顺序，未选的用一行"仍待处理"挂起，不打包。
2. **每条诊断命令都要能直接判读**：给出"结果 → 结论"的对照表（例如 `app_configs` 里有没有那行 = native 有没有吃下这个键），比给一堆命令更有用。
3. **先核实契约再让用户改生产配置**：本次多数返工都来自"想当然"；凡是能从代码读出来的，就不该让用户在实例上试。
4. **提交信息写清因果**：现象 → 根因 → 影响面 → 验证状态（`Verified:` / `Not verified:` 分开写）。
5. **CI 的镜像发布是两步**：构建推 GHCR（交接产物）→ 手工发布做仓对仓复制并移动 `latest`/渠道/版本标签。`latest` 只允许指向构建产物；构建前解析并守门 Pro 公钥。

---

## 6. 已知未完成 / 待补

- 登录开关**没有自动化测试**（auth 的 spec 需要活数据库与 native 模块，本机不具备）——建议在实例上确认行为后补 controller 级用例。
- `db.datasourceUrl` 属于 native 所有但**未投影**给 Node；TS 的 Prisma 依赖 `DATABASE_URL` 环境变量。若将来要支持"只在文件里配置数据库地址"，需要把它加进投影（注意它是密钥）。
- 旧同步协议的兼容层**没有端到端验证过**（需要真实的 0.27.4 桌面端 / 0.27.1 手机端各跑一次）。
- `screen` 之外的其它 native 键若将来由 TS 消费，记得同时更新两份名单（§2.1）。
