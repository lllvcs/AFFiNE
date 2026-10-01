# AFFiNE 自托管分支 — 开发经验（skill 式）

> **什么时候读这份文件。** 在本分支上改配置、认证、同步或镜像流水线之前；以及自托管实例里某个设置"不生效"的时候。部署与配置**用法**看 [`README.zh-CN.md`](./README.zh-CN.md)；这份文件是**开发经验**——改了什么、有哪些契约、哪些坑已经付过代价。
>
> 全文写成规则：**做 X → 因为 Y → 用 Z 验证**。每条自洽，按需查阅，不必通读。

- 基线：上游 AFFiNE 源码，版本 `0.27.5`，分支 `canary`
- 镜像：`ghcr.io/lllvcs/affine`（构建产物）→ `lvcs/affine`（发布产物）
- 本机没有 docker、也没有旧客户端：协议层与端到端行为**只能在部署实例上验证**

## 快速索引

| 规则 | 什么时候用 |
| --- | --- |
| R1 native 键要登两份名单 | 新增一个服务端也要读的配置键 |
| R2 所有权决定来源 | 同一个键"env 有效、文件无效"（或反之） |
| R3 Node 层从不读 `config.json` | 你期待 TS 代码读到文件里的值 |
| R4 两个读取方指向同一份文件 | "一半配置生效" |
| R5 每个跳过/拒绝分支都要有日志 | 故障在任何日志里都看不见 |
| R6 用构建标签判断客户端 | 客户端连不上/不同步 |
| R7 被删的协议要加法式回移 | 已发布客户端必须继续可用 |
| R8 别用 `trust proxy` 修 URL 协议 | 生成的 URL 协议不对 |
| R9 `server.hosts` 收裸主机名 | CORS/WebSocket 被拒 |
| R10 `crypto.privateKey` 有两个消费者 | 服务端因密钥崩溃重启 |
| R11 没重跑过就不算验证过 | 类型检查/grep 结果好得不真实 |
| R12 明说没验证的部分 | 准备写"完成"之前 |

---

## R1. native 所有的键要登记到**两份**名单

**做。** 一个键既归 native 运行时所有、服务端也要读时，两处都要加：

| 名单 | 文件 | 含义 |
| --- | --- | --- |
| `NATIVE_APP_CONFIG_KEYS` | `packages/backend/native/src/runtime/config/store.rs` | native 运行时存储并校验它 |
| `node_owned()` | `packages/backend/native/src/runtime/config/file.rs` | 它会被**投影**给 Node 层 |

**因为。** native 解析了 `config.json` 并持有该值，但只有投影会跨过 native→Node 的边界。只登第一处时，`config.json` 对服务端**静默失效**：解析了、持有、从不交付。`auth.signInMethods` 正是如此，排查了四轮。

**验证。** `grep -n "<你的键>" <两个文件>`，然后在实例上看打印解析值的那行日志。

---

## R2. 所有权决定"哪个来源有效"

**做。** 按**谁强制这个键**来定所有者：服务端强制 → Node（TS）所有的键；运行时强制 → native 键。两个来源都要能用时，保持 native 所有，并确保环境变量仍能兜底。

**因为。** `getDefaultConfig(excludedKeys)` 会**跳过** native 所有的键，因此它们的环境变量从来不会被读取；而 TS 所有的键只能来自环境变量或默认值（见 R3）。所以把一个键在两层之间搬家，等于**用文件换环境变量**——每翻一次，用户就多花一轮重启。

**验证。** 同一个值同时写进 `config.json` 和环境变量：以文件为准；再删掉文件里的值，确认环境变量仍生效。

---

## R3. Node 层**从不**读取 `config.json`

**做。** 不要期待某个 TS 可见的配置项能从挂载文件里取到值。要么用环境变量，要么让运行时投影它（R1）。

**因为。** `OVERRIDE_CONFIG_TOKEN` 只由代码注入（`ConfigModule.override()`）；`env.ts` 打开文件只是为了构造 native 句柄。文件**只能**经由 native 的投影到达服务端。

**验证。** `grep -rn "OVERRIDE_CONFIG_TOKEN" packages/backend/server/src`。

---

## R4. 让两个读取方指向同一份文件

**做。** 同一份文件挂到 `/app/config.json`，**并且**让 `AFFINE_BACKEND_RUNTIME_CONFIG_PATH` 指向同一个路径。

**因为。** Node 读取方优先 `/app/config.json`，否则回退 `$HOME/.affine/config/config.json`；native 读取方只认它那个环境变量。两者不一致就会出现"一半配置生效"：一部分正常、另一部分静默用默认值，而启动日志看起来完全健康。

**验证。** `docker compose exec <服务> sh -c 'printenv | grep RUNTIME_CONFIG'` + `ls -la /app/config.json`。

---

## R5. 每个跳过/拒绝的分支都要有日志

**做。** 任何"拒绝客户端 / 丢弃配置行 / 返回 no compatible target 类错误"的分支，都要打一行日志并写明它判断的输入。当某个配置项的**解析值**在线缆上不可观测时，就在读取它的分支打点。

**因为。** 这些路径原本都是静默的：join 被拒只表现为"连上就断"，被策略跳过的 BYOK profile 直接消失。`sign-in method policy: password=… magicLink=… oauth=…` 一行就结束了四轮猜测。失败串要保持机器可读的 code 作为**首 token**，上下文追加在后面——并且先 grep 这个串的所有消费方。

**验证。** 触发一次该路径，确认日志里的值与预期一致。

---

## R6. 用**构建标签**判断客户端，而不是源码

**做。** 想知道客户端说的是哪套协议，读它自报的版本（`x-<app>-version` 头、join 载荷里的 `clientVersion`）和服务端的门槛。**不要**用改标签的方式"降级"镜像。

**因为。** 客户端版本是**构建期标签**：改标签不会改变客户端实际运行的代码；而服务端同时托管前端时，网页端上报的也是同一个标签——为了迁就旧客户端去改标签，会把本来正常的浏览器端弄坏。这个误判让排查多花了两轮。

**验证。** 要 join 帧的 `clientVersion` 或请求头，而不是镜像 tag。

---

## R7. 被删掉的协议要**加法式**回移

**做。** 已发布客户端必须跨协议变更继续可用时：从旧发布版的 tag 拉取它自己的源码作为参考实现，把旧 handler **加在新 handler 旁边**，用新路径本就会设置的标记（房间成员）识别旧客户端，对它们保留**旧的每请求鉴权**，把输出**双发**到两条路径，并打一行"已接受"日志。

**因为。** 0.27.5 用 `space:join-batch` 取代了房间协议并**删除了**旧 handler，于是所有已发布客户端（0.27.4 桌面端、0.27.1 手机端）在 join 时被拒——socket 连上就断、完全不同步、工作区根文档永远推不上去。只降低门槛，只会把"响亮的拒绝"变成"静默的不同步"。

**验证。** 实例日志要出现带客户端版本的接受行；协议层**没有真实旧客户端就无法验证**。

---

## R8. 别用 `trust proxy` 修 URL 协议

**做。** 面向客户端的 URL（OAuth `redirect_uri`、邮件链接）按这个优先级取来源：管理员配置的规范 URL（对它自己的主机）→ 请求实际使用的协议（forwarded-proto，且仅限管理员列出的主机）→ 监听标志（最后这条是后台任务与既有单测依赖的行为，必须保留）。

**因为。** 监听标志描述的是**服务器怎么监听**，不是**客户端怎么访问**；TLS 终止在反代、该标志合法地为 `false` 的实例会生成 `http://` 回调。为了读转发头而打开 `trust proxy`，会让框架信任客户端自报的地址，并**静默削弱 IP 限流**。

**验证。** 从两个入口各发起一次 OAuth preflight，读响应里的 `redirect_uri`。

---

## R9. `server.hosts` 收**裸主机名**

**做。** 写成 `["100.64.0.1", "nas.local:3010"]`——不带协议；协议来自 `server.https`，端口只对 `localhost` 和裸 IP 自动补。

**因为。** URL 形状的条目会被拼成 `http://http://host`，永远匹配不上请求来源。这份列表同时是 CORS/WebSocket 的允许列表，所以症状是实时连接被拒，而不是一条配置报错。

**验证。** 启动日志 `Telemetry allowed origins updated: …` 必须列出你用到的每个入口。

---

## R10. `crypto.privateKey` 有两个消费者

**做。** 按**最严格**的那个消费者的格式生成：真正的 EC P-256 PEM，用镜像自带的 Node 生成（`generateKeyPairSync('ec', { namedCurve: 'prime256v1' })`，导出 `pkcs8/pem`），以单行 JSON（`\n` 转义）粘贴。数据库里还有旧值时**优先复用**（`select value from app_configs where id = 'crypto.privateKey'`），并提前告知：换新 key 会让已存凭据全部失效。

**因为。** 运行时只要求**非空且稳定**的字符串（KDF 根），但 Node 层会用 `createPrivateKey()` 解析同一个值，遇到随机字符串就崩溃重启（`error:1E08010C:DECODER routines::unsupported`）。

**验证。** 服务正常启动，日志为你的 `server.externalUrl` 打印 `recognized as …`。

---

## R11. 没重跑过就不算验证过

**做。** 改完被广泛引用的类型或名单后，删掉 `packages/backend/server/dist/tsconfig.tsbuildinfo` 再跑 `node_modules/.bin/tsc -p packages/backend/server/tsconfig.json --noEmit`。用 `tsc -p … --noEmit --listFiles | grep <文件>` 证明某文件**确实**参与编译（命中 1 次即在内）。cargo 不在 PATH 时，直接调工具链并显式指定缓存：`CARGO_HOME=… RUSTUP_HOME=… CARGO_TARGET_DIR=… ~/.rustup/toolchains/*/bin/cargo.exe check -p affine_server_native --lib`。

**因为。** 复合工程 + 陈旧的 build info 会给出**上一版内容**的结论（既有假红也有假绿）。对 minified bundle 用 `grep -c` 数的是**行数**不是次数，"2" 什么也证明不了。

**验证。** 改完再跑，不要用改之前的结果。

---

## R12. 明说没验证的部分

**做。** 每次改动都给出两张清单：本地**实际执行过**的（typecheck、lint、cargo check）和**只能在实例上完成**的（协议、OIDC 往返、客户端兼容）。两张都写进提交信息。

**因为。** 这台机器跑不了 docker。"应该可用"已经错过不止一次；用户只能测你明确告他测的东西。

---

## 提交前检查清单

1. `cargo check -p affine_server_native --lib`（干净）。
2. `tsc -p packages/backend/server/tsconfig.json --noEmit`（干净，且 build info 是新的）。
3. `oxlint <改动文件>`。
4. 新增 native 键？两份名单都更新（R1），并确定来源优先级（R2）。
5. 新增拒绝/跳过分支？日志写明它判断的输入（R5）。
6. 提交信息：现象、影响面、`Verified:` 与 `Not verified:` 分列。

## 待办

- 登录开关**没有自动化测试**（auth 的 spec 需要活数据库与 native 模块）。在实例上确认行为后补 controller 级用例。
- `db.datasourceUrl` 归 native 所有但**未投影**；服务端读的是 `DATABASE_URL`。若希望数据库地址能只在文件里配置，需要加进投影（它是密钥，投影不过滤密钥）。
- 旧同步协议兼容层**没有端到端验证**（需要真实的 0.27.4 桌面端与 0.27.1 手机端各一次）。
